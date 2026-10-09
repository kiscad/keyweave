//! One-shot metadata-only diagnostic; never samples key state or enables capture.
#[cfg(target_os = "windows")]
fn main() -> std::process::ExitCode {
    match keyweave_windows::check_input_access() {
        Ok(()) => {
            println!("input_access=observed");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("input_access=unavailable: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(not(target_os = "windows"))]
fn main() -> std::process::ExitCode {
    eprintln!("check_access requires Windows");
    std::process::ExitCode::from(2)
}
