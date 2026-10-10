//! Host console control. OS handler uses process-lifetime atomics only.
use std::{
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConsoleError {
    #[error("console control is forbidden in keyboard callback")]
    InCallback,
    #[error("console control is already claimed for this process")]
    Claimed,
    #[error("private-console diagnostic requires no existing console")]
    Attached,
    #[error("unsupported private console event")]
    Event,
    #[error("{call} failed: {code}")]
    Api { call: &'static str, code: u32 },
    #[error("console diagnostic event producer failed")]
    Thread,
}
static CLAIMED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);
fn context() -> Result<(), ConsoleError> {
    if super::outside_callback() {
        Ok(())
    } else {
        Err(ConsoleError::InCallback)
    }
}
fn error(call: &'static str) -> ConsoleError {
    // SAFETY: immediate same-thread last-error scalar.
    ConsoleError::Api {
        call,
        code: unsafe { api::GetLastError() },
    }
}
fn attached() -> Result<bool, ConsoleError> {
    let mut pid = [0u32; 1];
    // SAFETY: live aligned writable one-DWORD buffer. Only return scalar is read;
    // a larger required count still means attached, not a buffer length to use.
    let n = unsafe { api::GetConsoleProcessList(pid.as_mut_ptr(), 1) };
    if n != 0 {
        return Ok(true);
    }
    let e = error("GetConsoleProcessList");
    if matches!(e, ConsoleError::Api { code: 6, .. }) {
        Ok(false)
    } else {
        Err(e)
    }
}
fn private_console_allowed() -> Result<(), ConsoleError> {
    context()?;
    if attached()? {
        Err(ConsoleError::Attached)
    } else {
        Ok(())
    }
}
fn signal_event(event: u32) -> Result<u32, ConsoleError> {
    if event <= 1 {
        Ok(event)
    } else {
        Err(ConsoleError::Event)
    }
}
fn producer_result(
    result: std::thread::Result<Result<(), ConsoleError>>,
) -> Result<(), ConsoleError> {
    result.map_err(|_| ConsoleError::Thread)?
}
extern "system" fn handler(event: u32) -> i32 {
    if event == 0 || event == 1 {
        REQUESTED.store(true, Ordering::Release);
        1
    } else {
        0
    }
}
#[derive(Debug)]
pub struct ConsoleControl {
    registered: bool,
    available: bool,
    _thread: PhantomData<Rc<()>>,
}
impl ConsoleControl {
    pub fn install() -> Result<Self, ConsoleError> {
        context()?;
        if CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ConsoleError::Claimed);
        }
        let available = attached()?;
        if available {
            // SAFETY: NULL/FALSE enables CTRL+C only for this calling process;
            // no handler pointer or other process's console state is changed.
            if unsafe { api::SetConsoleCtrlHandler(None, 0) } == 0 {
                return Err(error("SetConsoleCtrlHandler enable"));
            }
            // SAFETY: static extern-system callback has no borrowed context; its
            // atomic storage lives until process exit, including delayed calls.
            if unsafe { api::SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
                return Err(error("SetConsoleCtrlHandler add"));
            }
        }
        Ok(Self {
            registered: available,
            available,
            _thread: PhantomData,
        })
    }
    pub fn available(&self) -> bool {
        self.available
    }
    pub fn requested(&self) -> bool {
        REQUESTED.load(Ordering::Acquire)
    }
    pub fn close(&mut self) -> Result<(), ConsoleError> {
        context()?;
        if self.registered {
            // SAFETY: exact previously registered static callback; no memory is
            // freed/reset, so callbacks already dispatched remain valid.
            if unsafe { api::SetConsoleCtrlHandler(Some(handler), 0) } == 0 {
                return Err(error("SetConsoleCtrlHandler remove"));
            }
            self.registered = false;
        }
        Ok(())
    }
}
impl Drop for ConsoleControl {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
#[cfg(not(test))]
mod api {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
        pub(super) fn GetConsoleProcessList(ids: *mut u32, size: u32) -> u32;
        pub(super) fn GetLastError() -> u32;
        pub(super) fn AllocConsole() -> i32;
        pub(super) fn FreeConsole() -> i32;
        pub(super) fn GetConsoleWindow() -> *mut std::ffi::c_void;
        pub(super) fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        pub(super) fn ShowWindow(window: *mut std::ffi::c_void, command: i32) -> i32;
    }
}
#[cfg(test)]
#[allow(non_snake_case)]
mod api {
    use std::cell::RefCell;
    #[derive(Default)]
    pub(super) struct Mock {
        pub fail_enable: bool,
        pub fail_add: bool,
        pub fail_remove: bool,
        pub presence: Option<u32>,
        pub error: u32,
        pub calls: Vec<&'static str>,
    }
    thread_local! {pub(super) static MOCK:RefCell<Mock>=RefCell::new(Mock::default());}
    pub(super) unsafe fn SetConsoleCtrlHandler(
        h: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32 {
        MOCK.with(|m| {
            let mut m = m.borrow_mut();
            if h.is_none() {
                assert_eq!(add, 0);
                m.calls.push("enable");
                return i32::from(!m.fail_enable);
            }
            m.calls.push(if add != 0 { "add" } else { "remove" });
            i32::from(if add != 0 {
                !m.fail_add
            } else {
                !m.fail_remove
            })
        })
    }
    pub(super) unsafe fn GetConsoleProcessList(_: *mut u32, size: u32) -> u32 {
        assert_eq!(size, 1);
        MOCK.with(|m| {
            let mut m = m.borrow_mut();
            m.calls.push("presence");
            m.presence.unwrap_or(1)
        })
    }
    pub(super) unsafe fn GetLastError() -> u32 {
        MOCK.with(|m| m.borrow().error)
    }
}
#[cfg(not(test))]
mod diagnostics;
#[cfg(test)]
mod tests;
#[cfg(all(windows, not(test)))]
pub use diagnostics::{ConsoleProbe, with_private_console_event};
