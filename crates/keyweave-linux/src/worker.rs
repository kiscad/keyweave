//! Guarded main-thread execution; no device is opened by this module.
use crate::{
    capture::CaptureSource,
    progress::{ClientError, ProgressClient},
    pump::FrameBackend,
    reactor::MonotonicClock,
    runtime::{Runtime, RuntimeError, RuntimePhase, StopReason},
};
use keyweave_types::MonoTime;
use std::{io, marker::PhantomData, rc::Rc};

pub(crate) const PARENT_ENV: &str = "KEYWEAVE_SUPERVISOR_PID";

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

pub(crate) fn run_with_wait<S, B, R, W, F>(
    mut runtime: Runtime<S, B>,
    mut client: ProgressClient<R, W>,
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
            client.receive(&runtime)?;
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
