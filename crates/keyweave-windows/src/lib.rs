//! Windows keyboard semantics and explicit native integration. No automatic capture.
pub mod access;
pub mod emergency;
pub mod hook;
pub mod keys;
#[cfg(any(target_os = "windows", test))]
mod native;
#[cfg(any(target_os = "windows", test))]
pub mod progress;
pub mod pump;
pub mod reactor;
pub mod runtime;
pub mod startup;
#[cfg(target_os = "windows")]
pub use native::SendInputBackend;
#[cfg(target_os = "windows")]
pub use native::{HookSession, HookSessionError, LoopStep};
#[cfg(target_os = "windows")]
pub use native::{check_input_access, sample_input_state};

#[cfg(target_os = "windows")]
pub use native::{CaptureSession, SessionError, SessionStep, ShutdownReport};

#[cfg(target_os = "windows")]
pub use native::{NotificationCleanup, NotificationError, SessionNotifications};

#[cfg(target_os = "windows")]
pub use native::{ChannelCleanupReport, JobError, JobProcessState, JobSpawnError, OwnedJobProcess};
#[cfg(target_os = "windows")]
pub use native::{PipeCloseReport, PipeReader, PipeWriter};
#[cfg(target_os = "windows")]
pub use native::{
    WorkerParentGuard, WorkerStartup, WorkerStartupError, check_synchronous_progress_pipe_rejected,
};
