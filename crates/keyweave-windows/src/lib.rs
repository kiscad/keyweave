//! Windows keyboard semantics and explicit native integration. No automatic capture.
pub mod hook;
pub mod keys;
#[cfg(any(target_os = "windows", test))]
mod native;
pub mod pump;
pub mod reactor;
pub mod runtime;
pub mod startup;
#[cfg(target_os = "windows")]
pub use native::SendInputBackend;
#[cfg(target_os = "windows")]
pub use native::sample_input_state;
#[cfg(target_os = "windows")]
pub use native::{HookSession, HookSessionError, LoopStep};
