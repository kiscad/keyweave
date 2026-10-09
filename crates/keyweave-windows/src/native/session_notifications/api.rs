#![allow(non_snake_case, clippy::too_many_arguments)]
use super::*;
use std::cell::RefCell;
#[derive(Default)]
pub(super) struct Mock {
    pub(super) fail: Option<&'static str>,
    pub(super) fail_also: Option<&'static str>,
    pub(super) calls: Vec<&'static str>,
    pub(super) register_event: Option<(usize, isize)>,
    pub(super) unregister_event: Option<(usize, isize)>,
    pub(super) destroy_during_unregister: bool,
}
thread_local! { pub(super) static MOCK: RefCell<Mock> = RefCell::new(Mock::default()); }
fn call(name: &'static str) -> bool {
    MOCK.with(|mock| {
        let mut m = mock.borrow_mut();
        m.calls.push(name);
        m.fail != Some(name) && m.fail_also != Some(name)
    })
}
pub(super) unsafe fn GetModuleHandleW(_: *const u16) -> Handle {
    if call("module") {
        1usize as Handle
    } else {
        null_mut()
    }
}
pub(super) unsafe fn GetLastError() -> u32 {
    5
}
pub(super) unsafe fn RegisterClassW(class: *const WindowClass) -> u16 {
    // SAFETY: tested caller supplies the initialized class for synchronous use.
    let class = unsafe { &*class };
    assert_eq!(class.style, 0);
    assert!(class.procedure.is_some());
    assert!(!class.name.is_null());
    assert!(class.menu.is_null());
    if call("class") { 7 } else { 0 }
}
pub(super) unsafe fn CreateWindowExW(
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
) -> Handle {
    assert_eq!(ex, 0x08000080);
    assert_eq!(class as usize, 7);
    assert!(name.is_null());
    assert_eq!((style, x, y, width, height), (0, 0, 0, 0, 0));
    assert!(parent.is_null() && menu.is_null() && parameter.is_null());
    assert_eq!(module as usize, 1);
    if call("window") {
        4096usize as Handle
    } else {
        null_mut()
    }
}
pub(super) unsafe fn WTSRegisterSessionNotification(window: Handle, flags: u32) -> i32 {
    assert_eq!(window as usize, 4096);
    assert_eq!(flags, 0);
    let success = call("register");
    let event = MOCK.with(|mock| mock.borrow_mut().register_event.take());
    if let Some((event, id)) = event {
        // SAFETY: scalar test notification through the exact native WindowProc.
        unsafe {
            procedure(window, 0x2b1, event, id);
        }
    }
    i32::from(success)
}
pub(super) unsafe fn WTSUnRegisterSessionNotification(window: Handle) -> i32 {
    assert_eq!(window as usize, 4096);
    let success = call("unregister");
    let event = MOCK.with(|mock| mock.borrow_mut().unregister_event.take());
    if let Some((event, id)) = event {
        // SAFETY: callback runs outside mock borrow, like synchronous Win32.
        unsafe {
            procedure(window, 0x2b1, event, id);
        }
    }
    if MOCK.with(|mock| mock.borrow_mut().destroy_during_unregister) {
        // SAFETY: scalar lifecycle event, with no test-state borrow held.
        unsafe {
            procedure(window, 0x82, 0, 0);
        }
    }
    i32::from(success)
}
pub(super) unsafe fn DestroyWindow(window: Handle) -> i32 {
    assert_eq!(window as usize, 4096);
    if !call("destroy") {
        return 0;
    }
    // SAFETY: synchronous lifecycle callback with numeric-only arguments.
    unsafe {
        procedure(window, 0x82, 0, 0);
    }
    1
}
pub(super) unsafe fn UnregisterClassW(class: *const u16, module: Handle) -> i32 {
    assert_eq!(class as usize, 7);
    assert_eq!(module as usize, 1);
    i32::from(call("unclass"))
}
pub(super) unsafe fn DefWindowProcW(_: Handle, _: u32, _: usize, _: isize) -> isize {
    1
}
pub(crate) fn send(event: usize, id: isize) {
    let window = WINDOW.with(Cell::get) as Handle;
    // SAFETY: exact native entrypoint with scalar-only WTS notification.
    unsafe {
        procedure(window, 0x2b1, event, id);
    }
}

pub(crate) fn fail(name: &'static str) {
    MOCK.with(|m| m.borrow_mut().fail = Some(name));
}
