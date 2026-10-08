//! Main-thread startup verification; no device is opened by this module.
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
