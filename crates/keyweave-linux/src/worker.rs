//! Guarded main-thread execution; no device is opened by this module.
use crate::{
    capture::CaptureSource,
    progress::{ClientError, ProgressClient},
    pump::FrameBackend,
    reactor::MonotonicClock,
    runtime::{Runtime, RuntimeError, RuntimePhase, StopReason},
};
use keyweave_types::MonoTime;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{io, marker::PhantomData, rc::Rc};

pub(crate) const PARENT_ENV: &str = "KEYWEAVE_SUPERVISOR_PID";
pub(crate) const REPORT_ENV: &str = "KEYWEAVE_TERMINAL_REPORT";
pub(crate) const RUN_ENV: &str = "KEYWEAVE_PROGRESS_RUN_ID";
pub(crate) const CONFIG_ENV: &str = "KEYWEAVE_CONFIG_FD";
static CHANNELS_CLAIMED: AtomicBool = AtomicBool::new(false);

fn positive_environment_u64(name: &str) -> io::Result<u64> {
    positive_u64(std::env::var(name).ok().as_deref()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("missing or invalid {name}"),
        )
    })
}
fn positive_u64(value: Option<&str>) -> Option<u64> {
    value
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 20
                && value.bytes().all(|byte| byte.is_ascii_digit())
        })
        .and_then(|value| value.parse().ok())
        .filter(|value| *value != 0)
}
fn config_descriptor(value: &str) -> Option<i32> {
    positive_u64(Some(value))
        .filter(|_| !value.starts_with('0'))
        .and_then(|v| i32::try_from(v).ok())
        .filter(|v| *v >= 3)
}

/// One-shot startup handoff. Construct on main before any device is opened.
/// stdin/stderr remain reserved for the progress protocol until process exit.
pub struct WorkerStartup {
    config_claimed: bool,
    guard: WorkerParentGuard,
    run_id: u64,
    client: ProgressClient<std::fs::File, std::fs::File>,
}
impl WorkerStartup {
    pub fn from_supervisor_environment() -> io::Result<Self> {
        use std::os::fd::AsFd;
        let guard = WorkerParentGuard::from_supervisor_environment()?;
        let run_id = positive_environment_u64(RUN_ENV)?;
        let reader = io::stdin();
        let writer = io::stderr();
        // Validate before irreversible claim or duplication. A second caller
        // must never race or share the single protocol receiver.
        if crate::ioctl::pipe_identity(reader.as_fd(), false)?
            == crate::ioctl::pipe_identity(writer.as_fd(), true)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "progress pipes must be distinct",
            ));
        }
        CHANNELS_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "worker progress channels already claimed",
                )
            })?;
        let (reader, writer) =
            crate::ioctl::duplicate_progress_pipes(reader.as_fd(), writer.as_fd())?;
        let client = ProgressClient::new(reader, writer, run_id)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        Ok(Self {
            config_claimed: false,
            guard,
            run_id,
            client,
        })
    }
    pub fn guard(&self) -> &WorkerParentGuard {
        &self.guard
    }
    /// One attempt on verified main, before files/devices/threads. The original
    /// inherited config FD is reserved until process exit; returned copy is CLOEXEC.
    pub fn claim_config(&mut self) -> io::Result<crate::config_artifact::ConfigArtifact> {
        if std::mem::replace(&mut self.config_claimed, true) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "configuration already claimed",
            ));
        }
        crate::ioctl::verify_parent_death(self.guard.parent)?;
        let value = std::env::var(CONFIG_ENV).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing configuration descriptor",
            )
        })?;
        let raw = config_descriptor(&value).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid configuration descriptor",
            )
        })?;
        crate::config_artifact::ConfigArtifact::from_inherited(raw)
    }
    pub fn run_id(&self) -> u64 {
        self.run_id
    }
    pub fn into_parts(
        self,
    ) -> (
        WorkerParentGuard,
        ProgressClient<std::fs::File, std::fs::File>,
    ) {
        (self.guard, self.client)
    }
}

