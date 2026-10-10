//! Linux evdev/uinput protocol and owned input-query boundaries.
pub mod activation;
#[cfg(target_os = "linux")]
pub mod bootstrap;
pub mod capture;
#[cfg(target_os = "linux")]
mod clock_continuity;
#[cfg(target_os = "linux")]
pub mod config_artifact;
#[cfg(target_os = "linux")]
pub mod console;
#[cfg(target_os = "linux")]
pub mod device;
#[cfg(target_os = "linux")]
pub mod discovery;
pub mod emergency;
pub mod events;
#[cfg(target_os = "linux")]
pub mod host_signals;
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
#[cfg(target_os = "linux")]
pub mod run_identity;
pub mod runtime;
#[cfg(target_os = "linux")]
pub mod supervisor;
pub mod termination;
#[cfg(target_os = "linux")]
pub mod uinput;
#[cfg(target_os = "linux")]
pub mod worker;

#[cfg(test)]
fn mock_syspath(value: &str) -> std::path::PathBuf {
    // Portable mock identities need host-absolute paths. Real Linux discovery
    // remains unchanged; no filesystem object or device is created here.
    if cfg!(target_os = "windows") && value.starts_with('/') {
        std::env::temp_dir()
            .join("keyweave-mock-sys")
            .join(value.trim_start_matches('/'))
    } else {
        value.into()
    }
}
