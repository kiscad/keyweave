//! Explicit notification resource smoke; never installs a keyboard Hook.
#[cfg(target_os = "windows")]
fn main() {
    match keyweave_windows::SessionNotifications::install() {
        Ok(mut window) => {
            let cleanup = window.close();
            if cleanup != keyweave_windows::NotificationCleanup::default() || window.access_lost() {
                eprintln!(
                    "session_notifications=unavailable: cleanup={cleanup:?}, changed={}",
                    window.access_lost()
                );
                std::process::exit(1);
            }
            println!("session_notifications=registered_and_closed");
        }
        Err(error) => {
            eprintln!("session_notifications=unavailable: {error}");
            std::process::exit(1);
        }
    }
}
#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("session_notifications=requires_windows");
    std::process::exit(2);
}
