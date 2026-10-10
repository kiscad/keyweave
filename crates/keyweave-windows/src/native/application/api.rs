#![allow(non_snake_case)]
use super::*;
use std::cell::RefCell;
pub(super) struct Mock {
    pub path: Vec<u16>,
    pub count: Option<u32>,
    pub terminate: bool,
    pub open: bool,
    pub query: bool,
    pub close: bool,
    pub changed: bool,
    pub foreground_calls: usize,
    pub queries: usize,
    pub closes: usize,
    pub live: bool,
}
impl Default for Mock {
    fn default() -> Self {
        Self {
            path: "C:\\Apps\\editor.exe".encode_utf16().collect(),
            count: None,
            terminate: true,
            open: true,
            query: true,
            close: true,
            changed: false,
            foreground_calls: 0,
            queries: 0,
            closes: 0,
            live: false,
        }
    }
}
thread_local! { pub(super) static MOCK: RefCell<Mock> = RefCell::new(Mock::default()); }
pub(super) unsafe fn GetForegroundWindow() -> Handle {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.foreground_calls += 1;
        if m.changed && m.foreground_calls > 1 {
            2usize as Handle
        } else {
            1usize as Handle
        }
    })
}
pub(super) unsafe fn GetWindowThreadProcessId(_: Handle, pid: *mut u32) -> u32 {
    // SAFETY: initialized caller DWORD output.
    unsafe {
        pid.write(42);
    }
    10
}
pub(super) unsafe fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle {
    assert_eq!((access, inherit, pid), (0x1000, 0, 42));
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !m.open {
            return null_mut();
        }
        assert!(!m.live);
        m.live = true;
        3usize as Handle
    })
}
pub(super) unsafe fn QueryFullProcessImageNameW(
    handle: Handle,
    flags: u32,
    buffer: *mut u16,
    count: *mut u32,
) -> i32 {
    assert_eq!((handle as usize, flags), (3, 0));
    // SAFETY: caller initialized count and full CAPACITY array.
    assert_eq!(unsafe { count.read() }, CAPACITY as u32);
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        assert!(m.live);
        m.queries += 1;
        if !m.query {
            return 0;
        }
        let len = m.path.len().min(CAPACITY - 1);
        // SAFETY: len bounded by caller array; disjoint mock data; live count.
        unsafe {
            std::ptr::copy_nonoverlapping(m.path.as_ptr(), buffer, len);
            buffer.add(len).write(if m.terminate { 0 } else { 7 });
            count.write(m.count.unwrap_or(m.path.len() as u32));
        }
        1
    })
}
pub(super) unsafe fn CloseHandle(handle: Handle) -> i32 {
    assert_eq!(handle as usize, 3);
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        assert!(m.live);
        m.closes += 1;
        if m.close {
            m.live = false;
            1
        } else {
            0
        }
    })
}
pub(super) unsafe fn GetLastError() -> u32 {
    5
}
