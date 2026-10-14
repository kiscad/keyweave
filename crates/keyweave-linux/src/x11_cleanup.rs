//! Cleanup after Runtime owners closed. Keep the same pending probe owner on every failure.
use crate::{
    worker::{WorkerReport, X11WorkerReport},
    x11_consumer::NativeX11Lease,
    x11_probe::ProbePhase,
};
use keyweave_types::MonoTime;
use std::io;
const BUDGET_NS: u64 = 500_000_000;
const POLL_NS: u64 = 2_000_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupTimingFailure {
    Overflow,
    Clock,
    Backwards,
    Deadline,
    AttemptBudget,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupConfirmation {
    NoPendingJob,
    ObservedExit,
}
pub(crate) trait Owner {
    fn cancel(&mut self) -> io::Result<()>;
    fn poll(&mut self) -> io::Result<Option<ProbePhase>>;
}
impl Owner for NativeX11Lease {
    fn cancel(&mut self) -> io::Result<()> {
        self.cancel()
    }
    fn poll(&mut self) -> io::Result<Option<ProbePhase>> {
        self.poll_canceled()
    }
}
pub(crate) struct Task<O> {
    pub(crate) owner: O,
    pub(crate) last: Option<MonoTime>,
    pub(crate) deadline: Option<MonoTime>,
    pub(crate) confirmation: Option<CleanupConfirmation>,
    pub(crate) timing: Option<CleanupTimingFailure>,
    pub(crate) cancel_error: Option<io::Error>,
    pub(crate) poll_error: Option<io::Error>,
    pub(crate) clock_error: Option<io::Error>,
    pub(crate) wait_error: Option<io::Error>,
}
impl<O: Owner> Task<O> {
    fn new(owner: O, now: MonoTime) -> Self {
        Self::new_at(owner, Ok(now))
    }
    pub(crate) fn new_at(mut owner: O, now: io::Result<MonoTime>) -> Self {
        let (last, deadline, timing, clock_error) = match now {
            Ok(now) => {
                let deadline = now.checked_add(BUDGET_NS);
                (
                    Some(now),
                    deadline,
                    deadline.is_none().then_some(CleanupTimingFailure::Overflow),
                    None,
                )
            }
            Err(error) => (None, None, Some(CleanupTimingFailure::Clock), Some(error)),
        };
        let cancel_error = owner.cancel().err();
        Self {
            owner,
            last,
            deadline,
            confirmation: None,
            timing,
            cancel_error,
            poll_error: None,
            clock_error,
            wait_error: None,
        }
    }
    pub(crate) fn step(&mut self, now: io::Result<MonoTime>) -> Option<CleanupConfirmation> {
        if self.confirmation.is_some() {
            return self.confirmation;
        }
        let timing = match now {
            Err(error) => {
                if self.clock_error.is_none() {
                    self.clock_error = Some(error);
                }
                Some(CleanupTimingFailure::Clock)
            }
            Ok(now) if self.last.is_some_and(|last| now < last) => {
                Some(CleanupTimingFailure::Backwards)
            }
            Ok(now) => {
                self.last = Some(now);
                self.deadline
                    .filter(|deadline| now >= *deadline)
                    .map(|_| CleanupTimingFailure::Deadline)
            }
        };
        if self.timing.is_none() {
            self.timing = timing;
        }
        // Cancellation/clock failure is not exit. Still observe this same owner once.
        match self.owner.poll() {
            Ok(None) => self.confirmation = Some(CleanupConfirmation::NoPendingJob),
            Ok(Some(ProbePhase::Exited)) => {
                self.confirmation = Some(CleanupConfirmation::ObservedExit)
            }
            Ok(Some(_)) => {}
            Err(error) => {
                if self.poll_error.is_none() {
                    self.poll_error = Some(error);
                }
            }
        }
        self.confirmation
    }
    pub(crate) fn next_wakeup(&self) -> Option<MonoTime> {
        if self.confirmation.is_some()
            || self.timing.is_some()
            || self.poll_error.is_some()
            || self.cancel_error.is_some()
            || self.wait_error.is_some()
        {
            return None;
        }
        Some(self.last?.checked_add(POLL_NS)?.min(self.deadline?))
    }
}
pub(crate) trait Schedule {
    fn now(&mut self) -> io::Result<MonoTime>;
    fn wait(&mut self, deadline: MonoTime) -> io::Result<()>;
}
pub(crate) struct NativeSchedule<'a>(pub(crate) &'a mut crate::reactor::MonotonicClock);
impl Schedule for NativeSchedule<'_> {
    fn now(&mut self) -> io::Result<MonoTime> {
        self.0.now()
    }
    fn wait(&mut self, deadline: MonoTime) -> io::Result<()> {
        let wake = crate::reactor::wait(self.0, &[], Some(deadline), POLL_NS)?;
        if wake.any_fault() {
            return Err(io::Error::other("cleanup wait fault"));
        }
        Ok(())
    }
}
impl<O: Owner> Task<O> {
    pub(crate) fn run(&mut self, schedule: &mut impl Schedule) -> bool {
        for _ in 0..512 {
            if self.step(schedule.now()).is_some() {
                return true;
            }
            let Some(deadline) = self.next_wakeup() else {
                return false;
            };
            if let Err(error) = schedule.wait(deadline) {
                if self.wait_error.is_none() {
                    self.wait_error = Some(error);
                }
                return self.step(schedule.now()).is_some();
            }
        }
        if self.timing.is_none() {
            self.timing = Some(CleanupTimingFailure::AttemptBudget);
        }
        false
    }
}
/// Runtime outcome and cleanup failures are independent. This contains no pending consumer owner.
#[derive(Debug)]
pub struct X11CleanupReport {
    pub worker: WorkerReport,
    pub prior_cancel_error: Option<io::Error>,
    pub cancel_error: Option<io::Error>,
    pub poll_error: Option<io::Error>,
    pub clock_error: Option<io::Error>,
    pub wait_error: Option<io::Error>,
    pub timing_failure: Option<CleanupTimingFailure>,
    pub confirmation: CleanupConfirmation,
    consumer_owner_dropped: bool,
}
impl X11CleanupReport {
    pub fn consumer_owner_dropped(&self) -> bool {
        self.consumer_owner_dropped
    }
}
#[derive(Debug)]
pub struct ReportedX11Cleanup {
    pub cleanup: X11CleanupReport,
    pub delivery: crate::termination::bounded::DeliveryReport,
}
/// Only a completed cleanup report can be sent. Preserve runtime, cleanup and delivery separately.
pub fn finalize_x11_cleanup(
    report: X11CleanupReport,
    writer: &mut crate::termination::TerminalWriter<std::fs::File>,
) -> ReportedX11Cleanup {
    finalize_with(report, |worker| {
        crate::worker::finalize_worker_report(worker, writer)
    })
}
fn finalize_with(
    report: X11CleanupReport,
    finalize: impl FnOnce(WorkerReport) -> crate::worker::ReportedWorker,
) -> ReportedX11Cleanup {
    let X11CleanupReport {
        worker,
        prior_cancel_error,
        cancel_error,
        poll_error,
        clock_error,
        wait_error,
        timing_failure,
        confirmation,
        consumer_owner_dropped,
    } = report;
    let crate::worker::ReportedWorker { worker, delivery } = finalize(worker);
    ReportedX11Cleanup {
        cleanup: X11CleanupReport {
            worker,
            prior_cancel_error,
            cancel_error,
            poll_error,
            clock_error,
            wait_error,
            timing_failure,
            confirmation,
            consumer_owner_dropped,
        },
        delivery,
    }
}
pub struct X11Cleanup {
    worker: WorkerReport,
    prior_cancel_error: Option<io::Error>,
    task: Task<NativeX11Lease>,
}
impl X11Cleanup {
    pub fn new(report: X11WorkerReport, now: MonoTime) -> Self {
        Self {
            worker: report.worker,
            prior_cancel_error: report.cancel_error,
            task: Task::new(report.consumer, now),
        }
    }
    pub fn new_at(report: X11WorkerReport, now: io::Result<MonoTime>) -> Self {
        match now {
            Ok(now) => Self::new(report, now),
            Err(error) => Self {
                worker: report.worker,
                prior_cancel_error: report.cancel_error,
                task: Task::new_at(report.consumer, Err(error)),
            },
        }
    }
    pub fn run(
        mut self,
        clock: &mut crate::reactor::MonotonicClock,
    ) -> Result<X11CleanupReport, Box<Self>> {
        if self.task.run(&mut NativeSchedule(clock)) {
            self.finish()
        } else {
            Err(Box::new(self))
        }
    }
    /// At most one child observation. Clock errors do not prevent reaping a canceled child.
    pub fn step(&mut self, now: io::Result<MonoTime>) -> Option<CleanupConfirmation> {
        self.task.step(now)
    }
    pub fn next_wakeup(&self) -> Option<MonoTime> {
        self.task.next_wakeup()
    }
    pub fn timing_failure(&self) -> Option<CleanupTimingFailure> {
        self.task.timing
    }
    /// A deferred result keeps the same owner; caller continues observing it, without restarting.
    pub fn finish(self) -> Result<X11CleanupReport, Box<Self>> {
        let Some(confirmation) = self.task.confirmation else {
            return Err(Box::new(self));
        };
        let Self {
            worker,
            prior_cancel_error,
            task,
        } = self;
        let Task {
            owner,
            timing,
            cancel_error,
            poll_error,
            clock_error,
            wait_error,
            ..
        } = task;
        drop(owner);
        Ok(X11CleanupReport {
            worker,
            prior_cancel_error,
            cancel_error,
            poll_error,
            clock_error,
            wait_error,
            timing_failure: timing,
            confirmation,
            consumer_owner_dropped: true,
        })
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque, rc::Rc};
    struct Mock {
        polls: VecDeque<io::Result<Option<ProbePhase>>>,
        canceled: Rc<Cell<bool>>,
        dropped: Rc<Cell<bool>>,
        fail_cancel: bool,
    }
    impl Owner for Mock {
        fn cancel(&mut self) -> io::Result<()> {
            self.canceled.set(true);
            if self.fail_cancel {
                Err(io::Error::other("cancel"))
            } else {
                Ok(())
            }
        }
        fn poll(&mut self) -> io::Result<Option<ProbePhase>> {
            self.polls
                .pop_front()
                .unwrap_or(Ok(Some(ProbePhase::Running)))
        }
    }
    impl Drop for Mock {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }
    fn mock(polls: Vec<io::Result<Option<ProbePhase>>>) -> Mock {
        Mock {
            polls: polls.into(),
            canceled: Rc::new(Cell::new(false)),
            dropped: Rc::new(Cell::new(false)),
            fail_cancel: false,
        }
    }
    #[test]
    fn cancel_is_not_exit_and_deadline_keeps_owner_for_later_observation() {
        let owner = mock(vec![
            Ok(Some(ProbePhase::Running)),
            Ok(Some(ProbePhase::Exited)),
        ]);
        let dropped = owner.dropped.clone();
        let canceled = owner.canceled.clone();
        let mut task = Task::new(owner, MonoTime(0));
        assert!(canceled.get());
        assert!(task.confirmation.is_none());
        assert_eq!(task.step(Ok(MonoTime(BUDGET_NS))), None);
        assert_eq!(task.timing, Some(CleanupTimingFailure::Deadline));
        assert!(!dropped.get());
        assert_eq!(task.next_wakeup(), None);
        assert_eq!(
            task.step(Ok(MonoTime(BUDGET_NS + 1))),
            Some(CleanupConfirmation::ObservedExit)
        );
        assert_eq!(task.timing, Some(CleanupTimingFailure::Deadline));
        drop(task);
        assert!(dropped.get());
    }
    #[test]
    fn clock_failures_do_not_fabricate_time_or_block_exit_observation() {
        for fault in [Err(io::Error::other("clock")), Ok(MonoTime(9))] {
            let owner = mock(vec![Ok(Some(ProbePhase::Exited))]);
            let mut task = Task::new(owner, MonoTime(10));
            assert_eq!(task.step(fault), Some(CleanupConfirmation::ObservedExit));
            assert!(task.timing.is_some());
            assert_eq!(task.last, Some(MonoTime(10)));
        }
        let task = Task::new(mock(vec![]), MonoTime(u64::MAX));
        assert_eq!(task.timing, Some(CleanupTimingFailure::Overflow));
        assert_eq!(task.next_wakeup(), None);
    }
    #[test]
    fn cancel_and_poll_errors_remain_independent_and_owner_is_retained() {
        let mut owner = mock(vec![
            Err(io::Error::other("poll")),
            Ok(Some(ProbePhase::Exited)),
        ]);
        owner.fail_cancel = true;
        let dropped = owner.dropped.clone();
        let mut task = Task::new(owner, MonoTime(0));
        assert_eq!(task.step(Ok(MonoTime(1))), None);
        assert!(!dropped.get());
        assert!(task.cancel_error.is_some() && task.poll_error.is_some());
        assert_eq!(task.next_wakeup(), None);
        assert_eq!(
            task.step(Ok(MonoTime(2))),
            Some(CleanupConfirmation::ObservedExit)
        );
        assert!(task.cancel_error.is_some() && task.poll_error.is_some());
    }
    #[test]
    fn no_pending_job_confirms_without_claiming_an_observed_exit() {
        let mut task = Task::new(mock(vec![Ok(None)]), MonoTime(0));
        assert_eq!(task.next_wakeup(), Some(MonoTime(POLL_NS)));
        assert_eq!(
            task.step(Ok(MonoTime(1))),
            Some(CleanupConfirmation::NoPendingJob)
        );
        assert_eq!(task.next_wakeup(), None);
    }
    #[test]
    fn actual_owned_helper_is_reaped_by_same_cleanup_task() -> io::Result<()> {
        use crate::x11_probe::{ProbeJob, ProbeTarget};
        use std::{os::unix::fs::PermissionsExt, path::Path};
        struct Job(ProbeJob);
        impl Owner for Job {
            fn cancel(&mut self) -> io::Result<()> {
                self.0.request_stop()
            }
            fn poll(&mut self) -> io::Result<Option<ProbePhase>> {
                self.0.poll(MonoTime(10)).map(Some)
            }
        }
        let _image_guard = crate::HELPER_IMAGE_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root =
            std::env::temp_dir().join(format!("keyweave-x11-cleanup-{}", std::process::id()));
        std::fs::create_dir(&root)?;
        let image = root.join("probe");
        std::fs::write(&image, "#!/usr/bin/python3\nimport time\ntime.sleep(10)\n")?;
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o700))?;
        let target = ProbeTarget::new(":0", Path::new("/dev/input/event42"), 13, 106)?;
        let job = ProbeJob::spawn(&image, target, MonoTime(10), BUDGET_NS)?;
        let mut task = Task::new(Job(job), MonoTime(10));
        assert!(task.confirmation.is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while task.step(Ok(MonoTime(10))).is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(task.confirmation, Some(CleanupConfirmation::ObservedExit));
        assert!(task.owner.0.exit_status().is_some());
        assert!(task.owner.0.take_report().is_none());
        drop(task);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    fn completed_fixture(termination: bool) -> X11CleanupReport {
        use crate::runtime::{StopReason, TerminationReport};
        let owner = mock(vec![Ok(None)]);
        let dropped = owner.dropped.clone();
        let mut task = Task::new(owner, MonoTime(0));
        assert_eq!(
            task.step(Ok(MonoTime(1))),
            Some(CleanupConfirmation::NoPendingJob)
        );
        drop(task);
        assert!(dropped.get());
        X11CleanupReport {
            worker: WorkerReport {
                outcome: Err(crate::worker::WorkerError::X11Consumer(
                    crate::x11_consumer::X11LeaseError::NotReady,
                )),
                termination: termination.then_some(TerminationReport {
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
                }),
            },
            prior_cancel_error: Some(io::Error::other("prior cancel")),
            cancel_error: Some(io::Error::other("cleanup cancel")),
            poll_error: Some(io::Error::other("poll")),
            clock_error: Some(io::Error::other("clock")),
            wait_error: None,
            timing_failure: Some(CleanupTimingFailure::Deadline),
            confirmation: CleanupConfirmation::NoPendingJob,
            consumer_owner_dropped: true,
        }
    }
    #[test]
    fn actual_owned_pipe_bridge_preserves_runtime_cleanup_and_delivery() -> io::Result<()> {
        use crate::termination::{TerminalFrame, TerminalWriter};
        use std::{
            io::Read,
            os::fd::{AsFd, OwnedFd},
        };
        let (mut reader, writer) = io::pipe()?;
        let writer = std::fs::File::from(OwnedFd::from(writer));
        crate::ioctl::nonblocking(writer.as_fd())?;
        let mut terminal = TerminalWriter::new(writer, 7).unwrap();
        let result = finalize_x11_cleanup(completed_fixture(true), &mut terminal);
        assert!(result.delivery.frame_sent && result.delivery.failure.is_none());
        assert!(result.cleanup.consumer_owner_dropped());
        assert!(matches!(
            result.cleanup.worker.outcome,
            Err(crate::worker::WorkerError::X11Consumer(_))
        ));
        assert_eq!(
            result.cleanup.timing_failure,
            Some(CleanupTimingFailure::Deadline)
        );
        assert!(
            result.cleanup.prior_cancel_error.is_some()
                && result.cleanup.cancel_error.is_some()
                && result.cleanup.poll_error.is_some()
                && result.cleanup.clock_error.is_some()
        );
        let mut bytes = [0; crate::termination::FRAME_BYTES];
        reader.read_exact(&mut bytes)?;
        let frame = TerminalFrame::decode(&bytes, 7).unwrap();
        assert!(frame.report.output_recovery_confirmed);
        Ok(())
    }
    #[test]
    fn ordinary_file_bridge_refuses_io_without_overwriting_cleanup_facts() -> io::Result<()> {
        use crate::termination::{TerminalWriter, bounded::DeliveryFailure};
        let path =
            std::env::temp_dir().join(format!("keyweave-x11-terminal-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let mut writer = TerminalWriter::new(file, 7).unwrap();
        let result = finalize_x11_cleanup(completed_fixture(true), &mut writer);
        assert!(matches!(
            result.delivery.failure,
            Some(DeliveryFailure::Admission(_))
        ));
        assert!(!result.delivery.frame_sent);
        assert_eq!(result.delivery.attempts, 0);
        assert!(
            result
                .cleanup
                .worker
                .termination
                .unwrap()
                .output_recovery_confirmed
        );
        assert!(result.cleanup.consumer_owner_dropped());
        assert_eq!(std::fs::metadata(&path)?.len(), 0);
        drop(writer);
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn missing_runtime_termination_is_not_fabricated_by_completed_cleanup() -> io::Result<()> {
        use crate::termination::{TerminalWriter, bounded::DeliveryFailure};
        use std::os::fd::{AsFd, OwnedFd};
        let (_reader, writer) = io::pipe()?;
        let writer = std::fs::File::from(OwnedFd::from(writer));
        crate::ioctl::nonblocking(writer.as_fd())?;
        let mut terminal = TerminalWriter::new(writer, 7).unwrap();
        let result = finalize_x11_cleanup(completed_fixture(false), &mut terminal);
        assert!(matches!(
            result.delivery.failure,
            Some(DeliveryFailure::MissingReport)
        ));
        assert!(!result.delivery.frame_sent);
        assert_eq!(result.delivery.attempts, 0);
        assert!(result.cleanup.worker.termination.is_none());
        assert!(result.cleanup.consumer_owner_dropped());
        Ok(())
    }
    struct FakeSchedule {
        now: u64,
        advance: u64,
        wait_fault: bool,
        waits: usize,
    }
    impl Schedule for FakeSchedule {
        fn now(&mut self) -> io::Result<MonoTime> {
            Ok(MonoTime(self.now))
        }
        fn wait(&mut self, _: MonoTime) -> io::Result<()> {
            self.waits += 1;
            self.now += self.advance;
            if self.wait_fault {
                Err(io::Error::other("wait"))
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn bounded_schedule_confirms_without_replacing_owner() {
        let owner = mock(vec![
            Ok(Some(ProbePhase::Running)),
            Ok(Some(ProbePhase::Exited)),
        ]);
        let dropped = owner.dropped.clone();
        let mut task = Task::new_at(owner, Ok(MonoTime(0)));
        let mut schedule = FakeSchedule {
            now: 0,
            advance: POLL_NS,
            wait_fault: false,
            waits: 0,
        };
        assert!(task.run(&mut schedule));
        assert_eq!(schedule.waits, 1);
        assert!(!dropped.get());
        assert_eq!(task.confirmation, Some(CleanupConfirmation::ObservedExit));
        drop(task);
        assert!(dropped.get());
    }
    #[test]
    fn stationary_clock_hits_attempt_budget_and_keeps_same_owner() {
        let owner = mock(vec![]);
        let dropped = owner.dropped.clone();
        let mut task = Task::new_at(owner, Ok(MonoTime(0)));
        let mut schedule = FakeSchedule {
            now: 0,
            advance: 0,
            wait_fault: false,
            waits: 0,
        };
        assert!(!task.run(&mut schedule));
        assert_eq!(schedule.waits, 512);
        assert_eq!(task.timing, Some(CleanupTimingFailure::AttemptBudget));
        assert!(!dropped.get());
        assert_eq!(task.next_wakeup(), None);
    }
    #[test]
    fn wait_failure_can_observe_exit_and_retains_independent_error() {
        let owner = mock(vec![
            Ok(Some(ProbePhase::Running)),
            Ok(Some(ProbePhase::Exited)),
        ]);
        let mut task = Task::new_at(owner, Ok(MonoTime(0)));
        let mut schedule = FakeSchedule {
            now: 0,
            advance: 1,
            wait_fault: true,
            waits: 0,
        };
        assert!(task.run(&mut schedule));
        assert_eq!(
            task.wait_error.as_ref().map(|error| error.to_string()),
            Some("wait".into())
        );
        assert_eq!(task.confirmation, Some(CleanupConfirmation::ObservedExit));
    }
    #[test]
    fn initial_clock_failure_has_no_fabricated_deadline_but_can_reap() {
        let mut task = Task::new_at(
            mock(vec![Ok(Some(ProbePhase::Exited))]),
            Err(io::Error::other("clock")),
        );
        assert_eq!(task.last, None);
        assert_eq!(task.deadline, None);
        assert!(task.clock_error.is_some());
        let mut schedule = FakeSchedule {
            now: 1,
            advance: 1,
            wait_fault: false,
            waits: 0,
        };
        assert!(task.run(&mut schedule));
        assert_eq!(schedule.waits, 0);
        assert_eq!(task.timing, Some(CleanupTimingFailure::Clock));
    }
}
