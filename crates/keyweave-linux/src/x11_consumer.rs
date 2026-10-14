//! Native metadata observation only; not a capture grant or native-code mapping proof.
mod lease;
pub use lease::{NativeX11Lease, X11LeaseError};

use crate::{
    device::EvdevDevice,
    graphical_session::{
        NativeSessionProbe, SESSION_LIFETIME_NS, SessionObserver, SessionSnapshot,
    },
    uinput::UinputDevice,
    worker::WorkerParentGuard,
    x11_probe::{CoverageRequirements, MetadataCoverage, ProbeJob, ProbePhase, ProbeTarget},
};
use keyweave_types::MonoTime;
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct X11Observation {
    output: PathBuf,
    observed: MonoTime,
    until: MonoTime,
    coverage: MetadataCoverage,
}
impl X11Observation {
    #[cfg(test)]
    pub(crate) fn for_test(output: PathBuf, observed: MonoTime, until: MonoTime) -> Self {
        Self {
            output,
            observed,
            until,
            coverage: MetadataCoverage {
                keyboard: None,
                pointer: None,
            },
        }
    }
    pub fn fresh_for(&self, output: &Path, now: MonoTime) -> bool {
        self.output == output && self.observed <= now && now < self.until
    }
    pub fn valid_until(&self) -> MonoTime {
        self.until
    }
    pub fn coverage(&self) -> &MetadataCoverage {
        &self.coverage
    }
}
struct Pending {
    job: ProbeJob,
    target: ProbeTarget,
    output: PathBuf,
    first: SessionSnapshot,
    observed: MonoTime,
    until: MonoTime,
    required: CoverageRequirements,
    canceled: bool,
}
fn reap_canceled(pending: &mut Pending, last: MonoTime) -> io::Result<ProbePhase> {
    if !pending.canceled {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    pending.job.poll(last)
}
pub struct NativeX11Observer {
    session: SessionObserver<NativeSessionProbe>,
    parent: u32,
    helper: PathBuf,
    pending: Option<Pending>,
    observation: Option<X11Observation>,
    last: Option<MonoTime>,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn enabled(output: &Path) -> io::Result<bool> {
    let mut bytes = Vec::new();
    File::open(output.join("inhibited"))?
        .take(9)
        .read_to_end(&mut bytes)?;
    Ok(bytes == b"0\n")
}
fn target_for(output: &UinputDevice, node: &EvdevDevice, display: &str) -> io::Result<ProbeTarget> {
    output
        .verify_output_node(node)
        .map_err(|_| invalid("owned output node not ready"))?;
    let candidate =
        crate::discovery::locate_event(output.input_syspath(), Path::new("/dev/input"))?
            .ok_or_else(|| invalid("owned output event missing"))?;
    if candidate.device_number != node.device_number() {
        return Err(invalid("owned node number changed"));
    }
    ProbeTarget::new(
        display,
        &candidate.path,
        candidate.device_number.0,
        candidate.device_number.1,
    )
}
fn consistent(
    first: &SessionSnapshot,
    last: &SessionSnapshot,
    start: MonoTime,
    until: MonoTime,
    now: MonoTime,
) -> bool {
    first.kind == "x11" && first == last && start <= now && now < until
}
impl NativeX11Observer {
    /// Host authenticates the explicit helper image; this constructor never repairs guards.
    pub fn new(guard: &WorkerParentGuard, helper: &Path) -> io::Result<Self> {
        let probe = NativeSessionProbe::for_parent(guard)?;
        let subject = probe.subject();
        let session =
            SessionObserver::new(probe, subject).map_err(|_| invalid("invalid session subject"))?;
        Ok(Self {
            session,
            parent: guard.parent_pid(),
            helper: helper.into(),
            pending: None,
            observation: None,
            last: None,
        })
    }
    fn now(&mut self, clock: &mut impl FnMut() -> io::Result<MonoTime>) -> io::Result<MonoTime> {
        let now = clock()?;
        if self.last.is_some_and(|last| now < last) {
            return Err(invalid("observer clock regressed"));
        }
        self.last = Some(now);
        Ok(now)
    }
    pub fn current(&self, output: &Path, now: MonoTime) -> Option<&X11Observation> {
        self.observation
            .as_ref()
            .filter(|value| value.fresh_for(output, now))
    }
    pub fn take_observation(&mut self) -> Option<X11Observation> {
        self.observation.take()
    }
    pub(crate) fn job_failed(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|value| value.job.failure().is_some())
    }
    pub fn job_phase(&self) -> Option<ProbePhase> {
        self.pending.as_ref().map(|value| value.job.phase())
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.pending
            .as_ref()
            .and_then(|value| value.job.next_deadline())
            .or_else(|| self.observation.as_ref().map(|value| value.until))
    }
    pub fn cancel(&mut self) -> io::Result<()> {
        self.observation = None;
        if let Some(pending) = &mut self.pending {
            pending.canceled = true;
            pending.job.request_stop()?;
        }
        Ok(())
    }
    /// Reap an already-canceled job even if the observer's clock source failed.
    /// Saved time is used only after result revocation; no new evidence can be minted.
    pub fn poll_canceled(&mut self) -> io::Result<Option<ProbePhase>> {
        let Some(pending) = &mut self.pending else {
            return Ok(None);
        };
        let phase = reap_canceled(
            pending,
            self.last.ok_or_else(|| invalid("missing saved clock"))?,
        )?;
        if phase == ProbePhase::Exited {
            self.pending = None;
        }
        Ok(Some(phase))
    }
    pub fn begin(
        &mut self,
        output: &UinputDevice,
        node: &EvdevDevice,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> io::Result<bool> {
        self.observation = None;
        if self.pending.is_some() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        WorkerParentGuard::verify(self.parent)?.verify_isolated_group()?;
        let required = CoverageRequirements::for_output(output.spec())?;
        let observed = self.now(clock)?;
        let until = observed
            .checked_add(SESSION_LIFETIME_NS)
            .ok_or_else(|| invalid("observer deadline overflow"))?;
        if !enabled(output.input_syspath())? {
            return Ok(false);
        }
        let Some(session) = self
            .session
            .observe(clock)
            .map_err(|_| invalid("session observation failed"))?
        else {
            return Ok(false);
        };
        let first = session.snapshot().clone();
        if first.kind != "x11" {
            return Ok(false);
        }
        let target = target_for(
            output,
            node,
            first
                .display
                .as_deref()
                .ok_or_else(|| invalid("X11 display missing"))?,
        )?;
        let now = self.now(clock)?;
        if now >= until {
            return Ok(false);
        }
        let job = ProbeJob::spawn(&self.helper, target.clone(), now, until.0 - now.0)?;
        self.pending = Some(Pending {
            job,
            target,
            output: output.input_syspath().into(),
            first,
            observed,
            until,
            required,
            canceled: false,
        });
        Ok(true)
    }
    /// Retains a stopped child until actual exit. Errors revoke evidence and request stop.
    pub fn poll(
        &mut self,
        output: &UinputDevice,
        node: &EvdevDevice,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> io::Result<()> {
        let result = self.poll_inner(output, node, clock);
        if result.is_err() {
            let _ = self.cancel();
        }
        result
    }
    fn poll_inner(
        &mut self,
        output: &UinputDevice,
        node: &EvdevDevice,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> io::Result<()> {
        let now = self.now(clock)?;
        if self
            .observation
            .as_ref()
            .is_some_and(|value| !value.fresh_for(output.input_syspath(), now))
        {
            self.observation = None;
        }
        let Some(pending) = &mut self.pending else {
            return Ok(());
        };
        // Observe child even after cancellation; failed output validation must not prevent reaping.
        let phase = pending.job.poll(now)?;
        if phase != ProbePhase::Exited {
            return Ok(());
        }
        let Some(mut pending) = self.pending.take() else {
            return Err(invalid("probe owner missing"));
        };
        self.observation = None;
        if pending.canceled || now >= pending.until {
            return Ok(());
        }
        let Some(report) = pending.job.take_report() else {
            return Ok(());
        };
        WorkerParentGuard::verify(self.parent)?.verify_isolated_group()?;
        if output.input_syspath() != pending.output || !enabled(output.input_syspath())? {
            return Ok(());
        }
        let target = target_for(
            output,
            node,
            pending
                .first
                .display
                .as_deref()
                .ok_or_else(|| invalid("X11 display missing"))?,
        )?;
        if target != pending.target {
            return Ok(());
        }
        let Some(session) = self
            .session
            .observe(clock)
            .map_err(|_| invalid("session recheck failed"))?
        else {
            return Ok(());
        };
        let last = session.snapshot().clone();
        let covered = report.check_coverage(&pending.target, &pending.required)?;
        let still_enabled = enabled(output.input_syspath())?;
        let after = self.now(clock)?;
        if still_enabled
            && consistent(
                &pending.first,
                &last,
                pending.observed,
                pending.until,
                after,
            )
        {
            self.observation = Some(X11Observation {
                output: pending.output,
                observed: pending.observed,
                until: pending.until,
                coverage: covered,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> SessionSnapshot {
        SessionSnapshot {
            parent_session: "c2".into(),
            worker_session: "c2".into(),
            uid: 1000,
            active: true,
            remote: false,
            class: "user".into(),
            kind: "x11".into(),
            seat: "seat0".into(),
            display: Some(":0".into()),
            state: "active".into(),
        }
    }
    #[test]
    fn observation_lifetime_is_half_open_and_bound_to_output() {
        let output = crate::mock_syspath("/sys/devices/virtual/input/input8");
        let observation = X11Observation {
            output: output.clone(),
            observed: MonoTime(10),
            until: MonoTime(20),
            coverage: MetadataCoverage {
                keyboard: None,
                pointer: None,
            },
        };
        assert!(!observation.fresh_for(&output, MonoTime(9)));
        assert!(observation.fresh_for(&output, MonoTime(10)));
        assert!(!observation.fresh_for(&output, MonoTime(20)));
        assert!(!observation.fresh_for(&crate::mock_syspath("/sys/other"), MonoTime(15)));
    }
    #[test]
    fn completion_cannot_extend_start_deadline() {
        let first = snapshot();
        assert!(consistent(
            &first,
            &first,
            MonoTime(10),
            MonoTime(20),
            MonoTime(19)
        ));
        assert!(!consistent(
            &first,
            &first,
            MonoTime(10),
            MonoTime(20),
            MonoTime(20)
        ));
        assert!(!consistent(
            &first,
            &first,
            MonoTime(10),
            MonoTime(20),
            MonoTime(9)
        ));
    }
    #[test]
    fn session_change_and_wayland_cannot_complete_x11_evidence() {
        let first = snapshot();
        for change in 0..4 {
            let mut last = first.clone();
            match change {
                0 => last.display = Some(":1".into()),
                1 => last.active = false,
                2 => last.seat = "seat1".into(),
                _ => last.parent_session = "c3".into(),
            }
            assert!(!consistent(
                &first,
                &last,
                MonoTime(10),
                MonoTime(20),
                MonoTime(15)
            ));
        }
        let mut wayland = first;
        wayland.kind = "wayland".into();
        wayland.display = None;
        assert!(!consistent(
            &wayland,
            &wayland,
            MonoTime(10),
            MonoTime(20),
            MonoTime(15)
        ));
    }
    #[test]
    fn canceled_owned_job_can_be_reaped_without_a_live_clock_or_devices() -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("keyweave-x11-consumer-reap-{}", std::process::id()));
        std::fs::create_dir(&directory)?;
        let image = directory.join("helper");
        std::fs::write(&image, "#!/usr/bin/python3\nimport time\ntime.sleep(10)\n")?;
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o700))?;
        let target = ProbeTarget::new(":0", Path::new("/dev/input/event42"), 13, 106)?;
        let job = ProbeJob::spawn(&image, target.clone(), MonoTime(10), SESSION_LIFETIME_NS)?;
        let mut pending = Pending {
            job,
            target,
            output: directory.clone(),
            first: snapshot(),
            observed: MonoTime(10),
            until: MonoTime(20),
            required: CoverageRequirements::new(&[38], &[])?,
            canceled: false,
        };
        assert_eq!(
            reap_canceled(&mut pending, MonoTime(10))
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::WouldBlock)
        );
        pending.canceled = true;
        pending.job.request_stop()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while reap_canceled(&mut pending, MonoTime(10))? != ProbePhase::Exited {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(pending.job.exit_status().is_some());
        assert!(pending.job.take_report().is_none());
        drop(pending);
        std::fs::remove_dir_all(directory)?;
        Ok(())
    }
}
