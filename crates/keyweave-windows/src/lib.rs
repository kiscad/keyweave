//! Windows keyboard semantics and explicit output. No Hook installation.
pub mod keys;
#[cfg(any(target_os = "windows", test))]
mod native;
pub mod pump;
#[cfg(target_os = "windows")]
pub use native::SendInputBackend;
