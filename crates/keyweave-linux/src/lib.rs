//! Linux evdev/uinput protocol and owned input-query boundaries.
#[cfg(target_os = "linux")]
pub mod device;
pub mod events;
#[cfg(target_os = "linux")]
mod ioctl;
pub mod keys;
pub mod probe;
pub mod read;
