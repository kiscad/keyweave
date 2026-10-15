//! Explicit read-only library trust/availability diagnostic, no device access.
fn main() -> std::process::ExitCode {
    #[cfg(target_os = "linux")]
    match keyweave_linux::device_types::provider_available() {
        Ok(()) => {
            println!("libudev loader available; device classification/readiness unverified");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("libudev provider unavailable: {e}");
            std::process::ExitCode::FAILURE
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("libudev provider requires Linux");
        std::process::ExitCode::FAILURE
    }
}
