//! Only own private console; never detach/reset a user's existing console.
use super::*;
#[derive(Debug)]
pub struct ConsoleProbe<T> {
    pub body: T,
    pub delivery: Result<(), ConsoleError>,
    pub unregister: Result<(), ConsoleError>,
    pub release: Result<(), ConsoleError>,
}
impl<T> ConsoleProbe<T> {
    pub fn complete(&self) -> bool {
        self.delivery.is_ok() && self.unregister.is_ok() && self.release.is_ok()
    }
}
struct PrivateConsole {
    open: bool,
}
impl PrivateConsole {
    fn create() -> Result<Self, ConsoleError> {
        private_console_allowed()?;
        if CLAIMED.load(Ordering::Acquire) {
            return Err(ConsoleError::Claimed);
        }
        // SAFETY: caller was verified not attached; allocate only this process's
        // fresh private console, no user console is detached or joined.
        if unsafe { api::AllocConsole() } == 0 {
            return Err(error("AllocConsole"));
        }
        let owner = Self { open: true };
        // SAFETY: window is for the new own console only; no enumeration or
        // foreground-window selection. Hidden immediately, never input injected.
        let window = unsafe { api::GetConsoleWindow() };
        if !window.is_null() {
            unsafe { api::ShowWindow(window, 0) };
        }
        Ok(owner)
    }
    fn close(&mut self) -> Result<(), ConsoleError> {
        if self.open {
            // SAFETY: release only the new private console owned by this scope;
            // producer has joined before explicit close and no host body remains.
            if unsafe { api::FreeConsole() } == 0 {
                return Err(error("FreeConsole"));
            }
            self.open = false;
        }
        Ok(())
    }
}
impl Drop for PrivateConsole {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
/// Diagnostic only: call in a fresh no-console process. The body must not change
/// console attachment/table; it may supervise only an explicitly trusted fixture.
/// Events are generated inside the new private console, never an existing one.
pub fn with_private_console_event<T>(
    event: u32,
    body: impl FnOnce(&ConsoleControl) -> T,
) -> Result<ConsoleProbe<T>, ConsoleError> {
    let event = signal_event(event)?;
    let mut private = PrivateConsole::create()?;
    let mut control = ConsoleControl::install()?;
    let producer = std::thread::Builder::new()
        .name("keyweave-private-console-event".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(750));
            // SAFETY: this process created/keeps its sole private console, the body
            // must not reattach; group0 reaches only this new console's members.
            // Detached fixture worker cannot receive this broadcast directly.
            if unsafe { api::GenerateConsoleCtrlEvent(event, 0) } == 0 {
                return Err(error("GenerateConsoleCtrlEvent"));
            }
            Ok(())
        })
        .map_err(|_| ConsoleError::Thread)?;
    let result = body(&control);
    let delivery = producer_result(producer.join());
    let unregister = control.close();
    let release = private.close();
    Ok(ConsoleProbe {
        body: result,
        delivery,
        unregister,
        release,
    })
}
