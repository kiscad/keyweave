//! Windows keyboard semantics and explicit native integration. No automatic capture.
pub mod hook;
pub mod keys;
#[cfg(any(target_os = "windows", test))]
mod native;
pub mod pump;
#[cfg(target_os = "windows")]
pub use native::SendInputBackend;
#[cfg(target_os = "windows")]
pub use native::{HookSession, HookSessionError};
