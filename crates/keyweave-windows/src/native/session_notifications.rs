//! Owned, same-thread WTS notification window. No key or account data.
use super::session_info::{SessionIdentity, own_session};
use crate::startup::ProbeError;
use std::{
    cell::Cell,
    ffi::c_void,
    marker::PhantomData,
    ptr::null_mut,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};
type Handle = *mut c_void;
type WindowProc = unsafe extern "system" fn(Handle, u32, usize, isize) -> isize;
#[repr(C)]
struct WindowClass {
    style: u32,
    procedure: Option<WindowProc>,
    class_extra: i32,
    window_extra: i32,
    module: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu: *const u16,
    name: *const u16,
}
thread_local! {
    static WINDOW: Cell<usize> = const { Cell::new(0) };
    static SESSION: Cell<u32> = const { Cell::new(0) };
    static LOST: Cell<bool> = const { Cell::new(false) };
    static POISONED: Cell<bool> = const { Cell::new(false) };
}
static NEXT_CLASS: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotificationCleanup {
    /// Window was destroyed outside this owner; its numeric HWND is not reused.
    pub unexpected_destroy: bool,
    pub unregister: Option<u32>,
    pub destroy: Option<u32>,
    pub class: Option<u32>,
}
impl NotificationCleanup {
    fn failed(self) -> bool {
        self != Self::default()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NotificationError {
    #[error(transparent)]
    Probe(#[from] ProbeError),
    #[error("notification thread is busy, poisoned or in a keyboard callback")]
    Busy,
    #[error("WTS notification changed the session during startup")]
    Changed { cleanup: NotificationCleanup },
    #[error("{call} failed: {code}; rollback: {cleanup:?}")]
    Api {
        call: &'static str,
        code: u32,
        cleanup: NotificationCleanup,
    },
}
#[cfg(not(test))]
mod api {
    use super::*;
    #[link(name = "user32")]
    unsafe extern "system" {
        pub(super) fn RegisterClassW(class: *const WindowClass) -> u16;
        pub(super) fn UnregisterClassW(class: *const u16, module: Handle) -> i32;
        pub(super) fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Handle,
            menu: Handle,
            module: Handle,
            parameter: *mut c_void,
        ) -> Handle;
        pub(super) fn DestroyWindow(window: Handle) -> i32;
        pub(super) fn DefWindowProcW(
            window: Handle,
            message: u32,
            wparam: usize,
            lparam: isize,
        ) -> isize;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn GetModuleHandleW(name: *const u16) -> Handle;
        pub(super) fn GetLastError() -> u32;
    }
    #[link(name = "wtsapi32")]
    unsafe extern "system" {
        pub(super) fn WTSRegisterSessionNotification(window: Handle, flags: u32) -> i32;
        pub(super) fn WTSUnRegisterSessionNotification(window: Handle) -> i32;
    }
}
use api::*;
fn last_error() -> u32 {
    // SAFETY: immediate thread-local error retrieval, no pointers or ownership.
    unsafe { GetLastError() }
}
/// Explicit resource acquisition; creates a hidden window but never a Hook.
/// Keep it on the creating thread and continue dispatching that thread's queue.
pub struct SessionNotifications {
    window: Handle,
    module: Handle,
    atom: u16,
    registered: bool,
    closed: bool,
    lost: bool,
    cleanup: NotificationCleanup,
    _thread: PhantomData<Rc<()>>,
}
/// Independent delivery gate, including reentry during output recovery.
pub(super) fn access_lost() -> bool {
    LOST.try_with(Cell::get).unwrap_or(true)
}
fn lose_access() {
    let first = LOST.try_with(|lost| !lost.replace(true)).unwrap_or(true);
    let _ = POISONED.try_with(|poisoned| poisoned.set(true));
    if first {
        super::hook_thread::session_changed();
    }
}
unsafe extern "system" fn procedure(
    window: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let owns = WINDOW
        .try_with(|owned| owned.get() != 0 && owned.get() == window as usize)
        .unwrap_or(false);
    if owns && message == 0x2b1 {
        let id = u32::try_from(lparam);
        // Invalid numeric payload cannot prove a different, harmless session.
        if id.is_err() || SESSION.try_with(|own| id == Ok(own.get())).unwrap_or(true) {
            let _ = wparam; // All own-session changes revoke; never auto resume.
            lose_access();
        }
        return 0;
    }
    if owns && message == 0x82 {
        // Unexpected destruction loses monitoring. Normal close clears WINDOW
        // before DestroyWindow, so its synchronous NCDESTROY cannot stop twice.
        lose_access();
        let _ = WINDOW.try_with(|owned| owned.set(0));
    }
    // SAFETY: forward the OS window arguments without dereferencing lParam;
    // class has no instance pointer or Rust user data, including during creation.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
impl SessionNotifications {
    pub fn install() -> Result<Self, NotificationError> {
        Self::install_for(own_session()?)
    }
    pub(super) fn install_for(session: SessionIdentity) -> Result<Self, NotificationError> {
        if !super::hook_thread::outside_callback()
            || WINDOW.with(Cell::get) != 0
            || POISONED.with(Cell::get)
        {
            return Err(NotificationError::Busy);
        }
        // SAFETY: borrowed current executable module, no ownership transfer.
        let module = unsafe { GetModuleHandleW(std::ptr::null()) };
        if module.is_null() {
            return Err(NotificationError::Api {
                call: "GetModuleHandleW",
                code: last_error(),
                cleanup: NotificationCleanup::default(),
            });
        }
        let number = NEXT_CLASS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| NotificationError::Busy)?;
        let name: Vec<u16> = format!("KeyWeave.SessionNotifications.{number}")
            .encode_utf16()
            .chain([0])
            .collect();
        let class = WindowClass {
            style: 0,
            procedure: Some(procedure),
            class_extra: 0,
            window_extra: 0,
            module,
            icon: null_mut(),
            cursor: null_mut(),
            background: null_mut(),
            menu: std::ptr::null(),
            name: name.as_ptr(),
        };
        // SAFETY: initialized SDK-checked WNDCLASSW and nul-terminated class name
        // are borrowed only for registration. The procedure is static system ABI.
        let atom = unsafe { RegisterClassW(&class) };
        if atom == 0 {
            return Err(NotificationError::Api {
                call: "RegisterClassW",
                code: last_error(),
                cleanup: NotificationCleanup::default(),
            });
        }
        let mut owned = Self {
            window: null_mut(),
            module,
            atom,
            registered: false,
            closed: false,
            lost: false,
            cleanup: NotificationCleanup::default(),
            _thread: PhantomData,
        };
        LOST.with(|lost| lost.set(false));
        SESSION.with(|id| id.set(session.id));
        // SAFETY: low-word class atom is MAKEINTATOM, never dereferenced by Rust;
        // no WS_VISIBLE, no parent/menu/instance pointer; zero-sized hidden
        // top-level window, WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW. Only this owned
        // class/module is used, and synchronous creation callbacks are static.
        owned.window = unsafe {
            CreateWindowExW(
                0x08000080,
                atom as usize as *const u16,
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                module,
                null_mut(),
            )
        };
        if owned.window.is_null() {
            let code = last_error();
            let cleanup = owned.close();
            return Err(NotificationError::Api {
                call: "CreateWindowExW",
                code,
                cleanup,
            });
        }
        WINDOW.with(|window| window.set(owned.window as usize));
        // SAFETY: live owned HWND on this same thread; THIS_SESSION=0, so no
        // subscriptions to other users. No borrow spans potential sent messages.
        if unsafe { WTSRegisterSessionNotification(owned.window, 0) } == 0 {
            let code = last_error();
            let cleanup = owned.close();
            return Err(NotificationError::Api {
                call: "WTSRegisterSessionNotification",
                code,
                cleanup,
            });
        }
        owned.registered = true;
        if owned.access_lost() {
            let cleanup = owned.close();
            return Err(NotificationError::Changed { cleanup });
        }
        Ok(owned)
    }
    pub fn access_lost(&self) -> bool {
        if self.closed {
            self.lost
        } else {
            access_lost()
        }
    }
    /// Idempotent cleanup. All failures retained; uncertain OS resources poison
    /// this thread instead of reusing handles. Drop never submits keyboard input.
    pub fn close(&mut self) -> NotificationCleanup {
        if self.closed {
            return self.cleanup;
        }
        self.closed = true;
        self.lost = access_lost();
        let live = !self.window.is_null() && WINDOW.with(Cell::get) == self.window as usize;
        self.cleanup.unexpected_destroy = !self.window.is_null() && !live;
        if self.registered && live {
            self.registered = false;
            // SAFETY: exactly one matching successful registration on this HWND;
            // attempt unregister before destruction, same !Send creating thread.
            if unsafe { WTSUnRegisterSessionNotification(self.window) } == 0 {
                self.cleanup.unregister = Some(last_error());
            }
        }
        // Unregistration can deliver a final sent notification: retain its latch.
        self.lost |= access_lost();
        // Unregister may deliver NCDESTROY too; reacquire the scalar evidence
        // after that API before touching a HWND that may already be invalid.
        let live = !self.window.is_null() && WINDOW.with(Cell::get) == self.window as usize;
        self.cleanup.unexpected_destroy |= !self.window.is_null() && !live;
        self.registered = false;
        WINDOW.with(|window| window.set(0));
        SESSION.with(|id| id.set(0));
        if !self.window.is_null() && live {
            // SAFETY: unique HWND; detached TLS first, procedure uses no instance
            // pointer. An unexpected NCDESTROY already removed it: don't reuse.
            if unsafe { DestroyWindow(self.window) } == 0 {
                self.cleanup.destroy = Some(last_error());
            }
        }
        self.window = null_mut();
        if self.cleanup.destroy.is_none() {
            // SAFETY: owned class atom/module; all known windows now destroyed.
            if unsafe { UnregisterClassW(self.atom as usize as *const u16, self.module) } == 0 {
                self.cleanup.class = Some(last_error());
            }
        }
        if self.cleanup.failed() {
            POISONED.with(|poisoned| poisoned.set(true));
        }
        self.cleanup
    }
}
impl Drop for SessionNotifications {
    fn drop(&mut self) {
        self.close();
    }
}
#[cfg(test)]
pub(super) mod api;
#[cfg(test)]
mod tests;
