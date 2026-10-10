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
pub(crate) const RUN_ENV: &str = "KEYWEAVE_PROGRESS_RUN_ID";
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

/// One-shot startup handoff. Construct on main before any device is opened.
/// stdin/stderr remain reserved for the progress protocol until process exit.
pub struct WorkerStartup {
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
            guard,
            run_id,
            client,
        })
    }
    pub fn guard(&self) -> &WorkerParentGuard {
        &self.guard
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
    #[error("worker OS boundary failed: {0}")]
    System(#[from] io::Error),
    #[error(transparent)]
    Progress(#[from] ClientError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("worker wait endpoint failed: {0:?}")]
    Endpoint(StopReason),
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
    let _verified = WorkerParentGuard::verify(guard.parent_pid())?;
    for request in client.wait_requests_with_control(true) {
        crate::ioctl::nonblocking(request.fd)?;
    }
    run_with_wait_control(runtime, client, true, |runtime, client, clock| {
        runtime.wait_native_with_control(client, clock, true)
    })
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
    mut runtime: Runtime<S, B>,
    mut client: ProgressClient<R, W>,
    control: bool,
    mut wait: F,
) -> Result<StopReason, WorkerError>
where
    S: CaptureSource,
    B: FrameBackend,
    R: io::Read,
    W: io::Write,
    F: FnMut(&Runtime<S, B>, &ProgressClient<R, W>, &mut MonotonicClock) -> io::Result<WorkerWake>,
{
    let mut clock = MonotonicClock::new();
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
            runtime.poll(now)?;
            client.publish(&runtime)?;
            if runtime.phase() == RuntimePhase::Stopped {
                return Ok(runtime.stop_reason().unwrap_or(StopReason::ControlFault));
            }
            let wake = wait(&runtime, &client, &mut clock)?;
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
    result
}
#[cfg(test)]
mod tests {
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
}
