//! Owned-output node discovery. This cannot establish desktop consumer readiness.
use keyweave_types::MonoTime;
use std::{
    fs,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
const MAX_ENTRIES: usize = 64;
const MAX_DEV_BYTES: usize = 32;
pub const MAX_DISCOVERY_NS: u64 = 5_000_000_000;
pub const MIN_RETRY_NS: u64 = 1_000_000;
#[derive(Clone, Copy, Debug)]
pub struct DiscoveryPolicy {
    pub timeout_ns: u64,
    pub interval_ns: u64,
}
impl Default for DiscoveryPolicy {
    fn default() -> Self {
        Self {
            timeout_ns: 2_000_000_000,
            interval_ns: 20_000_000,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryPhase {
    Waiting,
    Complete,
    Failed,
}
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("invalid output discovery policy")]
    Policy,
    #[error("output discovery time overflow")]
    TimeOverflow,
    #[error("output discovery clock moved backwards")]
    ClockBackwards,
    #[error("output discovery deadline expired")]
    Expired,
    #[error("output discovery is no longer waiting")]
    Finished,
    #[error("output node discovery failed: {0}")]
    Probe(#[from] io::Error),
}
#[derive(Debug)]
pub enum DiscoveryPoll<T> {
    Pending { next_at: MonoTime },
    Ready(T),
}
pub struct NodeDiscovery {
    phase: DiscoveryPhase,
    deadline: MonoTime,
    next_at: MonoTime,
    last: MonoTime,
    interval: u64,
}
impl NodeDiscovery {
    pub fn new(now: MonoTime, policy: DiscoveryPolicy) -> Result<Self, DiscoveryError> {
        if policy.timeout_ns == 0
            || policy.timeout_ns > MAX_DISCOVERY_NS
            || policy.interval_ns < MIN_RETRY_NS
            || policy.interval_ns > 1_000_000_000
            || policy.interval_ns > policy.timeout_ns
        {
            return Err(DiscoveryError::Policy);
        }
        let deadline = now
            .checked_add(policy.timeout_ns)
            .ok_or(DiscoveryError::TimeOverflow)?;
        Ok(Self {
            phase: DiscoveryPhase::Waiting,
            deadline,
            next_at: now,
            last: now,
            interval: policy.interval_ns,
        })
    }
    pub fn phase(&self) -> DiscoveryPhase {
        self.phase
    }
    pub fn deadline(&self) -> MonoTime {
        self.deadline
    }
    pub fn next_wakeup(&self) -> Option<MonoTime> {
        (self.phase == DiscoveryPhase::Waiting).then_some(self.next_at)
    }
    pub fn poll<T>(
        &mut self,
        now: MonoTime,
        probe: impl FnOnce() -> io::Result<Option<T>>,
    ) -> Result<DiscoveryPoll<T>, DiscoveryError> {
        if self.phase != DiscoveryPhase::Waiting {
            return Err(DiscoveryError::Finished);
        }
        if now < self.last {
            self.phase = DiscoveryPhase::Failed;
            return Err(DiscoveryError::ClockBackwards);
        }
        self.last = now;
        if now >= self.deadline {
            self.phase = DiscoveryPhase::Failed;
            return Err(DiscoveryError::Expired);
        }
        if now < self.next_at {
            return Ok(DiscoveryPoll::Pending {
                next_at: self.next_at,
            });
        }
        match probe() {
            Ok(Some(node)) => {
                self.phase = DiscoveryPhase::Complete;
                Ok(DiscoveryPoll::Ready(node))
            }
            Ok(None) => {
                self.next_at = now
                    .checked_add(self.interval)
                    .unwrap_or(self.deadline)
                    .min(self.deadline);
                Ok(DiscoveryPoll::Pending {
                    next_at: self.next_at,
                })
            }
            Err(error) => {
                self.phase = DiscoveryPhase::Failed;
                Err(error.into())
            }
        }
    }
    pub fn poll_timed<T>(
        &mut self,
        clock: &mut crate::reactor::MonotonicClock,
        probe: impl FnOnce() -> io::Result<Option<T>>,
    ) -> Result<DiscoveryPoll<T>, DiscoveryError> {
        if self.phase != DiscoveryPhase::Waiting {
            return Err(DiscoveryError::Finished);
        }
        let now = clock.now().map_err(|error| {
            self.phase = DiscoveryPhase::Failed;
            DiscoveryError::Probe(error)
        })?;
        let result = self.poll(now, probe)?;
        let after = clock.now().map_err(|error| {
            self.phase = DiscoveryPhase::Failed;
            DiscoveryError::Probe(error)
        })?;
        self.last = after;
        if after >= self.deadline {
            self.phase = DiscoveryPhase::Failed;
            return Err(DiscoveryError::Expired);
        }
        Ok(result)
    }
    pub fn poll_uinput(
        &mut self,
        clock: &mut crate::reactor::MonotonicClock,
        output: &crate::uinput::UinputDevice,
    ) -> Result<DiscoveryPoll<crate::device::EvdevDevice>, DiscoveryError> {
        self.poll_timed(clock, || output.discover_output_node())
    }
}
pub(crate) struct EventCandidate {
    pub path: PathBuf,
    pub device_number: (u32, u32),
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn event_name(name: &str) -> bool {
    name.strip_prefix("event").is_some_and(|digits| {
        !digits.is_empty()
            && digits.len() <= 10
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && digits
                .parse::<u32>()
                .is_ok_and(|value| value.to_string() == digits)
    })
}
fn device_number(bytes: &[u8]) -> io::Result<(u32, u32)> {
    if bytes.is_empty() || bytes.len() > MAX_DEV_BYTES {
        return Err(invalid("invalid event dev attribute length"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("non-ASCII event dev attribute"))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let (major, minor) = text
        .split_once(':')
        .ok_or_else(|| invalid("invalid event dev attribute"))?;
    if major.is_empty()
        || minor.is_empty()
        || !major
            .bytes()
            .chain(minor.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid("invalid event dev digits"));
    }
    let major = major
        .parse::<u32>()
        .map_err(|_| invalid("event major overflow"))?;
    let minor = minor
        .parse::<u32>()
        .map_err(|_| invalid("event minor overflow"))?;
    if major != 13 {
        return Err(invalid("unexpected event major"));
    }
    Ok((major, minor))
}
pub(crate) fn locate_event(
    input_syspath: &Path,
    dev_root: &Path,
) -> io::Result<Option<EventCandidate>> {
    if !input_syspath.is_absolute() || fs::canonicalize(input_syspath)? != input_syspath {
        return Err(invalid("expected canonical owned input syspath"));
    }
    let mut found = None;
    for (index, entry) in fs::read_dir(input_syspath)?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(invalid("too many owned input directory entries"));
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|name| event_name(name)) else {
            continue;
        };
        if found.is_some() {
            return Err(invalid("ambiguous owned event nodes"));
        }
        found = Some(name.to_owned());
    }
    let Some(name) = found else {
        return Ok(None);
    };
    let event = input_syspath.join(&name);
    let canonical = match fs::canonicalize(&event) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if canonical.parent() != Some(input_syspath)
        || canonical.file_name() != Some(std::ffi::OsStr::new(&name))
    {
        return Err(invalid("event node escapes owned input syspath"));
    }
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(canonical.join("dev"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid("event dev attribute is not a regular file"));
    }
    let mut bytes = Vec::with_capacity(MAX_DEV_BYTES + 1);
    file.take((MAX_DEV_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(Some(EventCandidate {
        path: dev_root.join(name),
        device_number: device_number(&bytes)?,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        rc::Rc,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    const TICK: u64 = MIN_RETRY_NS;
    fn state() -> NodeDiscovery {
        NodeDiscovery::new(
            MonoTime(0),
            DiscoveryPolicy {
                timeout_ns: 5 * TICK,
                interval_ns: 2 * TICK,
            },
        )
        .unwrap()
    }
    #[test]
    fn discovery_retries_only_when_due_and_ready_is_consumed_once() {
        let mut discovery = state();
        assert_eq!(discovery.next_wakeup(), Some(MonoTime(0)));
        assert!(
            matches!(discovery.poll::<u32>(MonoTime(0),||Ok(None)).unwrap(),DiscoveryPoll::Pending{next_at:MonoTime(value)} if value==2*TICK)
        );
        assert!(matches!(
            discovery
                .poll::<u32>(MonoTime(TICK), || panic!("probe before retry deadline"))
                .unwrap(),
            DiscoveryPoll::Pending { .. }
        ));
        assert!(matches!(
            discovery.poll(MonoTime(2 * TICK), || Ok(Some(42))).unwrap(),
            DiscoveryPoll::Ready(42)
        ));
        assert_eq!(discovery.phase(), DiscoveryPhase::Complete);
        assert_eq!(discovery.next_wakeup(), None);
        assert!(matches!(
            discovery.poll::<u32>(MonoTime(3 * TICK), || panic!("duplicate probe")),
            Err(DiscoveryError::Finished)
        ));
    }
    #[test]
    fn expiration_wins_over_available_node_and_next_retry_is_clamped() {
        let mut discovery = state();
        assert!(
            matches!(discovery.poll::<u32>(MonoTime(4*TICK),||Ok(None)).unwrap(),DiscoveryPoll::Pending{next_at:MonoTime(value)} if value==5*TICK)
        );
        assert!(matches!(
            discovery.poll::<u32>(MonoTime(5 * TICK), || panic!(
                "late node must not be accessed"
            )),
            Err(DiscoveryError::Expired)
        ));
        assert_eq!(discovery.phase(), DiscoveryPhase::Failed);
        assert_eq!(discovery.next_wakeup(), None);
    }
    #[test]
    fn backwards_clock_and_probe_errors_are_terminal_and_preserve_errno() {
        let mut discovery = state();
        discovery.poll::<u32>(MonoTime(TICK), || Ok(None)).unwrap();
        assert!(matches!(
            discovery.poll::<u32>(MonoTime(0), || panic!("backwards clock must not probe")),
            Err(DiscoveryError::ClockBackwards)
        ));
        assert_eq!(discovery.phase(), DiscoveryPhase::Failed);
        let mut discovery = state();
        assert!(
            matches!(discovery.poll::<u32>(MonoTime(0),||Err(io::Error::from_raw_os_error(libc::EACCES))),Err(DiscoveryError::Probe(error)) if error.raw_os_error()==Some(libc::EACCES))
        );
        assert!(matches!(
            discovery.poll::<u32>(MonoTime(TICK), || panic!("failed probe cannot retry")),
            Err(DiscoveryError::Finished)
        ));
    }
    #[test]
    fn policy_and_overflow_are_rejected_before_any_probe() {
        for policy in [
            DiscoveryPolicy {
                timeout_ns: 0,
                interval_ns: TICK,
            },
            DiscoveryPolicy {
                timeout_ns: MAX_DISCOVERY_NS + 1,
                interval_ns: TICK,
            },
            DiscoveryPolicy {
                timeout_ns: TICK,
                interval_ns: 0,
            },
            DiscoveryPolicy {
                timeout_ns: TICK,
                interval_ns: TICK - 1,
            },
            DiscoveryPolicy {
                timeout_ns: TICK,
                interval_ns: 2 * TICK,
            },
        ] {
            assert!(matches!(
                NodeDiscovery::new(MonoTime(0), policy),
                Err(DiscoveryError::Policy)
            ));
        }
        assert!(matches!(
            NodeDiscovery::new(MonoTime(u64::MAX), DiscoveryPolicy::default()),
            Err(DiscoveryError::TimeOverflow)
        ));
    }
    #[test]
    fn timed_probe_never_returns_ready_after_real_deadline() {
        struct Node(Rc<Cell<usize>>);
        impl Drop for Node {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let calls = Cell::new(0);
        let mut clock = crate::reactor::MonotonicClock::new();
        let now = clock.now().unwrap();
        let mut discovery = NodeDiscovery::new(
            now,
            DiscoveryPolicy {
                timeout_ns: 10 * TICK,
                interval_ns: TICK,
            },
        )
        .unwrap();
        let result = discovery.poll_timed(&mut clock, || {
            calls.set(calls.get() + 1);
            std::thread::sleep(Duration::from_millis(20));
            Ok(Some(Node(drops.clone())))
        });
        assert!(matches!(result, Err(DiscoveryError::Expired)));
        assert_eq!(discovery.phase(), DiscoveryPhase::Failed);
        assert_eq!(drops.get(), calls.get()); // Includes pre-probe expiration under load.
    }
    #[test]
    fn strict_event_and_device_numbers_cover_dynamic_minors_without_guessing() {
        for name in ["event0", "event31", "event256", "event4294967295"] {
            assert!(event_name(name));
        }
        for name in [
            "event",
            "event01",
            "event-1",
            "event1x",
            "../event1",
            "event4294967296",
        ] {
            assert!(!event_name(name));
        }
        assert_eq!(device_number(b"13:64\n").unwrap(), (13, 64));
        assert_eq!(device_number(b"13:4294967295\n").unwrap(), (13, u32::MAX));
        for bytes in [
            b"".as_slice(),
            b"14:64\n",
            b"13:+1\n",
            b"13:-1",
            b" 13:64",
            b"13:1:2",
            b"13:1\n\n",
            b"13:4294967296",
            b"13:\xff",
            &[b'0'; 33],
        ] {
            assert!(device_number(bytes).is_err());
        }
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        root: PathBuf,
        input: PathBuf,
        dev: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "keyweave-discovery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let input = root.join("input999");
            let dev = root.join("dev");
            fs::create_dir(&input).unwrap();
            fs::create_dir(&dev).unwrap();
            Self { root, input, dev }
        }
        fn event(&self, name: &str, number: &[u8]) {
            let event = self.input.join(name);
            fs::create_dir(&event).unwrap();
            fs::write(event.join("dev"), number).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn filesystem_node_discovery_waits_for_attribute_then_selects_owned_path_only() {
        let fixture = Fixture::new();
        assert!(
            locate_event(&fixture.input, &fixture.dev)
                .unwrap()
                .is_none()
        );
        fs::create_dir(fixture.input.join("event7")).unwrap();
        assert!(
            locate_event(&fixture.input, &fixture.dev)
                .unwrap()
                .is_none()
        );
        fs::write(fixture.input.join("event7/dev"), b"13:71\n").unwrap();
        let candidate = locate_event(&fixture.input, &fixture.dev).unwrap().unwrap();
        assert_eq!(candidate.path, fixture.dev.join("event7"));
        assert_eq!(candidate.device_number, (13, 71));
        // No dev node is created/opened by locating the sysfs candidate.
        assert!(!candidate.path.exists());
    }
    #[test]
    fn filesystem_ambiguity_and_scan_capacity_fail_without_selecting_a_node() {
        let fixture = Fixture::new();
        fixture.event("event1", b"13:65\n");
        fixture.event("event2", b"13:66\n");
        assert_eq!(
            locate_event(&fixture.input, &fixture.dev)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let fixture = Fixture::new();
        for i in 0..=MAX_ENTRIES {
            fs::write(fixture.input.join(format!("metadata{i}")), b"").unwrap();
        }
        assert_eq!(
            locate_event(&fixture.input, &fixture.dev)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn filesystem_event_and_attribute_symlinks_cannot_escape_owned_input() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let other = fixture.root.join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("dev"), b"13:65\n").unwrap();
        symlink(&other, fixture.input.join("event1")).unwrap();
        assert!(locate_event(&fixture.input, &fixture.dev).is_err());
        let fixture = Fixture::new();
        fs::create_dir(fixture.input.join("event1")).unwrap();
        let other = fixture.root.join("other-dev");
        fs::write(&other, b"13:65\n").unwrap();
        symlink(&other, fixture.input.join("event1/dev")).unwrap();
        assert!(locate_event(&fixture.input, &fixture.dev).is_err());
    }
    #[test]
    fn filesystem_malformed_large_or_non_file_attributes_fail() {
        for bytes in [b"1:2\n".as_slice(), &[b'0'; 100]] {
            let fixture = Fixture::new();
            fixture.event("event1", bytes);
            assert_eq!(
                locate_event(&fixture.input, &fixture.dev)
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let fixture = Fixture::new();
        fs::create_dir(fixture.input.join("event1")).unwrap();
        fs::create_dir(fixture.input.join("event1/dev")).unwrap();
        assert_eq!(
            locate_event(&fixture.input, &fixture.dev)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
