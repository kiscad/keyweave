//! Linux evdev/uinput protocol and owned input-query boundaries.
pub mod capture;
#[cfg(target_os = "linux")]
pub mod device;
pub mod emergency;
pub mod events;
#[cfg(target_os = "linux")]
mod ioctl;
pub mod keys;
pub mod output;
pub mod probe;
pub mod pump;
pub mod read;
pub mod runtime;
#[cfg(target_os = "linux")]
pub mod uinput;