/// Third-channel startup, verified on real main before files/devices/threads.
/// Original stdout is reserved until actual process exit, never formatted.
pub struct ReportingStartup {
    startup: WorkerStartup,
    terminal: crate::termination::TerminalWriter<std::fs::File>,
}
impl ReportingStartup {
    pub fn guard(&self) -> &WorkerParentGuard {
        self.startup.guard()
    }
    pub fn run_id(&self) -> u64 {
        self.startup.run_id()
    }
    pub fn claim_config(&mut self) -> io::Result<crate::config_artifact::ConfigArtifact> {
        self.startup.claim_config()
    }
    pub fn from_supervisor_environment() -> io::Result<Self> {
        use std::os::fd::AsFd;
        let _guard = WorkerParentGuard::from_supervisor_environment()?;
        if std::env::var(REPORT_ENV).ok().as_deref() != Some("1") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reporting mode required",
            ));
        }
        let run_id = positive_environment_u64(RUN_ENV)?;
        let stdout = io::stdout();
        let report_id = crate::ioctl::pipe_identity(stdout.as_fd(), true)?;
        if report_id == crate::ioctl::pipe_identity(io::stdin().as_fd(), false)?
            || report_id == crate::ioctl::pipe_identity(io::stderr().as_fd(), true)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal pipe must be distinct",
            ));
        }
        let startup = WorkerStartup::from_supervisor_environment()?;
        // Validated protocol endpoint only; std OwnedFd creates CLOEXEC copy.
        // Never take or close the original standard descriptor.
        let file = std::fs::File::from(stdout.as_fd().try_clone_to_owned()?);
        let terminal = crate::termination::TerminalWriter::new(file, run_id)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        Ok(Self { startup, terminal })
    }
    pub fn into_parts(
        self,
    ) -> (
        WorkerParentGuard,
        ProgressClient<std::fs::File, std::fs::File>,
        crate::termination::TerminalWriter<std::fs::File>,
    ) {
        let (guard, client) = self.startup.into_parts();
        (guard, client, self.terminal)
    }
}

