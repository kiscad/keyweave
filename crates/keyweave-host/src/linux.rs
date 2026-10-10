//! One owned Linux worker; injected control events never act as progress.
mod drive;
pub use drive::{DriveStatus, NativeReport, NativeSession};
use keyweave_linux::{
    host_signals::StopSignal,
    runtime::StopReason,
    supervisor::{Supervisor, SupervisorStatus, TerminationReason},
    termination::{
        ReadStatus,
        bounded::{ObservationReport, SystemFailure},
    },
};
use keyweave_types::MonoTime;
use std::{io, process::ExitStatus};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultSource {
    Control,
    Supervision,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub source: FaultSource,
    pub primary: SystemFailure,
    pub stop_failure: Option<SystemFailure>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub status: ExitStatus,
    pub reason: Option<TerminationReason>,
    pub signal: Option<StopSignal>,
    pub fault: Option<Fault>,
    pub terminal: ObservationReport,
    pub progress_epoch: u64,
}
impl Report {
    /// Worker-reported owner/recovery facts, independent of process exit. Does
    /// not certify global keyboard state, hardware acceptance or driver health.
    pub fn recovery_confirmed(&self) -> bool {
        if self.terminal.failure.is_some() {
            return false;
        }
        let ReadStatus::Complete(frame) = self.terminal.status else {
            return false;
        };
        let r = frame.report;
        r.input_owners_released
            && r.engine_reset_confirmed
            && !r.recovery_failed
            && !r.recovery_budget_exhausted
            && !r.submitted_remaining
            && !r.report_pending
            && !r.acceptance_unknown
            && r.output_recovery_confirmed
            && r.output_owner_dropped
            && r.backend_transition_confirmed
    }
    pub fn successful(&self) -> bool {
        let ReadStatus::Complete(frame) = self.terminal.status else {
            return false;
        };
        self.status.code() == Some(0)
            && self.fault.is_none()
            && matches!(self.reason, None | Some(TerminationReason::Cooperative))
            && matches!(
                frame.report.reason,
                StopReason::Pause | StopReason::Emergency
            )
            && self.recovery_confirmed()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Pending(SupervisorStatus),
    Exited(Report),
}
trait Owner {
    fn shutdown(&mut self, now: MonoTime) -> io::Result<SupervisorStatus>;
    fn stop(&mut self) -> io::Result<()>;
    fn poll(&mut self, now: MonoTime) -> io::Result<SupervisorStatus>;
    fn status(&self) -> SupervisorStatus;
    fn terminal(&mut self) -> ObservationReport;
    fn epoch(&self) -> u64;
}
impl Owner for Supervisor {
    fn shutdown(&mut self, now: MonoTime) -> io::Result<SupervisorStatus> {
        self.request_shutdown(now)
    }
    fn stop(&mut self) -> io::Result<()> {
        self.request_stop()
    }
    fn poll(&mut self, now: MonoTime) -> io::Result<SupervisorStatus> {
        self.poll(now)
    }
    fn status(&self) -> SupervisorStatus {
        self.status()
    }
    fn terminal(&mut self) -> ObservationReport {
        self.collect_terminal()
    }
    fn epoch(&self) -> u64 {
        self.progress_epoch()
    }
}
struct Controller<O: Owner> {
    owner: O,
    signal: Option<StopSignal>,
    fault: Option<Fault>,
    report: Option<Report>,
}
impl<O: Owner> Controller<O> {
    fn new(owner: O) -> Self {
        Self {
            owner,
            signal: None,
            fault: None,
            report: None,
        }
    }
    fn fail(&mut self, source: FaultSource, error: io::Error) {
        if self.report.is_some() || self.fault.is_some() {
            return;
        }
        self.fault = Some(Fault {
            source,
            primary: error.into(),
            stop_failure: self.owner.stop().err().map(Into::into),
        });
    }
    fn poll(&mut self, now: MonoTime, signal: Option<StopSignal>) -> State {
        if let Some(report) = self.report {
            return State::Exited(report);
        }
        if self.fault.is_none()
            && self.signal.is_none()
            && let Some(signal) = signal
        {
            self.signal = Some(signal);
            if let Err(e) = self.owner.shutdown(now) {
                self.fail(FaultSource::Supervision, e);
            }
        }
        let status = match self.owner.poll(now) {
            Ok(status) => status,
            Err(e) => {
                self.fail(FaultSource::Supervision, e);
                self.owner.status()
            }
        };
        if let SupervisorStatus::Exited { status, reason } = status {
            let report = Report {
                status,
                reason,
                signal: self.signal,
                fault: self.fault,
                terminal: self.owner.terminal(),
                progress_epoch: self.owner.epoch(),
            };
            self.report = Some(report);
            State::Exited(report)
        } else {
            State::Pending(status)
        }
    }
}
/// Owns the same Supervisor from spawn until actual exit. Caller drives this
/// from its independent control thread, using the same explicit monotonic time
/// domain as spawn. A blocked capture thread cannot drive its own parent lease.
/// Native signal/clock/wait errors must enter fail(), then continue polling this
/// same owner until actual exit; Drop alone does not prove exit or recovery.
pub struct Session {
    controller: Controller<Supervisor>,
}
impl Session {
    pub fn new(supervisor: Supervisor) -> Self {
        Self {
            controller: Controller::new(supervisor),
        }
    }
    pub fn poll(&mut self, now: MonoTime, signal: Option<StopSignal>) -> State {
        self.controller.poll(now, signal)
    }
    pub fn fail(&mut self, error: io::Error) {
        self.controller.fail(FaultSource::Control, error)
    }
    pub fn fault(&self) -> Option<Fault> {
        self.controller.fault
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    struct Mock {
        status: SupervisorStatus,
        shutdowns: usize,
        stops: usize,
        reads: usize,
        error: bool,
        stop_error: bool,
    }
    impl Owner for Mock {
        fn shutdown(&mut self, _: MonoTime) -> io::Result<SupervisorStatus> {
            self.shutdowns += 1;
            if self.error {
                Err(io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(self.status)
            }
        }
        fn stop(&mut self) -> io::Result<()> {
            self.stops += 1;
            if self.stop_error {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                Ok(())
            }
        }
        fn poll(&mut self, _: MonoTime) -> io::Result<SupervisorStatus> {
            Ok(self.status)
        }
        fn status(&self) -> SupervisorStatus {
            self.status
        }
        fn terminal(&mut self) -> ObservationReport {
            self.reads += 1;
            ObservationReport {
                status: ReadStatus::Missing,
                attempts: 1,
                failure: None,
            }
        }
        fn epoch(&self) -> u64 {
            3
        }
    }
    fn mock() -> Mock {
        Mock {
            status: SupervisorStatus::Running,
            shutdowns: 0,
            stops: 0,
            reads: 0,
            error: false,
            stop_error: false,
        }
    }
    #[test]
    fn first_signal_requests_once_no_report_before_actual_exit_and_cache_after() {
        let mut c = Controller::new(mock());
        assert!(matches!(
            c.poll(MonoTime(1), Some(StopSignal::Interrupt)),
            State::Pending(_)
        ));
        c.poll(MonoTime(2), Some(StopSignal::Terminate));
        assert_eq!(c.owner.shutdowns, 1);
        assert_eq!(c.owner.reads, 0);
        c.owner.status = SupervisorStatus::Exited {
            status: ExitStatus::from_raw(0),
            reason: Some(TerminationReason::Cooperative),
        };
        let State::Exited(report) = c.poll(MonoTime(3), None) else {
            panic!("missing actual exit");
        };
        assert_eq!(report.signal, Some(StopSignal::Interrupt));
        assert!(!report.successful());
        assert_eq!(
            c.poll(MonoTime(0), Some(StopSignal::Hangup)),
            State::Exited(report)
        );
        assert_eq!(c.owner.reads, 1);
        assert_eq!(c.owner.shutdowns, 1);
    }
    #[test]
    fn primary_control_failure_and_failed_stop_are_independent_and_latched() {
        let mut m = mock();
        m.stop_error = true;
        let mut c = Controller::new(m);
        c.fail(FaultSource::Control, io::ErrorKind::InvalidData.into());
        c.fail(FaultSource::Control, io::ErrorKind::Interrupted.into());
        c.poll(MonoTime(1), Some(StopSignal::Interrupt));
        let fault = c.fault.unwrap();
        assert_eq!(fault.primary.kind, io::ErrorKind::InvalidData);
        assert_eq!(
            fault.stop_failure.unwrap().kind,
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(c.owner.stops, 1);
        assert_eq!(c.owner.shutdowns, 0);
        assert_eq!(c.owner.reads, 0);
    }
    #[test]
    fn shutdown_error_forces_only_owned_stop_without_inventing_exit() {
        let mut m = mock();
        m.error = true;
        let mut c = Controller::new(m);
        assert!(matches!(
            c.poll(MonoTime(1), Some(StopSignal::Hangup)),
            State::Pending(_)
        ));
        assert_eq!(c.owner.stops, 1);
        assert_eq!(c.fault.unwrap().source, FaultSource::Supervision);
        assert_eq!(c.owner.reads, 0);
    }
    #[test]
    fn complete_report_recovery_and_exit_classification_are_independent() {
        use keyweave_linux::{runtime::TerminationReport, termination::TerminalFrame};
        let facts = TerminationReport {
            reason: StopReason::Pause,
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
        };
        let mut r = Report {
            status: ExitStatus::from_raw(0),
            reason: None,
            signal: None,
            fault: None,
            progress_epoch: 0,
            terminal: ObservationReport {
                status: ReadStatus::Complete(TerminalFrame {
                    run_id: 1,
                    report: facts,
                }),
                attempts: 1,
                failure: None,
            },
        };
        assert!(r.successful());
        r.status = ExitStatus::from_raw(68 << 8);
        assert!(r.recovery_confirmed());
        assert!(!r.successful());
        r.status = ExitStatus::from_raw(0);
        r.reason = Some(TerminationReason::Requested);
        assert!(!r.successful());
        r.reason = None;
        let mut unknown = facts;
        unknown.acceptance_unknown = true;
        r.terminal.status = ReadStatus::Complete(TerminalFrame {
            run_id: 1,
            report: unknown,
        });
        assert!(!r.recovery_confirmed());
        r.terminal.status = ReadStatus::Complete(TerminalFrame {
            run_id: 1,
            report: TerminationReport {
                reason: StopReason::InputFault,
                ..facts
            },
        });
        assert!(r.recovery_confirmed());
        assert!(!r.successful());
    }
}
