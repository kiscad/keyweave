//! Bounded source polling and ack-bound baseline. No input/metadata IO inline.
use super::file_source::{Compared, FileSource, MAX_SOURCE_BYTES, SourceError, SourceStep};
use super::{DriveStatus, NativeSession};
use keyweave_progress::reload::Outcome;
use keyweave_types::{ConfigGeneration, MonoTime};
use std::{io, path::PathBuf, sync::Arc};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    Unchanged,
    Submitted,
    Accepted(ConfigGeneration),
    Rejected(ConfigGeneration),
    Cancelled(ConfigGeneration),
    ReadFailed(SourceError),
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchError {
    Source(SourceError),
    Interval,
    Baseline,
    Identity,
    Clock,
    Generation,
    Submit,
}
impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "configuration watcher failed: {self:?}")
    }
}
impl std::error::Error for WatchError {}
trait Source {
    fn request(&mut self, seen: Arc<[u8]>, active: Arc<[u8]>) -> Result<(), SourceError>;
    fn take_ready(&mut self) -> Option<Result<Compared, SourceError>>;
    fn notice(&mut self) -> bool {
        false
    }
    fn close(&mut self);
    fn reap(&mut self) -> Result<bool, SourceError>;
}
impl Source for FileSource {
    fn request(&mut self, seen: Arc<[u8]>, active: Arc<[u8]>) -> Result<(), SourceError> {
        self.request_compare(seen, active)
    }
    fn take_ready(&mut self) -> Option<Result<Compared, SourceError>> {
        if self.poll() == SourceStep::Ready {
            self.take().map(|r| r.and_then(|s| s.into_comparison()))
        } else {
            None
        }
    }
    fn notice(&mut self) -> bool {
        self.poll_notice()
    }
    fn close(&mut self) {
        FileSource::close(self)
    }
    fn reap(&mut self) -> Result<bool, SourceError> {
        self.reap_closed()
    }
}
trait Peer {
    fn drive(&mut self) -> DriveStatus;
    fn at(&self) -> MonoTime;
    fn run(&self) -> Option<u64>;
    fn ready(&self) -> bool;
    fn submit(&mut self, body: Box<[u8]>) -> io::Result<()>;
    fn outcome(&mut self) -> Option<Outcome>;
    fn generation(&self) -> Option<ConfigGeneration>;
    fn fail(&mut self, error: io::Error);
}
impl Peer for NativeSession {
    fn drive(&mut self) -> DriveStatus {
        self.poll_once()
    }
    fn at(&self) -> MonoTime {
        self.watcher_time()
    }
    fn run(&self) -> Option<u64> {
        self.watcher_run()
    }
    fn ready(&self) -> bool {
        self.watcher_ready()
    }
    fn submit(&mut self, body: Box<[u8]>) -> io::Result<()> {
        self.request_reload(body)
    }
    fn outcome(&mut self) -> Option<Outcome> {
        self.take_reload_outcome()
    }
    fn generation(&self) -> Option<ConfigGeneration> {
        self.observed_config_generation()
    }
    fn fail(&mut self, error: io::Error) {
        self.watcher_fail(error)
    }
}
type Candidate = (Box<[u8]>, Arc<[u8]>);
struct Scheduler<S: Source> {
    source: S,
    active: Arc<[u8]>,
    seen: Arc<[u8]>,
    candidate: Option<Candidate>,
    inflight: Option<Arc<[u8]>>,
    base: Option<ConfigGeneration>,
    run: Option<u64>,
    last: Option<MonoTime>,
    next: Option<MonoTime>,
    interval: u64,
    dirty_since: Option<MonoTime>,
    quiet_until: Option<MonoTime>,
    reading: bool,
    manual: bool,
    event: Option<WatchEvent>,
    closed: bool,
}
impl<S: Source> Scheduler<S> {
    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.source.close();
        self.candidate = None;
        self.inflight = None;
        self.event = Some(WatchEvent::Closed);
    }
    fn poll<P: Peer>(&mut self, peer: &mut P) -> Result<DriveStatus, WatchError> {
        if self.run.is_some_and(|old| peer.run() != Some(old)) {
            self.close();
            return Err(WatchError::Identity);
        }
        let status = peer.drive();
        if !matches!(
            status,
            DriveStatus::Pending(keyweave_linux::supervisor::SupervisorStatus::Running)
        ) {
            self.close();
            return Ok(status);
        }
        if self.closed {
            return Ok(status);
        }
        let result = self.advance(peer);
        if let Err(error) = result {
            self.close();
            if error != WatchError::Identity {
                peer.fail(io::Error::other(error));
            }
            return Err(error);
        }
        Ok(status)
    }
    fn advance<P: Peer>(&mut self, peer: &mut P) -> Result<(), WatchError> {
        let run = peer.run().ok_or(WatchError::Identity)?;
        if self.run.is_some_and(|old| old != run) {
            return Err(WatchError::Identity);
        }
        if self.run.is_none() && peer.generation() != Some(ConfigGeneration(1)) {
            return Err(WatchError::Baseline);
        }
        self.run = Some(run);
        let now = peer.at();
        if self.last.is_some_and(|last| now < last) {
            return Err(WatchError::Clock);
        }
        self.last = Some(now);
        if self.source.notice() {
            if self.dirty_since.is_none() {
                self.dirty_since = Some(now);
            }
            self.quiet_until = Some(now.checked_add(200_000_000).ok_or(WatchError::Clock)?);
        }
        // Confirmation processing never waits behind a user status notification.
        if self.inflight.is_some()
            && let Some(outcome) = peer.outcome()
        {
            let generation = peer.generation().ok_or(WatchError::Generation)?;
            let base = self.base.ok_or(WatchError::Generation)?;
            let expected = if outcome == Outcome::Accepted {
                ConfigGeneration(base.0.checked_add(1).ok_or(WatchError::Generation)?)
            } else {
                base
            };
            if generation != expected {
                return Err(WatchError::Generation);
            }
            if let Some(bytes) = self.inflight.take()
                && outcome == Outcome::Accepted
            {
                self.active = bytes;
            }
            self.base = None;
            self.event = Some(match outcome {
                Outcome::Accepted => WatchEvent::Accepted(generation),
                Outcome::Rejected => WatchEvent::Rejected(generation),
                Outcome::Cancelled => WatchEvent::Cancelled(generation),
            });
            self.next = Some(now.checked_add(self.interval).ok_or(WatchError::Clock)?);
        }
        if self.event.is_some() || self.inflight.is_some() {
            return Ok(());
        }
        if self.reading
            && let Some(result) = self.source.take_ready()
        {
            self.reading = false;
            match result {
                Ok(Compared::Seen) => self.event = Some(WatchEvent::Unchanged),
                Ok(Compared::Active) => {
                    self.seen = self.active.clone();
                    self.event = Some(WatchEvent::Unchanged)
                }
                Ok(Compared::Changed { body, retained }) => self.candidate = Some((body, retained)),
                Err(error) => {
                    if matches!(
                        error,
                        SourceError::Panicked
                            | SourceError::Spawn
                            | SourceError::Comparison
                            | SourceError::Closed
                            | SourceError::Busy
                    ) {
                        return Err(WatchError::Source(error));
                    }
                    self.event = Some(WatchEvent::ReadFailed(error));
                }
            }
            self.next = Some(now.checked_add(self.interval).ok_or(WatchError::Clock)?);
        }
        if self.event.is_some() {
            return Ok(());
        }
        if self.candidate.is_some() && peer.ready() {
            let base = peer.generation().ok_or(WatchError::Generation)?;
            if base.0 == 0 || base.0 == u64::MAX {
                return Err(WatchError::Generation);
            }
            let (body, retained) = self.candidate.take().ok_or(WatchError::Submit)?;
            peer.submit(body).map_err(|_| WatchError::Submit)?;
            self.seen = retained.clone();
            self.inflight = Some(retained);
            self.base = Some(base);
            self.event = Some(WatchEvent::Submitted);
            return Ok(());
        }
        if !self.reading
            && self.candidate.is_none()
            && peer.ready()
            && (self.manual
                || match (self.dirty_since, self.quiet_until) {
                    (Some(first), Some(quiet)) => {
                        now >= quiet
                            || now >= first.checked_add(1_000_000_000).ok_or(WatchError::Clock)?
                    }
                    _ => self.next.is_none_or(|next| now >= next),
                })
        {
            self.source
                .request(self.seen.clone(), self.active.clone())
                .map_err(WatchError::Source)?;
            self.reading = true;
            self.manual = false;
            self.dirty_since = None;
            self.quiet_until = None;
        }
        Ok(())
    }
}
/// Exclusively owns this session's reload source/results after first poll.
/// Construct cold with exact bytes used in the initial sealed generation1.
/// Consume notifications to permit further reads; use no other reload owner.
/// poll drives the bound NativeSession first, even after watcher close.
/// A different session is rejected before driving it.
/// close stops source work; it does not retract an already submitted reload.
pub struct ConfigWatcher {
    inner: Scheduler<FileSource>,
}
impl ConfigWatcher {
    pub fn new(path: PathBuf, initial: Arc<[u8]>, interval_ns: u64) -> Result<Self, WatchError> {
        if !(100_000_000..=60_000_000_000).contains(&interval_ns) {
            return Err(WatchError::Interval);
        }
        if initial.is_empty() || initial.len() > MAX_SOURCE_BYTES {
            return Err(WatchError::Baseline);
        }
        let source = FileSource::with_notifications(path).map_err(WatchError::Source)?;
        Ok(Self {
            inner: Scheduler {
                source,
                active: initial.clone(),
                seen: initial,
                candidate: None,
                inflight: None,
                base: None,
                run: None,
                last: None,
                next: None,
                interval: interval_ns,
                dirty_since: None,
                quiet_until: None,
                reading: false,
                manual: false,
                event: None,
                closed: false,
            },
        })
    }
    pub fn poll(&mut self, session: &mut NativeSession) -> Result<DriveStatus, WatchError> {
        self.inner.poll(session)
    }
    /// Coalesce manual requests into one flag; never extend an in-flight deadline.
    pub fn request_read(&mut self) -> Result<(), WatchError> {
        if self.inner.closed {
            return Err(WatchError::Source(SourceError::Closed));
        }
        self.inner.manual = true;
        Ok(())
    }
    /// Cached notification failure; periodic source reading still continues.
    pub fn notification_error(&self) -> Option<SourceError> {
        self.inner.source.notification_error()
    }
    pub fn take_event(&mut self) -> Option<WatchEvent> {
        self.inner.event.take()
    }
    pub fn close(&mut self) {
        self.inner.close()
    }
    pub fn reap_closed(&mut self) -> Result<bool, WatchError> {
        if !self.inner.closed {
            return Err(WatchError::Source(SourceError::Busy));
        }
        self.inner.source.reap().map_err(WatchError::Source)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use keyweave_linux::supervisor::SupervisorStatus;
    type Baselines = (Arc<[u8]>, Arc<[u8]>);
    #[derive(Default)]
    struct FakeSource {
        requests: Vec<Baselines>,
        result: Option<Result<Compared, SourceError>>,
        closed: bool,
        reaped: bool,
        hint: bool,
    }
    impl Source for FakeSource {
        fn request(&mut self, seen: Arc<[u8]>, active: Arc<[u8]>) -> Result<(), SourceError> {
            assert!(!self.closed);
            self.requests.push((seen, active));
            Ok(())
        }
        fn take_ready(&mut self) -> Option<Result<Compared, SourceError>> {
            self.result.take()
        }
        fn notice(&mut self) -> bool {
            std::mem::take(&mut self.hint)
        }
        fn close(&mut self) {
            self.closed = true;
        }
        fn reap(&mut self) -> Result<bool, SourceError> {
            Ok(self.reaped)
        }
    }
    struct FakePeer {
        status: DriveStatus,
        run: u64,
        now: MonoTime,
        ready: bool,
        generation: ConfigGeneration,
        drives: usize,
        submissions: Vec<Box<[u8]>>,
        outcome: Option<Outcome>,
        faults: usize,
    }
    impl Peer for FakePeer {
        fn drive(&mut self) -> DriveStatus {
            self.drives += 1;
            self.status
        }
        fn at(&self) -> MonoTime {
            self.now
        }
        fn run(&self) -> Option<u64> {
            Some(self.run)
        }
        fn ready(&self) -> bool {
            self.ready
        }
        fn submit(&mut self, body: Box<[u8]>) -> io::Result<()> {
            assert!(self.ready);
            self.submissions.push(body);
            self.ready = false;
            Ok(())
        }
        fn outcome(&mut self) -> Option<Outcome> {
            self.outcome.take()
        }
        fn generation(&self) -> Option<ConfigGeneration> {
            Some(self.generation)
        }
        fn fail(&mut self, _: io::Error) {
            self.faults += 1;
        }
    }
    fn setup() -> (Scheduler<FakeSource>, FakePeer) {
        let initial: Arc<[u8]> = Arc::from(b"initial".as_slice());
        (
            Scheduler {
                source: FakeSource::default(),
                active: initial.clone(),
                seen: initial,
                candidate: None,
                inflight: None,
                base: None,
                run: None,
                last: None,
                next: None,
                interval: 100_000_000,
                dirty_since: None,
                quiet_until: None,
                reading: false,
                manual: false,
                event: None,
                closed: false,
            },
            FakePeer {
                status: DriveStatus::Pending(SupervisorStatus::Running),
                run: 57,
                now: MonoTime(0),
                ready: true,
                generation: ConfigGeneration(1),
                drives: 0,
                submissions: Vec::new(),
                outcome: None,
                faults: 0,
            },
        )
    }
    fn changed(s: &mut Scheduler<FakeSource>, bytes: &[u8]) {
        s.source.result = Some(Ok(Compared::Changed {
            body: bytes.into(),
            retained: Arc::from(bytes),
        }));
    }
    #[test]
    fn interval_notifications_and_active_gate_bound_reads_without_blocking_drive() {
        let (mut s, mut p) = setup();
        p.ready = false;
        s.poll(&mut p).unwrap();
        assert!(s.source.requests.is_empty());
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        assert_eq!(s.event, Some(WatchEvent::Unchanged));
        p.now = MonoTime(99_999_999);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        s.event.take();
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        p.now = MonoTime(100_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        assert_eq!(p.drives, 6);
    }
    #[test]
    fn only_matching_accepted_confirmation_advances_active_baseline() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"candidate");
        s.poll(&mut p).unwrap();
        assert_eq!(s.event, Some(WatchEvent::Submitted));
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(p.submissions[0].as_ref(), b"candidate");
        s.manual = true;
        p.now = MonoTime(500_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        assert!(s.manual);
        // Accepted is processed even if Submitted was never consumed.
        p.outcome = Some(Outcome::Accepted);
        p.generation = ConfigGeneration(2);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.active.as_ref(), b"candidate");
        assert_eq!(
            s.event.take(),
            Some(WatchEvent::Accepted(ConfigGeneration(2)))
        );
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        assert!(!s.manual);
        assert_eq!(s.source.requests[1].1.as_ref(), b"candidate");
    }
    #[test]
    fn rejected_bytes_are_suppressed_and_revert_to_active_needs_no_transaction() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"invalid");
        s.poll(&mut p).unwrap();
        s.event.take();
        p.outcome = Some(Outcome::Rejected);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(s.seen.as_ref(), b"invalid");
        assert_eq!(
            s.event.take(),
            Some(WatchEvent::Rejected(ConfigGeneration(1)))
        );
        s.manual = true;
        s.poll(&mut p).unwrap();
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        assert_eq!(s.event.take(), Some(WatchEvent::Unchanged));
        s.manual = true;
        s.poll(&mut p).unwrap();
        s.source.result = Some(Ok(Compared::Active));
        s.poll(&mut p).unwrap();
        assert_eq!(s.seen.as_ref(), b"initial");
        assert_eq!(p.submissions.len(), 1);
        assert_eq!(p.generation, ConfigGeneration(1));
    }
    #[test]
    fn ordinary_read_failure_preserves_baseline_and_recovers_on_next_interval() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        let error = SourceError::Io {
            kind: io::ErrorKind::NotFound,
            errno: Some(2),
        };
        s.source.result = Some(Err(error));
        s.poll(&mut p).unwrap();
        assert_eq!(s.event.take(), Some(WatchEvent::ReadFailed(error)));
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(p.faults, 0);
        p.now = MonoTime(100_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        changed(&mut s, b"fixed");
        s.poll(&mut p).unwrap();
        assert_eq!(p.submissions[0].as_ref(), b"fixed");
    }
    #[test]
    fn stop_first_closes_source_and_discards_late_candidate_without_submission() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"late");
        p.status = DriveStatus::Pending(SupervisorStatus::ShutdownRequested {
            deadline: MonoTime(2_000_000_000),
            request_sent: false,
        });
        s.poll(&mut p).unwrap();
        assert!(s.closed && s.source.closed);
        assert!(p.submissions.is_empty());
        assert!(!s.source.reap().unwrap());
        s.source.reaped = true;
        assert!(s.source.reap().unwrap());
        s.event.take();
        s.poll(&mut p).unwrap();
        assert!(s.event.is_none());
        assert_eq!(p.drives, 3);
    }
    #[test]
    fn wrong_session_is_never_driven_or_stopped_and_bad_initial_baseline_fails() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        p.run = 58;
        assert_eq!(s.poll(&mut p), Err(WatchError::Identity));
        assert_eq!(p.drives, 1);
        assert_eq!(p.faults, 0);
        assert!(s.closed);
        let (mut s, mut p) = setup();
        p.generation = ConfigGeneration(2);
        assert_eq!(s.poll(&mut p), Err(WatchError::Baseline));
        assert_eq!(p.faults, 1);
        assert!(s.source.requests.is_empty());
    }
    #[test]
    fn backward_time_and_wrong_ack_generation_permanently_close_without_retry() {
        let (mut s, mut p) = setup();
        p.now = MonoTime(1);
        s.poll(&mut p).unwrap();
        p.now = MonoTime(0);
        assert_eq!(s.poll(&mut p), Err(WatchError::Clock));
        s.poll(&mut p).unwrap();
        assert_eq!(p.faults, 1);
        assert!(s.source.closed);
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"unknown");
        s.poll(&mut p).unwrap();
        p.outcome = Some(Outcome::Accepted);
        assert_eq!(s.poll(&mut p), Err(WatchError::Generation));
        assert_eq!(s.active.as_ref(), b"initial");
        assert!(s.closed);
        p.ready = true;
        p.generation = ConfigGeneration(2);
        s.poll(&mut p).unwrap();
        assert_eq!(p.submissions.len(), 1);
        assert_eq!(p.faults, 1);
    }
    #[test]
    fn fatal_source_failure_stops_same_owner_and_constructor_checks_are_cold() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        s.source.result = Some(Err(SourceError::Panicked));
        assert_eq!(
            s.poll(&mut p),
            Err(WatchError::Source(SourceError::Panicked))
        );
        assert_eq!(p.faults, 1);
        assert!(s.closed);
        assert!(matches!(
            ConfigWatcher::new(PathBuf::from("/missing"), Arc::from(b"a".as_slice()), 1),
            Err(WatchError::Interval)
        ));
        assert!(matches!(
            ConfigWatcher::new(
                PathBuf::from("/missing"),
                Arc::from(b"".as_slice()),
                100_000_000
            ),
            Err(WatchError::Baseline)
        ));
        let mut watcher = ConfigWatcher::new(
            PathBuf::from("/missing"),
            Arc::from(b"a".as_slice()),
            100_000_000,
        )
        .unwrap();
        assert!(!watcher.inner.source.job_pending());
        watcher.request_read().unwrap();
        watcher.request_read().unwrap();
        assert!(watcher.inner.manual);
        watcher.close();
        assert_eq!(
            watcher.request_read(),
            Err(WatchError::Source(SourceError::Closed))
        );
        assert!(watcher.reap_closed().unwrap());
    }
    #[test]
    fn cancelled_confirmation_is_distinct_and_does_not_advance_baseline() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"cancelled");
        s.poll(&mut p).unwrap();
        p.outcome = Some(Outcome::Cancelled);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(
            s.event.take(),
            Some(WatchEvent::Cancelled(ConfigGeneration(1)))
        );
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(s.seen.as_ref(), b"cancelled");
        assert!(s.inflight.is_none());
        assert_eq!(p.submissions.len(), 1);
    }
    #[test]
    fn directory_hints_wait_for_quiet_and_cannot_postpone_checks_forever() {
        let (mut s, mut p) = setup();
        s.interval = 60_000_000_000;
        s.poll(&mut p).unwrap();
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        s.event.take();
        p.now = MonoTime(1);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        p.now = MonoTime(200_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        p.now = MonoTime(200_000_001);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        s.event.take();
        p.now = MonoTime(300_000_000);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        for at in [
            400_000_000,
            600_000_000,
            800_000_000,
            1_000_000_000,
            1_200_000_000,
        ] {
            p.now = MonoTime(at);
            s.source.hint = true;
            s.poll(&mut p).unwrap();
            assert_eq!(s.source.requests.len(), 2);
        }
        p.now = MonoTime(1_300_000_000);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 3);
        assert!(s.dirty_since.is_none());
    }
    #[test]
    fn hints_during_busy_ack_and_notifications_coalesce_without_renewing_transaction() {
        let (mut s, mut p) = setup();
        s.interval = 60_000_000_000;
        s.poll(&mut p).unwrap();
        changed(&mut s, b"candidate");
        s.poll(&mut p).unwrap();
        p.now = MonoTime(1);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        assert_eq!(s.base, Some(ConfigGeneration(1)));
        p.now = MonoTime(200_000_001);
        p.generation = ConfigGeneration(2);
        p.outcome = Some(Outcome::Accepted);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        s.event.take();
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
    }
}
