//! Windows keyboard semantics and explicit native integration. No automatic capture.
pub mod access;
#[cfg(any(target_os = "windows", test))]
pub mod control_identity;
pub mod emergency;
pub mod hook;
pub mod keys;
pub mod mixed;
pub mod mixed_actor;
pub mod mouse_hook;
#[cfg(any(target_os = "windows", test))]
mod native;
pub mod pointer;
#[cfg(any(target_os = "windows", test))]
pub mod progress;
pub mod pump;
pub mod reactor;
pub mod runtime;
pub mod session_control;
pub mod startup;
#[cfg(target_os = "windows")]
pub use native::SendInputBackend;
#[cfg(target_os = "windows")]
pub use native::{
    ArtifactCleanup, ArtifactCreateError, ArtifactError, ArtifactReaderError, ConfigArtifact,
    ConfigReader, MAX_ARTIFACT_BYTES,
};
#[cfg(windows)]
pub use native::{ConsoleControl, ConsoleError};
#[cfg(all(windows, not(test)))]
pub use native::{ConsoleProbe, with_private_console_event};
#[cfg(target_os = "windows")]
pub use native::{HookCloseReport, HookSession, HookSessionError, LoopStep};
#[cfg(target_os = "windows")]
pub use native::{IdentityError, RunIdentity};
#[cfg(target_os = "windows")]
pub use native::{check_input_access, sample_input_state};

#[cfg(target_os = "windows")]
pub use native::{
    CaptureSession, MixedCaptureSession, MixedShutdownReport, SessionError, SessionStep,
    ShutdownReport,
};

#[cfg(target_os = "windows")]
pub use native::{NotificationCleanup, NotificationError, SessionNotifications};

#[cfg(target_os = "windows")]
pub use native::{ChannelCleanupReport, JobError, JobProcessState, JobSpawnError, OwnedJobProcess};
#[cfg(target_os = "windows")]
pub use native::{
    ChannelDirection, ChannelFailure, Supervisor, SupervisorChannelCleanup, SupervisorCleanup,
    SupervisorConfigCleanup, SupervisorConfigResources, SupervisorError, SupervisorSpawnError,
    SupervisorStatus, TerminationReason, WORKER_ARGUMENT,
};
#[cfg(target_os = "windows")]
pub use native::{
    MixedWorker, Worker, WorkerAccessError, WorkerFault, WorkerInstallError, WorkerReply,
    WorkerReport, WorkerStep, WorkerStopReason,
};
#[cfg(target_os = "windows")]
pub use native::{PipeCloseReport, PipeReader, PipeWriter};
#[cfg(target_os = "windows")]
pub use native::{
    WorkerParentGuard, WorkerStartup, WorkerStartupError, check_synchronous_progress_pipe_rejected,
};

#[cfg(target_os = "windows")]
pub use native::{ControlIdentity, ControlSecurity};

#[cfg(windows)]
pub use native::{ControlMessagePipe, ControlPipeCleanup, ControlPipeCreateError};

#[cfg(windows)]
pub use native::ControlServerTransport;

#[cfg(windows)]
pub use control_identity::ControlServerInstance;
#[cfg(windows)]
pub use native::{ControlClientCleanup, ControlClientTransport};
