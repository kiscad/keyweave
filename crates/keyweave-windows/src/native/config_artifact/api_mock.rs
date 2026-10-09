#![allow(non_snake_case, clippy::too_many_arguments, clippy::unwrap_used)]
use super::*;
use std::cell::RefCell;
#[derive(Default)]
pub(crate) struct Mock {
    pub fail: Option<&'static str>,
    pub calls: Vec<&'static str>,
    pub data: Vec<u8>,
    pub closes: Vec<usize>,
    pub flags: u32,
    pub file_type: Option<u32>,
    pub drive: Option<u32>,
    pub acl: Option<u32>,
    pub final_path: Option<String>,
    pub chunk: Option<usize>,
    pub bad_write: bool,
    pub bad_read: bool,
    pub close_fail: bool,
    pub dirty_failure_output: bool,
    pub cursor: usize,
}
thread_local! {pub(crate) static MOCK:RefCell<Mock>=RefCell::new(Mock::default());}
fn call(name: &'static str) -> bool {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls.push(name);
        m.fail != Some(name)
    })
}
unsafe fn wide(p: *const u16) -> String {
    let mut v = vec![];
    for n in 0..PATH_CAP {
        // SAFETY: tested caller passes a terminated bounded UTF16 input.
        let x = unsafe { *p.add(n) };
        if x == 0 {
            break;
        }
        v.push(x);
    }
    String::from_utf16(&v).unwrap()
}
unsafe fn copy_text(p: *mut u16, capacity: u32, text: &str) -> u32 {
    let v: Vec<_> = text.encode_utf16().chain([0]).collect();
    assert!(v.len() <= capacity as usize);
    // SAFETY: tested caller supplies capacity-sized writable output; v disjoint.
    unsafe {
        std::ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
    }
    (v.len() - 1) as u32
}
pub(super) unsafe fn GetLastError() -> u32 {
    5
}
pub(super) unsafe fn BCryptGenRandom(a: Handle, b: *mut u8, n: u32, f: u32) -> i32 {
    assert!(a.is_null());
    assert_eq!((n, f), (16, 2));
    if !call("rng") {
        return -1;
    }
    // SAFETY: fixed caller live 16-byte RNG buffer.
    unsafe {
        std::ptr::write_bytes(b, 0xab, 16);
    }
    0
}
pub(super) unsafe fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
    t: *const u16,
    r: u32,
    d: *mut *mut c_void,
    s: *mut u32,
) -> i32 {
    // SAFETY: tested caller provides a terminated SDDL literal.
    assert_eq!(unsafe { wide(t) }, "D:P(A;;GA;;;OW)(A;;GA;;;SY)");
    assert_eq!(r, 1);
    assert!(s.is_null());
    if !call("security") {
        if MOCK.with(|m| m.borrow().dirty_failure_output) {
            // SAFETY: caller's live output; intentionally unspecified on failure.
            unsafe {
                *d = 0xdeadusize as *mut c_void;
            }
        }
        return 0;
    }
    // SAFETY: tested caller live output; fake descriptor never dereferenced.
    unsafe {
        *d = 0x4321usize as *mut c_void;
    }
    1
}
pub(super) unsafe fn LocalFree(p: *mut c_void) -> *mut c_void {
    assert_eq!(p as usize, 0x4321);
    if call("free-security") { null_mut() } else { p }
}
pub(super) unsafe fn CreateFileW(
    p: *const u16,
    a: u32,
    s: u32,
    sa: *const SecurityAttributes,
    c: u32,
    f: u32,
    t: Handle,
) -> Handle {
    // SAFETY: live caller terminated name and security struct.
    let name = unsafe { wide(p) };
    let sa = unsafe { &*sa };
    assert!(name.ends_with("keyweave-config-abababababababababababababababab.jsonc"));
    assert_eq!((a, s, c, f), (0xc0010000, 1, 1, 0x04000100));
    assert!(t.is_null());
    assert_eq!(
        sa.length as usize,
        std::mem::size_of::<SecurityAttributes>()
    );
    assert_eq!(sa.inherit, 0);
    assert_eq!(sa.descriptor as usize, 0x4321);
    if call("create") {
        0x100usize as Handle
    } else {
        (-1isize) as Handle
    }
}
pub(super) unsafe fn CloseHandle(h: Handle) -> i32 {
    if !call("close") || MOCK.with(|m| m.borrow().close_fail) {
        return 0;
    }
    MOCK.with(|m| m.borrow_mut().closes.push(h as usize));
    1
}
pub(super) unsafe fn GetFileType(_: Handle) -> u32 {
    call("type");
    MOCK.with(|m| m.borrow().file_type.unwrap_or(1))
}
pub(super) unsafe fn GetHandleInformation(_: Handle, p: *mut u32) -> i32 {
    if !call("flags") {
        return 0;
    }
    // SAFETY: tested caller supplies a live DWORD output.
    unsafe {
        *p = MOCK.with(|m| m.borrow().flags);
    }
    1
}
pub(super) unsafe fn GetFinalPathNameByHandleW(_: Handle, p: *mut u16, c: u32, f: u32) -> u32 {
    assert_eq!((c, f), (PATH_CAP as u32, 0));
    if !call("final-path") {
        return 0;
    }
    let text = MOCK.with(|m| {
        m.borrow()
            .final_path
            .clone()
            .unwrap_or_else(|| "\\\\?\\C:\\mock\\sealed.jsonc".into())
    });
    // SAFETY: tested caller output capacity checked by copy helper.
    unsafe { copy_text(p, c, &text) }
}
pub(super) unsafe fn GetVolumePathNameW(_: *const u16, p: *mut u16, c: u32) -> i32 {
    if !call("volume-path") {
        return 0;
    }
    // SAFETY: tested caller output capacity checked by helper.
    unsafe {
        copy_text(p, c, "C:\\");
    }
    1
}
pub(super) unsafe fn GetDriveTypeW(_: *const u16) -> u32 {
    call("drive");
    MOCK.with(|m| m.borrow().drive.unwrap_or(3))
}
pub(super) unsafe fn GetVolumeInformationW(
    _: *const u16,
    n: *mut u16,
    nc: u32,
    s: *mut u32,
    mc: *mut u32,
    f: *mut u32,
    fs: *mut u16,
    fc: u32,
) -> i32 {
    assert!(n.is_null() && s.is_null() && mc.is_null() && fs.is_null());
    assert_eq!((nc, fc), (0, 0));
    if !call("volume-info") {
        return 0;
    }
    // SAFETY: tested caller supplies a live flags DWORD.
    unsafe {
        *f = MOCK.with(|m| m.borrow().acl.unwrap_or(8));
    }
    1
}
pub(super) unsafe fn WriteFile(
    _: Handle,
    b: *const c_void,
    n: u32,
    p: *mut u32,
    o: *mut c_void,
) -> i32 {
    assert!(o.is_null());
    assert!(n as usize <= CHUNK);
    if !call("write") {
        return 0;
    }
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        let count = (n as usize).min(m.chunk.unwrap_or(CHUNK));
        // SAFETY: caller provides n readable bytes/count output; no alias to mock.
        let bytes = unsafe { std::slice::from_raw_parts(b.cast::<u8>(), count) };
        m.data.extend_from_slice(bytes);
        m.cursor += count;
        // SAFETY: count is a live output; bad_count only tests validation, no overrun.
        unsafe {
            *p = if m.bad_write { n + 1 } else { count as u32 };
        }
    });
    1
}
pub(super) unsafe fn ReadFile(
    _: Handle,
    b: *mut c_void,
    n: u32,
    p: *mut u32,
    o: *mut c_void,
) -> i32 {
    assert!(o.is_null());
    assert!(n as usize <= CHUNK);
    if !call("read") {
        return 0;
    }
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        let count = (n as usize).min(m.data.len() - m.cursor);
        // SAFETY: live writable caller buffer n; disjoint owned mock data/count.
        unsafe {
            std::ptr::copy_nonoverlapping(m.data[m.cursor..].as_ptr(), b.cast::<u8>(), count);
            *p = if m.bad_read { n + 1 } else { count as u32 };
        }
        m.cursor += count;
    });
    1
}
pub(super) unsafe fn DuplicateHandle(
    a: Handle,
    h: Handle,
    t: Handle,
    p: *mut Handle,
    access: u32,
    inherit: i32,
    flags: u32,
) -> i32 {
    assert_eq!((a as isize, t as isize), (-1, -1));
    assert_eq!(h as usize, 0x100);
    assert_eq!((access, inherit, flags), (0x80000000, 0, 0));
    if !call("duplicate") {
        return 0;
    }
    // SAFETY: caller live handle output, fake owned read-only handle.
    unsafe {
        *p = 0x200usize as Handle;
    }
    1
}
pub(super) unsafe fn SetFilePointerEx(
    _: Handle,
    d: LargeInteger,
    p: *mut LargeInteger,
    o: u32,
) -> i32 {
    // SAFETY: caller initializes union quad to0.
    assert_eq!(unsafe { d.quad }, 0);
    assert!(p.is_null());
    assert_eq!(o, 0);
    if !call("rewind") {
        return 0;
    }
    MOCK.with(|m| m.borrow_mut().cursor = 0);
    1
}
