//! Test Win32 replacement: no OS APIs, keyboard capture or input injection.
#![allow(non_snake_case)]
use super::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};
pub(super) struct Mock {
    pub visible: bool,
    pub active: bool,
    pub elevated: bool,
    pub impersonating: bool,
    pub deny_desktop: bool,
    pub bad_name: bool,
    pub bad_sid: bool,
    pub large_token: bool,
    pub foreground_changed: bool,
    pub foreground_flip: bool,
    pub foreground_reads: usize,
    pub fail_label: bool,
    pub fail_close: bool,
    pub key: i16,
    pub key_reads: usize,
    pub closed: Vec<(bool, usize)>,
    error: u32,
    next_desktop: usize,
    next_token: usize,
    desktops: BTreeSet<usize>,
    handles: BTreeSet<usize>,
    tokens: BTreeMap<usize, bool>,
}
impl Default for Mock {
    fn default() -> Self {
        Self {
            visible: true,
            active: true,
            elevated: false,
            impersonating: false,
            deny_desktop: false,
            bad_name: false,
            bad_sid: false,
            large_token: false,
            foreground_changed: false,
            foreground_flip: false,
            foreground_reads: 0,
            fail_label: false,
            fail_close: false,
            key: 0,
            key_reads: 0,
            closed: vec![],
            error: 0,
            next_desktop: 10,
            next_token: 100,
            desktops: BTreeSet::new(),
            handles: BTreeSet::new(),
            tokens: BTreeMap::new(),
        }
    }
}
impl Mock {
    pub(super) fn live_handles(&self) -> usize {
        self.handles.len() + self.desktops.len()
    }
}
thread_local! { pub(super) static MOCK: RefCell<Mock> = RefCell::new(Mock::default()); }
pub(super) unsafe fn GetLastError() -> u32 {
    MOCK.with(|mock| mock.borrow().error)
}
pub(super) unsafe fn GetCurrentThreadId() -> u32 {
    1
}
pub(super) unsafe fn GetCurrentThread() -> Handle {
    (-2isize) as Handle
}
pub(super) unsafe fn GetCurrentProcess() -> Handle {
    (-1isize) as Handle
}
pub(super) unsafe fn GetCurrentProcessId() -> u32 {
    200
}
pub(super) unsafe fn GetProcessWindowStation() -> Handle {
    1usize as Handle
}
pub(super) unsafe fn GetThreadDesktop(_: u32) -> Handle {
    2usize as Handle
}
pub(super) unsafe fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> Handle {
    assert_eq!((flags, inherit, access), (0, 0, 9));
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        if mock.deny_desktop {
            mock.error = 5;
            return null_mut();
        }
        let handle = mock.next_desktop;
        mock.next_desktop += 1;
        mock.desktops.insert(handle);
        handle as Handle
    })
}
pub(super) unsafe fn CloseDesktop(handle: Handle) -> i32 {
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        assert!(mock.desktops.remove(&(handle as usize)));
        mock.closed.push((true, handle as usize));
    });
    1
}
pub(super) unsafe fn CloseHandle(handle: Handle) -> i32 {
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        assert!(mock.handles.contains(&(handle as usize)));
        mock.closed.push((false, handle as usize));
        if mock.fail_close {
            mock.error = 5;
            return 0;
        }
        mock.handles.remove(&(handle as usize));
        mock.tokens.remove(&(handle as usize));
        1
    })
}
pub(super) unsafe fn GetUserObjectInformationW(
    object: Handle,
    index: i32,
    data: *mut c_void,
    length: u32,
    needed: *mut u32,
) -> i32 {
    let object = object as usize;
    MOCK.with(|mock| {
        let mock = mock.borrow();
        // SAFETY: the tested caller supplies live aligned output structs/arrays
        // and DWORD length; sizes below are asserted before writing each kind.
        unsafe {
            match index {
                1 => {
                    assert_eq!(length, 12);
                    data.cast::<UserFlags>().write(UserFlags {
                        inherit: 0,
                        reserved: 0,
                        flags: u32::from(mock.visible),
                    });
                    needed.write(12);
                }
                6 => {
                    assert_eq!(length, 4);
                    data.cast::<i32>().write(i32::from(mock.active));
                    needed.write(4);
                }
                2 => {
                    assert_eq!(length, 128);
                    let words = std::slice::from_raw_parts_mut(data.cast::<u16>(), 64);
                    let text = if object == 1 { "WinSta0" } else { "Default" };
                    for (out, word) in words.iter_mut().zip(text.encode_utf16()) {
                        *out = word;
                    }
                    words[text.len()] = 0;
                    needed.write(if mock.bad_name {
                        130
                    } else {
                        (text.len() as u32 + 1) * 2
                    });
                }
                _ => panic!("unexpected USER info"),
            }
        }
        1
    })
}
pub(super) unsafe fn GetForegroundWindow() -> Handle {
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        mock.foreground_reads += 1;
        if (mock.foreground_changed && mock.key_reads != 0)
            || (mock.foreground_flip && mock.foreground_reads % 2 == 0)
        {
            21usize as Handle
        } else {
            20usize as Handle
        }
    })
}
pub(super) unsafe fn GetWindowThreadProcessId(_: Handle, process: *mut u32) -> u32 {
    // SAFETY: tested caller's initialized DWORD output remains live for call.
    unsafe {
        process.write(100);
    }
    2
}
pub(super) unsafe fn OpenProcess(access: u32, inherit: i32, _: u32) -> Handle {
    assert_eq!((access, inherit), (0x1000, 0));
    MOCK.with(|mock| {
        assert!(mock.borrow_mut().handles.insert(4));
    });
    4usize as Handle
}
pub(super) unsafe fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32 {
    assert_eq!(access, 8);
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        let handle = mock.next_token;
        mock.next_token += 1;
        mock.handles.insert(handle);
        mock.tokens.insert(handle, process as isize == -1);
        // SAFETY: tested caller provides writable HANDLE output.
        unsafe {
            token.write(handle as Handle);
        }
    });
    1
}
pub(super) unsafe fn OpenThreadToken(
    _: Handle,
    access: u32,
    as_self: i32,
    token: *mut Handle,
) -> i32 {
    assert_eq!((access, as_self), (8, 1));
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        if !mock.impersonating {
            mock.error = 1008;
            return 0;
        }
        let handle = mock.next_token;
        mock.next_token += 1;
        mock.handles.insert(handle);
        // SAFETY: tested caller provides writable HANDLE output.
        unsafe {
            token.write(handle as Handle);
        }
        1
    })
}
pub(super) unsafe fn GetTokenInformation(
    token: Handle,
    class: i32,
    data: *mut c_void,
    length: u32,
    needed: *mut u32,
) -> i32 {
    assert_eq!(class, 25);
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        let used = size_of::<Label>() + 12;
        // SAFETY: caller provides a live DWORD output, and on the second call
        // a 256-byte aligned initialized buffer; no write exceeds that capacity.
        unsafe {
            needed.write(if mock.large_token { 300 } else { used as u32 });
            if data.is_null() {
                assert_eq!(length, 0);
                mock.error = 122;
                return 0;
            }
            assert_eq!(length, 256);
            if mock.fail_label {
                mock.error = 5;
                return 0;
            }
            let sid_ptr = data.cast::<u8>().add(size_of::<Label>());
            data.cast::<Label>().write(Label {
                sid: if mock.bad_sid {
                    usize::MAX
                } else {
                    sid_ptr as usize
                },
                attributes: 0x20,
            });
            let sid = std::slice::from_raw_parts_mut(sid_ptr, 12);
            sid.copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0]);
            let own = mock.tokens[&(token as usize)];
            let rid: u32 = if !own && mock.elevated {
                0x3000
            } else {
                0x2000
            };
            sid[8..12].copy_from_slice(&rid.to_le_bytes());
        }
        1
    })
}
pub(super) unsafe fn GetAsyncKeyState(key: i32) -> i16 {
    assert!((1..=254).contains(&key));
    MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        mock.key_reads += 1;
        if key == 65 { mock.key } else { 0 }
    })
}
