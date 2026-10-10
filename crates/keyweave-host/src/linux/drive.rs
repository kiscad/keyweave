//! Native control pump. Observation expiry retains the same owned child.
use super::{Controller, Fault, FaultSource, Owner, Report, Session};
use keyweave_linux::{
    host_signals::{HostSignals, StopSignal},
    reactor::MonotonicClock,
    supervisor::SupervisorStatus,
    termination::bounded::SystemFailure,
};
use keyweave_types::MonoTime;
use std::{
    io,
    time::{Duration, Instant},
};
const INTERVAL: Duration = Duration::from_millis(5);
const OBSERVATION_LIMIT: Duration = Duration::from_secs(5);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeReport {
    pub worker: Report,
    pub signal_close_failure: Option<SystemFailure>,
}
impl NativeReport {
    pub fn successful(&self) -> bool {
        self.signal_close_failure.is_none() && self.worker.successful()
    }
}
impl std::fmt::Display for NativeReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use keyweave_linux::termination::ReadStatus;
        use std::os::unix::process::ExitStatusExt;
        let (terminal, facts) = match self.worker.terminal.status {
            ReadStatus::Complete(frame) => ("complete", Some(frame.report)),
            ReadStatus::Unsupported => ("unsupported", None),
            ReadStatus::NotExited => ("not_exited", None),
            ReadStatus::Pending => ("pending", None),
            ReadStatus::Missing => ("missing", None),
            ReadStatus::Invalid(_) => ("invalid", None),
            ReadStatus::Io(_) => ("io_error", None),
        };
        // Never format TerminalFrame itself: it contains the private run ID.
        write!(
            f,
            "worker_exit={:?}, worker_signal={:?}, reason={:?}, stop_signal={:?}, progress_epoch={}, terminal={}, terminal_failure={:?}, recovery_confirmed={}, recovery_facts={:?}, host_fault={:?}, signal_close_failure={:?}",
            self.worker.status.code(),
            self.worker.status.signal(),
            self.worker.reason,
            self.worker.signal,
            self.worker.progress_epoch,
            terminal,
            self.worker.terminal.failure,
            self.worker.recovery_confirmed(),
            facts,
            self.worker.fault,
            self.signal_close_failure
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveStatus {
    Pending(SupervisorStatus),
    Exited(NativeReport),
    ObservationExpired,
}
trait Schedule {
    fn now(&mut self) -> io::Result<MonoTime>;
    fn elapsed(&self) -> Duration;
    fn receive(&mut self) -> io::Result<Option<StopSignal>>;
    fn wait(&mut self, recovery: bool) -> io::Result<()>;
    fn close(&mut self) -> io::Result<()>;
}
struct NativeSchedule {
    signals: Option<HostSignals>,
    clock: MonotonicClock,
    start: Instant,
}
impl Schedule for NativeSchedule {
    fn now(&mut self) -> io::Result<MonoTime> {
        self.clock.now()
    }
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
    fn receive(&mut self) -> io::Result<Option<StopSignal>> {
        self.signals
            .as_mut()
            .ok_or(io::ErrorKind::NotConnected)?
            .poll()
    }
    fn wait(&mut self, recovery: bool) -> io::Result<()> {
        if recovery {
            std::thread::sleep(INTERVAL);
            Ok(())
        } else {
            self.signals
                .as_ref()
                .ok_or(io::ErrorKind::NotConnected)?
                .wait(INTERVAL)
        }
    }
    fn close(&mut self) -> io::Result<()> {
        self.signals
            .take()
            .ok_or(io::ErrorKind::NotConnected)?
            .close()
    }
}
struct Pump<O: Owner, S: Schedule> {
    // On emergency Drop, owned child cleanup precedes restoring signal mask.
    controller: Controller<O>,
    schedule: S,
    last: MonoTime,
    observation_start: Option<Duration>,
    finished: Option<NativeReport>,
}
impl<O: Owner, S: Schedule> Pump<O, S> {
    fn tick(&mut self) -> DriveStatus {
        if let Some(report) = self.finished {
            return DriveStatus::Exited(report);
        }
        let elapsed = self.schedule.elapsed();
        let now = match self.schedule.now() {
            Ok(now) if now >= self.last => {
                self.last = now;
                now
            }
            Ok(_) => {
                self.controller.fail(
                    FaultSource::Control,
                    io::Error::new(io::ErrorKind::InvalidData, "host clock moved backwards"),
                );
                self.last
            }
            Err(e) => {
                self.controller.fail(FaultSource::Control, e);
                self.last
            }
        };
        let signal = match self.schedule.receive() {
            Ok(signal) => signal,
            Err(e) => {
                self.controller.fail(FaultSource::Control, e);
                None
            }
        };
        let status = match self.controller.poll(now, signal) {
            super::State::Pending(status) => status,
            super::State::Exited(worker) => {
                let report = NativeReport {
                    worker,
                    signal_close_failure: self.schedule.close().err().map(Into::into),
                };
                self.finished = Some(report);
                return DriveStatus::Exited(report);
            }
        };
        if self.controller.fault.is_some() || !matches!(status, SupervisorStatus::Running) {
            let start = *self.observation_start.get_or_insert(elapsed);
            if elapsed
                .checked_sub(start)
                .is_none_or(|delta| delta >= OBSERVATION_LIMIT)
            {
                // This is not exit. Never collect terminal data or drop/restart
                // the owner just because the bounded observation expired.
                return DriveStatus::ObservationExpired;
            }
        }
        if let Err(e) = self.schedule.wait(self.controller.fault.is_some()) {
            self.controller.fail(FaultSource::Control, e);
            self.observation_start.get_or_insert(elapsed);
        }
        DriveStatus::Pending(self.controller.owner.status())
    }
}
/// Main-thread owned pump. Clock must already have a spawn-time observation in
/// the same domain passed to Supervisor. Install signals and prepare_child
/// before isolated spawn; no native input operations occur in this pump.
/// Active operation has no arbitrary lifetime limit. After stopping/fault,
/// bounded observation is five seconds; expiry retains this same owner.
pub struct NativeSession {
    pump: Pump<keyweave_linux::supervisor::Supervisor, NativeSchedule>,
}
impl NativeSession {
    pub fn new(session: Session, signals: HostSignals, clock: MonotonicClock) -> Self {
        let mut controller = session.controller;
        let last = if let Some(last) = clock.last() {
            last
        } else {
            controller.fail(
                FaultSource::Control,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "missing spawn clock observation",
                ),
            );
            MonoTime(0)
        };
        Self {
            pump: Pump {
                controller,
                schedule: NativeSchedule {
                    signals: Some(signals),
                    clock,
                    start: Instant::now(),
                },
                last,
                observation_start: None,
                finished: None,
            },
        }
    }
    /// One bounded iteration; actual exit is checked before observation expiry.
    /// After expiry callers may reobserve the same owner; budget is not reset.
    pub fn poll_once(&mut self) -> DriveStatus {
        self.pump.tick()
    }
    pub fn run(&mut self) -> DriveStatus {
        loop {
            match self.poll_once() {
                DriveStatus::Pending(_) => {}
                done => return done,
            }
        }
    }
    pub fn status(&self) -> SupervisorStatus {
        self.pump.controller.owner.status()
    }
    pub fn fault(&self) -> Option<Fault> {
        self.pump.controller.fault
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use keyweave_linux::{
        supervisor::TerminationReason,
        termination::{ReadStatus, bounded::ObservationReport},
    };
    use std::os::unix::process::ExitStatusExt;
    struct MockOwner {
        status: SupervisorStatus,
        stops: usize,
        shutdowns: usize,
        terminal_reads: usize,
        times: Vec<MonoTime>,
    }
    impl Owner for MockOwner {
        fn shutdown(&mut self, _: MonoTime) -> io::Result<SupervisorStatus> {
            self.shutdowns += 1;
            self.status = SupervisorStatus::ShutdownRequested {
                deadline: MonoTime(2_000_000_001),
                request_sent: true,
            };
            Ok(self.status)
        }
        fn stop(&mut self) -> io::Result<()> {
            self.stops += 1;
            self.status = SupervisorStatus::KillRequested(TerminationReason::Requested);
            Ok(())
        }
        fn poll(&mut self, now: MonoTime) -> io::Result<SupervisorStatus> {
            self.times.push(now);
            Ok(self.status)
        }
        fn status(&self) -> SupervisorStatus {
            self.status
        }
        fn terminal(&mut self) -> ObservationReport {
            self.terminal_reads += 1;
            ObservationReport {
                status: ReadStatus::Missing,
                attempts: 1,
                failure: None,
            }
        }
        fn epoch(&self) -> u64 {
            0
        }
    }
    struct MockSchedule {
        now: MonoTime,
        elapsed: Duration,
        signal: Option<StopSignal>,
        clock_error: bool,
        receive_error: bool,
        wait_error: bool,
        close_error: bool,
        waits: usize,
        closes: usize,
        recovery_wait: bool,
    }
    impl Schedule for MockSchedule {
        fn now(&mut self) -> io::Result<MonoTime> {
            if self.clock_error {
                Err(io::ErrorKind::Other.into())
            } else {
                Ok(self.now)
            }
        }
        fn elapsed(&self) -> Duration {
            self.elapsed
        }
        fn receive(&mut self) -> io::Result<Option<StopSignal>> {
            if self.receive_error {
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(self.signal.take())
            }
        }
        fn wait(&mut self, recovery: bool) -> io::Result<()> {
            self.waits += 1;
            self.recovery_wait = recovery;
            if self.wait_error {
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            }
        }
        fn close(&mut self) -> io::Result<()> {
            self.closes += 1;
            if self.close_error {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        }
    }
    fn pump() -> Pump<MockOwner, MockSchedule> {
        Pump {
            controller: Controller::new(MockOwner {
                status: SupervisorStatus::Running,
                stops: 0,
                shutdowns: 0,
                terminal_reads: 0,
                times: vec![],
            }),
            schedule: MockSchedule {
                now: MonoTime(1),
                elapsed: Duration::ZERO,
                signal: None,
                clock_error: false,
                receive_error: false,
                wait_error: false,
                close_error: false,
                waits: 0,
                closes: 0,
                recovery_wait: false,
            },
            last: MonoTime(0),
            observation_start: None,
            finished: None,
        }
    }
    #[test]
    fn active_lifetime_is_unbounded_and_does_not_invent_progress() {
        let mut p = pump();
        p.schedule.elapsed = Duration::from_secs(100);
        assert!(matches!(
            p.tick(),
            DriveStatus::Pending(SupervisorStatus::Running)
        ));
        assert_eq!(p.observation_start, None);
        assert_eq!(p.controller.owner.terminal_reads, 0);
    }
    #[test]
    fn expiry_retains_owner_and_later_actual_exit_can_be_observed_without_reset() {
        let mut p = pump();
        p.schedule.signal = Some(StopSignal::Interrupt);
        p.tick();
        p.schedule.elapsed = OBSERVATION_LIMIT;
        assert_eq!(p.tick(), DriveStatus::ObservationExpired);
        assert_eq!(p.controller.owner.terminal_reads, 0);
        assert_eq!(p.schedule.closes, 0);
        p.schedule.signal = Some(StopSignal::Terminate);
        assert_eq!(p.tick(), DriveStatus::ObservationExpired);
        assert_eq!(p.controller.owner.shutdowns, 1);
        p.controller.owner.status = SupervisorStatus::Exited {
            status: std::process::ExitStatus::from_raw(0),
            reason: Some(TerminationReason::Cooperative),
        };
        let DriveStatus::Exited(report) = p.tick() else {
            panic!("actual exit missing");
        };
        assert_eq!(report.worker.signal, Some(StopSignal::Interrupt));
        assert!(!report.successful());
        assert_eq!(p.tick(), DriveStatus::Exited(report));
        assert_eq!(p.schedule.closes, 1);
        assert_eq!(p.controller.owner.terminal_reads, 1);
    }
    #[test]
    fn clock_fault_uses_last_accepted_time_and_forces_owned_stop() {
        let mut p = pump();
        p.tick();
        p.schedule.clock_error = true;
        p.schedule.now = MonoTime(999);
        p.tick();
        assert_eq!(p.controller.owner.times, vec![MonoTime(1), MonoTime(1)]);
        assert_eq!(p.controller.owner.stops, 1);
        assert!(p.schedule.recovery_wait);
        assert!(p.controller.fault.is_some());
        assert_eq!(p.controller.owner.terminal_reads, 0);
    }
    #[test]
    fn backwards_clock_never_reaches_supervisor() {
        let mut p = pump();
        p.tick();
        p.schedule.now = MonoTime(0);
        p.tick();
        assert_eq!(p.controller.owner.times, vec![MonoTime(1), MonoTime(1)]);
        assert_eq!(
            p.controller.fault.unwrap().primary.kind,
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn receive_and_wait_faults_start_finite_observation_and_recovery_wait() {
        for receive in [false, true] {
            let mut p = pump();
            p.schedule.receive_error = receive;
            p.schedule.wait_error = !receive;
            p.tick();
            assert_eq!(p.observation_start, Some(Duration::ZERO));
            assert_eq!(p.controller.owner.stops, 1);
            p.schedule.wait_error = false;
            p.tick();
            assert!(p.schedule.recovery_wait);
            p.schedule.elapsed = OBSERVATION_LIMIT;
            assert_eq!(p.tick(), DriveStatus::ObservationExpired);
            assert_eq!(p.controller.owner.terminal_reads, 0);
        }
    }
    #[test]
    fn signal_close_failure_is_independent_of_actual_exit_and_is_cached() {
        let mut p = pump();
        p.schedule.close_error = true;
        p.controller.owner.status = SupervisorStatus::Exited {
            status: std::process::ExitStatus::from_raw(0),
            reason: None,
        };
        let DriveStatus::Exited(report) = p.tick() else {
            panic!("missing exit");
        };
        assert_eq!(
            report.signal_close_failure.unwrap().kind,
            io::ErrorKind::PermissionDenied
        );
        assert!(!report.successful());
        assert_eq!(p.tick(), DriveStatus::Exited(report));
        assert_eq!(p.schedule.closes, 1);
    }
    #[test]
    fn display_reports_recovery_facts_without_private_run_identity() {
        use keyweave_linux::{
            runtime::{StopReason, TerminationReport},
            termination::TerminalFrame,
        };
        let mut p = pump();
        p.controller.owner.status = SupervisorStatus::Exited {
            status: std::process::ExitStatus::from_raw(78 << 8),
            reason: None,
        };
        let DriveStatus::Exited(mut report) = p.tick() else {
            panic!("missing exit");
        };
        report.worker.terminal.status = ReadStatus::Complete(TerminalFrame {
            run_id: u64::MAX,
            report: TerminationReport {
                reason: StopReason::ControlFault,
                input_owners_released: true,
                engine_reset_confirmed: true,
                recovery_failed: false,
                recovery_budget_exhausted: false,
                submitted_remaining: false,
                report_pending: false,
                acceptance_unknown: false,
                output_recovery_confirmed: true,
                output_owner_dropped: true,
                backend_transition_confirmed: true,
            },
        });
        let text = report.to_string();
        assert!(text.contains("worker_exit=Some(78)"));
        assert!(text.contains("terminal=complete"));
        assert!(text.contains("ControlFault"));
        assert!(!text.contains(&u64::MAX.to_string()));
        assert!(!text.contains("run_id"));
    }
}