/// Evidence that the worker main thread currently has SIGKILL parent-death
/// protection for its expected supervisor. Verify before opening any device.
///
/// The supervisor's creating thread must remain alive. Do not change effective
/// or filesystem credentials, clear the signal, exec, or fork with device FDs
/// after verification. This token cannot enforce subsequent process changes.
#[derive(Debug)]
pub struct WorkerParentGuard {
    parent: u32,
    _main_thread: PhantomData<Rc<()>>,
}
impl WorkerParentGuard {
    pub fn from_supervisor_environment() -> io::Result<Self> {
        let parent = std::env::var(PARENT_ENV)
            .ok()
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        Self::verify(parent)
    }
    /// Verification only: never silently installs or repairs a missing guard.
    pub fn verify(parent: u32) -> io::Result<Self> {
        crate::ioctl::verify_parent_death(parent)?;
        Ok(Self {
            parent,
            _main_thread: PhantomData,
        })
    }
    pub fn parent_pid(&self) -> u32 {
        self.parent
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error(transparent)]
    Consumer(#[from] crate::console::ConsoleLeaseError),
    #[error("worker OS boundary failed: {0}")]
    System(#[from] io::Error),
    #[error(transparent)]
    Progress(#[from] ClientError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("worker wait endpoint failed: {0:?}")]
    Endpoint(StopReason),
}
/// Independent loop outcome and explicit Runtime termination evidence. Neither
/// a stopped loop nor owner Drop alone confirms accepted output recovery.
#[derive(Debug)]
pub struct WorkerReport {
    pub outcome: Result<StopReason, WorkerError>,
    pub termination: Option<crate::runtime::TerminationReport>,
}
#[derive(Debug)]
pub struct ReportedWorker {
    pub worker: WorkerReport,
    pub delivery: crate::termination::bounded::DeliveryReport,
}
/// Consume startup claimed before device admission. Runtime must independently
/// satisfy output/consumer/neutral/capture gates. Does not exit the process.
pub fn run_reporting_worker(
    runtime: Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
    startup: ReportingStartup,
) -> Result<ReportedWorker, WorkerError> {
    let (guard, client, mut terminal) = startup.into_parts();
    let worker = run_worker_controlled_report(runtime, client, &guard)?;
    Ok(finalize_worker_report(worker, &mut terminal))
}
/// Finalization only: caller has completed controlled capture shutdown. Reject
/// changed/blocking endpoint flags before IO; original stdout remains reserved.
pub fn finalize_worker_report(
    worker: WorkerReport,
    writer: &mut crate::termination::TerminalWriter<std::fs::File>,
) -> ReportedWorker {
    if worker.termination.is_some()
        && let Err(error) = crate::ioctl::pipe_identity(writer.wait_fd(), true)
    {
        return ReportedWorker {
            worker,
            delivery: crate::termination::bounded::DeliveryReport {
                frame_sent: false,
                attempts: 0,
                failure: Some(crate::termination::bounded::DeliveryFailure::Admission(
                    error.into(),
                )),
            },
        };
    }

    let mut schedule = NativeTerminalSchedule(crate::reactor::MonotonicClock::new());
    finalize_worker(worker, writer, &mut schedule)
}
pub(crate) fn finalize_worker<W: io::Write, S: crate::termination::bounded::SendSchedule<W>>(
    worker: WorkerReport,
    writer: &mut crate::termination::TerminalWriter<W>,
    schedule: &mut S,
) -> ReportedWorker {
    let delivery = crate::termination::bounded::send_bounded(writer, worker.termination, schedule);
    ReportedWorker { worker, delivery }
}
pub(crate) struct NativeTerminalSchedule(pub(crate) crate::reactor::MonotonicClock);
impl crate::termination::bounded::SendSchedule<std::fs::File> for NativeTerminalSchedule {
    fn now(&mut self) -> io::Result<MonoTime> {
        self.0.now()
    }
    fn wait(
        &mut self,
        writer: &crate::termination::TerminalWriter<std::fs::File>,
        deadline: MonoTime,
    ) -> io::Result<(MonoTime, bool)> {
        let request = crate::reactor::WaitRequest {
            fd: writer.wait_fd(),
            interest: crate::reactor::Interest::Write,
        };
        let wake = crate::reactor::wait(
            &mut self.0,
            &[request],
            Some(deadline),
            crate::reactor::HOUSEKEEPING_NS,
        )?;
        Ok((wake.at_ns, wake.readiness(0).is_some_and(|r| r.fault())))
    }
}
impl crate::termination::bounded::ReadSchedule<io::PipeReader> for NativeTerminalSchedule {
    fn now(&mut self) -> io::Result<MonoTime> {
        self.0.now()
    }
    fn wait(
        &mut self,
        reader: &crate::termination::TerminalReader<io::PipeReader>,
        deadline: MonoTime,
    ) -> io::Result<(MonoTime, bool)> {
        let request = crate::reactor::WaitRequest {
            fd: reader.wait_fd(),
            interest: crate::reactor::Interest::Read,
        };
        let wake = crate::reactor::wait(
            &mut self.0,
            &[request],
            Some(deadline),
            crate::reactor::HOUSEKEEPING_NS,
        )?;
        // A writer's closure produces HUP even with readable queued bytes. Read
        // those bytes and EOF on the next bounded poll; HUP alone isn't failure.
        Ok((
            wake.at_ns,
            wake.readiness(0).is_some_and(|r| r.error || r.invalid),
        ))
    }
}
pub(crate) struct WorkerWake {
    pub at_ns: MonoTime,
    pub failure: Option<StopReason>,
}

/// Run an already admitted production Runtime on this guarded main thread.
/// Own nonblocking, unbuffered progress endpoints; no backend FD is duplicated.
/// This does not perform device discovery, readiness or capture admission.
pub fn run_worker<R, W>(
    runtime: Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
    client: ProgressClient<R, W>,
    guard: &WorkerParentGuard,
) -> Result<StopReason, WorkerError>
where
    R: io::Read + std::os::fd::AsFd,
    W: io::Write + std::os::fd::AsFd,
{
    let _verified = WorkerParentGuard::verify(guard.parent_pid())?;
    for request in client.wait_requests() {
        crate::ioctl::nonblocking(request.fd)?;
    }
    run_with_wait(runtime, client, |runtime, client, clock| {
        runtime.wait_native(client, clock)
    })
}

/// Opt-in controlled worker. StopReason is loop status, not output recovery proof.
pub fn run_worker_controlled<R, W>(
    runtime: Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
    client: ProgressClient<R, W>,
    guard: &WorkerParentGuard,
) -> Result<StopReason, WorkerError>
where
    R: io::Read + std::os::fd::AsFd,
    W: io::Write + std::os::fd::AsFd,
{
    run_worker_controlled_report(runtime, client, guard)?.outcome
}
/// Admission failures return before a report; caller must not infer cleanup.
pub fn run_worker_controlled_report<R, W>(
    runtime: Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
    client: ProgressClient<R, W>,
    guard: &WorkerParentGuard,
) -> Result<WorkerReport, WorkerError>
where
    R: io::Read + std::os::fd::AsFd,
    W: io::Write + std::os::fd::AsFd,
{
    let _verified = WorkerParentGuard::verify(guard.parent_pid())?;
    for request in client.wait_requests_with_control(true) {
        crate::ioctl::nonblocking(request.fd)?;
    }
    Ok(run_with_wait_report(
        runtime,
        client,
        true,
        |runtime, client, clock| runtime.wait_native_with_control(client, clock, true),
    ))
}

pub(crate) fn run_with_wait<S, B, R, W, F>(
    runtime: Runtime<S, B>,
    client: ProgressClient<R, W>,
    wait: F,
) -> Result<StopReason, WorkerError>
where
    S: CaptureSource,
    B: FrameBackend,
    R: io::Read,
    W: io::Write,
    F: FnMut(&Runtime<S, B>, &ProgressClient<R, W>, &mut MonotonicClock) -> io::Result<WorkerWake>,
{
    run_with_wait_control(runtime, client, false, wait)
}
pub(crate) fn run_with_wait_control<S, B, R, W, F>(
    runtime: Runtime<S, B>,
    client: ProgressClient<R, W>,
    control: bool,
    wait: F,
) -> Result<StopReason, WorkerError>
where
    S: CaptureSource,
    B: FrameBackend,
    R: io::Read,
    W: io::Write,
    F: FnMut(&Runtime<S, B>, &ProgressClient<R, W>, &mut MonotonicClock) -> io::Result<WorkerWake>,
{
    run_with_wait_report(runtime, client, control, wait).outcome
}
pub(crate) fn run_with_wait_report<S, B, R, W, F>(
    runtime: Runtime<S, B>,
    client: ProgressClient<R, W>,
    control: bool,
    mut wait: F,
) -> WorkerReport
where
    S: CaptureSource,
    B: FrameBackend,
    R: io::Read,
    W: io::Write,
    F: FnMut(&Runtime<S, B>, &ProgressClient<R, W>, &mut MonotonicClock) -> io::Result<WorkerWake>,
{
    run_with_poll_control(
        runtime,
        client,
        control,
        MonotonicClock::new(),
        NoConsumer,
        |runtime, client, clock, _| wait(runtime, client, clock),
    )
}
pub(crate) trait PollControl<S: CaptureSource, B: FrameBackend> {
    fn before_poll(
        &mut self,
        runtime: &Runtime<S, B>,
        clock: &mut MonotonicClock,
        now: MonoTime,
    ) -> Result<MonoTime, WorkerError>;
    fn next_wakeup(&self) -> Option<MonoTime>;
}
struct NoConsumer;
impl<S: CaptureSource, B: FrameBackend> PollControl<S, B> for NoConsumer {
    fn before_poll(
        &mut self,
        _: &Runtime<S, B>,
        _: &mut MonotonicClock,
        now: MonoTime,
    ) -> Result<MonoTime, WorkerError> {
        Ok(now)
    }
    fn next_wakeup(&self) -> Option<MonoTime> {
        None
    }
}
pub(crate) fn run_with_poll_control<S, B, R, W, C, F>(
    mut runtime: Runtime<S, B>,
    mut client: ProgressClient<R, W>,
    control: bool,
    mut clock: MonotonicClock,
    mut poll_control: C,
    mut wait: F,
) -> WorkerReport
where
    S: CaptureSource,
    B: FrameBackend,
    R: io::Read,
    W: io::Write,
    C: PollControl<S, B>,
    F: FnMut(
        &Runtime<S, B>,
        &ProgressClient<R, W>,
        &mut MonotonicClock,
        Option<MonoTime>,
    ) -> io::Result<WorkerWake>,
{
    let result = (|| {
        let mut now = clock.now()?;
        loop {
            if control {
                if client.receive_command(&runtime)? == keyweave_progress::RequestEvent::Stop {
                    runtime.pause(now)?; // closes capture before recovery/output
                    for _ in 0..crate::runtime::MAX_RECOVERY_POLLS {
                        if runtime.phase() != RuntimePhase::Recovering {
                            break;
                        }
                        runtime.poll(now)?;
                    }
                    return Ok(runtime.stop_reason().unwrap_or(StopReason::ControlFault));
                }
            } else {
                client.receive(&runtime)?;
            }
            now = poll_control.before_poll(&runtime, &mut clock, now)?;
            runtime.poll(now)?;
            client.publish(&runtime)?;
            if runtime.phase() == RuntimePhase::Stopped {
                return Ok(runtime.stop_reason().unwrap_or(StopReason::ControlFault));
            }
            let wake = wait(&runtime, &client, &mut clock, poll_control.next_wakeup())?;
            now = wake.at_ns;
            if let Some(reason) = wake.failure {
                runtime.external_fault(reason);
                return Err(WorkerError::Endpoint(reason));
            }
        }
    })();
    if result.is_err() {
        // Close capture before recovery, without further wait or publication.
        // Use the last accepted Runtime time, never a fabricated new deadline.
        runtime.external_fault(StopReason::ControlFault);
        runtime.finish_recovery();
    }
    let termination = runtime.termination_report();
    drop(runtime); // all capture/output owners precede the read-only provider
    drop(poll_control);
    WorkerReport {
        outcome: result,
        termination,
    }
}

struct ConsoleControl {
    provider: crate::console::ConsoleProvider,
    lease: crate::console::Lease,
}
impl PollControl<crate::capture::EvdevSource, crate::uinput::UinputDevice> for ConsoleControl {
    fn before_poll(
        &mut self,
        runtime: &Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
        clock: &mut MonotonicClock,
        now: MonoTime,
    ) -> Result<MonoTime, WorkerError> {
        if runtime.phase() != RuntimePhase::Active {
            return Ok(now);
        }
        let now = clock.now()?;
        let output = runtime
            .backend()
            .ok_or(crate::console::ConsoleLeaseError::NotReady)?;
        if self.lease.check(output.input_syspath(), now)? {
            let ready = self
                .provider
                .observe(output, clock)
                .map_err(|error| self.lease.provider_failed(error))?;
            let after = clock.now()?;
            self.lease.renew(output.input_syspath(), after, ready)?;
            return Ok(after);
        }
        Ok(now)
    }
    fn next_wakeup(&self) -> Option<MonoTime> {
        self.lease.next_wakeup()
    }
}
/// Explicit opt-in active text-console loop. All values come from the guarded
/// ConsoleBootstrap handoff; clock must be that same worker monotonic clock.
/// Stop wins before provider IO. Consumer failure uses ControlFault shutdown,
/// preserving the independent error and Runtime termination facts.
pub fn run_console_worker_controlled_report<R, W>(
    runtime: Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
    client: ProgressClient<R, W>,
    guard: &WorkerParentGuard,
    provider: crate::console::ConsoleProvider,
    clock: MonotonicClock,
    ready: crate::console::ConsoleReady,
) -> Result<WorkerReport, WorkerError>
where
    R: io::Read + std::os::fd::AsFd,
    W: io::Write + std::os::fd::AsFd,
{
    WorkerParentGuard::verify(guard.parent_pid())?;
    for request in client.wait_requests_with_control(true) {
        crate::ioctl::nonblocking(request.fd)?;
    }
    Ok(run_with_poll_control(
        runtime,
        client,
        true,
        clock,
        ConsoleControl {
            provider,
            lease: crate::console::Lease::new(ready),
        },
        |runtime, client, clock, deadline| runtime.wait_native_until(client, clock, true, deadline),
    ))
}
pub fn run_reporting_console_worker(
    runtime: Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
    provider: crate::console::ConsoleProvider,
    clock: MonotonicClock,
    ready: crate::console::ConsoleReady,
    startup: ReportingStartup,
) -> Result<ReportedWorker, WorkerError> {
    let (guard, client, mut terminal) = startup.into_parts();
    let worker =
        run_console_worker_controlled_report(runtime, client, &guard, provider, clock, ready)?;
    Ok(finalize_worker_report(worker, &mut terminal))
}
pub enum ConsoleSessionResult {
    /// No Runtime was dispatched. Terminal report intentionally absent.
    StartupStopped,
    Worker(ReportedWorker),
}
#[derive(Debug, thiserror::Error)]
pub enum ConsoleSessionError {
    #[error(transparent)]
    Bootstrap(#[from] crate::bootstrap::BootstrapError),
    #[error(transparent)]
    Worker(#[from] WorkerError),
}
/// Full guarded console transaction, with cold configuration already compiled.
/// No endpoint logging and no fabricated Runtime report during startup.
pub fn run_reporting_console_session(
    startup: ReportingStartup,
    vt: &std::path::Path,
    uinput: &std::path::Path,
    inputs: Vec<(keyweave_types::DeviceInstanceId, std::path::PathBuf)>,
    config: std::sync::Arc<keyweave_engine::ConfigSnapshot>,
    spec: crate::output::DeviceSpec,
) -> Result<ConsoleSessionResult, ConsoleSessionError> {
    let (guard, mut client, mut terminal) = startup.into_parts();
    WorkerParentGuard::verify(guard.parent_pid()).map_err(WorkerError::from)?;
    for request in client.wait_requests_with_control(true) {
        crate::ioctl::nonblocking(request.fd).map_err(WorkerError::from)?;
    }
    // Before even read-only VT/provider construction. Later stop arrivals are
    // cooperative checkpoints, not an atomic prohibition on the next syscall.
    if client
        .receive_startup_command()
        .map_err(WorkerError::from)?
        == keyweave_progress::RequestEvent::Stop
    {
        return Ok(ConsoleSessionResult::StartupStopped);
    }
    let boot = crate::bootstrap::ConsoleBootstrap::new(&guard, vt, uinput, inputs, config, spec)?;
    let Some(crate::bootstrap::ConsolePrepared {
        runtime,
        provider,
        clock,
        ready,
    }) = boot.run(&mut client)?
    else {
        return Ok(ConsoleSessionResult::StartupStopped);
    };
    let worker =
        run_console_worker_controlled_report(runtime, client, &guard, provider, clock, ready)?;
    Ok(ConsoleSessionResult::Worker(finalize_worker_report(
        worker,
        &mut terminal,
    )))
}
#[cfg(test)]
mod tests {
    use super::config_descriptor;
    use super::positive_u64;
    #[test]
    fn startup_identity_is_positive_bounded_ascii_without_signs_or_whitespace() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("+1"),
            Some("-1"),
            Some(" 1"),
            Some("1\n"),
            Some("１"),
            Some("18446744073709551616"),
            Some("000000000000000000001"),
        ] {
            assert_eq!(positive_u64(value), None, "{value:?}");
        }
        assert_eq!(positive_u64(Some("1")), Some(1));
        assert_eq!(positive_u64(Some("0001")), Some(1));
        assert_eq!(positive_u64(Some("18446744073709551615")), Some(u64::MAX));
    }
    #[test]
    fn config_descriptor_is_canonical_bounded_and_disjoint_from_stdio() {
        for value in [
            "",
            "0",
            "1",
            "2",
            "03",
            "+3",
            "-3",
            " 3",
            "3\n",
            "３",
            "2147483648",
            "999999999999999999999",
        ] {
            assert_eq!(config_descriptor(value), None);
        }
        assert_eq!(config_descriptor("3"), Some(3));
        assert_eq!(config_descriptor("2147483647"), Some(i32::MAX));
    }
    #[test]
    fn native_finalization_rejects_non_pipe_owner_without_writing() {
        use super::{WorkerReport, finalize_worker_report};
        use crate::{
            runtime::{StopReason, TerminationReport},
            termination::{TerminalWriter, bounded::DeliveryFailure},
        };
        let path =
            std::env::temp_dir().join(format!("keyweave-finalization-{}.bin", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("{e}"));
        let mut writer = TerminalWriter::new(file, 7).unwrap_or_else(|e| panic!("{e}"));
        let worker = WorkerReport {
            outcome: Ok(StopReason::Pause),
            termination: Some(TerminationReport {
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
            }),
        };
        let reported = finalize_worker_report(worker, &mut writer);
        assert!(matches!(
            reported.delivery.failure,
            Some(DeliveryFailure::Admission(_))
        ));
        assert_eq!(reported.delivery.attempts, 0);
        assert!(!reported.delivery.frame_sent);
        assert!(matches!(reported.worker.outcome, Ok(StopReason::Pause)));
        assert_eq!(
            std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("{e}"))
                .len(),
            0
        );
        drop(writer);
        std::fs::remove_file(path).unwrap_or_else(|e| panic!("{e}"));
    }
}
