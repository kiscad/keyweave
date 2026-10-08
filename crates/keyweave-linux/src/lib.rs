//! Linux evdev/uinput protocol and owned input-query boundaries.
pub mod activation;
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
pub mod progress;
pub mod pump;
#[cfg(target_os = "linux")]
pub mod reactor;
pub mod read;
pub mod runtime;
#[cfg(target_os = "linux")]
pub mod supervisor;
#[cfg(target_os = "linux")]
pub mod uinput;
#[cfg(target_os = "linux")]
pub mod worker;
