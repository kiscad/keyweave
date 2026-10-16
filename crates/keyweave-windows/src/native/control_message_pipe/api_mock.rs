#![allow(non_snake_case, clippy::too_many_arguments, clippy::unwrap_used)]
use super::*;
use std::{cell::RefCell, collections::BTreeMap};
#[derive(Clone, Copy, Default)]
pub(in crate::native) enum Mode {
    #[default]
    Immediate,
    Pending,
    Error(u32),
}
#[derive(Clone, Copy)]
struct Pending {
    overlap: usize,
    buffer: usize,
    count: usize,
    kind: Kind,
    event: usize,
}
#[derive(Default)]
pub(in crate::native) struct Mock {
    pub server_pid: Option<u32>,
    pub open_error: Option<u32>,
    pub fail: Vec<&'static str>,
    pub calls: Vec<&'static str>,
    pub closes: Vec<usize>,
    pub error: u32,
    pub connect: Mode,
    pub read: Mode,
    pub write: Mode,
    pub completion: Option<(u32, u32)>,
    pub payload: Vec<u8>,
    pub writes: Vec<Vec<u8>>,
    pub cancel_code: Option<u32>,
    pub defer_cancel: bool,
    pub signal_errors: bool,
    pub flags: Option<u32>,
    pub mode: Option<u32>,
    pub inheritance: u32,
    pub immediate_count: Option<u32>,
    next: usize,
    handles: BTreeMap<usize, bool>,
    events: BTreeMap<usize, bool>,
    pending: BTreeMap<usize, Pending>,
}
thread_local! {pub(super) static MOCK:RefCell<Mock>=RefCell::new(Mock {signal_errors:true,..Mock::default()});}
fn call(m: &mut Mock, name: &'static str) -> bool {
    m.calls.push(name);
    if m.fail.contains(&name) {
        m.error = 5;
        false
    } else {
        true
    }
}
pub(super) unsafe fn GetLastError() -> u32 {
    MOCK.with(|m| m.borrow().error)
}
pub(super) unsafe fn CreateNamedPipeW(
    n: *const u16,
    o: u32,
    mode: u32,
    instances: u32,
    out: u32,
    input: u32,
    timeout: u32,
    a: *const SecurityAttributes,
) -> Handle {
    assert_eq!(
        (o, mode, instances, out, input, timeout),
        (0x4008_0003, 14, 1, 128, 128, 0)
    );
    let a = unsafe { &*a };
    assert_eq!(a.length, size_of::<SecurityAttributes>() as u32);
    assert!(!a.descriptor.is_null());
    assert_eq!(a.inherit, 0);
    assert_eq!(unsafe { *n }, b'\\' as u16);
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "create-server") {
            return usize::MAX as Handle;
        }
        m.next += 1;
        let h = m.next + 100;
        m.handles.insert(h, true);
        h as Handle
    })
}
pub(super) unsafe fn CreateFileW(
    n: *const u16,
    access: u32,
    share: u32,
    a: *const SecurityAttributes,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    assert!(!n.is_null());
    assert_eq!(
        (access, share, disposition, flags),
        (0xc000_0000, 0, 3, 0x4011_0000)
    );
    assert!(a.is_null() && template.is_null());
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "open-client") {
            if let Some(error) = m.open_error {
                m.error = error;
            }
            return usize::MAX as Handle;
        }
        m.next += 1;
        let h = m.next + 100;
        m.handles.insert(h, false);
        h as Handle
    })
}
pub(super) unsafe fn CreateEventW(
    a: *const SecurityAttributes,
    manual: i32,
    initial: i32,
    name: *const u16,
) -> Handle {
    assert!(a.is_null() && name.is_null());
    assert_eq!((manual, initial), (1, 0));
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "create-event") {
            return null_mut();
        }
        m.next += 1;
        let h = m.next + 1000;
        m.events.insert(h, false);
        h as Handle
    })
}
pub(super) unsafe fn SetNamedPipeHandleState(
    h: Handle,
    mode: *const u32,
    count: *const u32,
    timeout: *const u32,
) -> i32 {
    assert_eq!(unsafe { *mode }, 2);
    assert!(count.is_null() && timeout.is_null());
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        assert!(!m.handles[&(h as usize)]);
        i32::from(call(&mut m, "set-mode"))
    })
}
pub(super) unsafe fn GetNamedPipeInfo(
    h: Handle,
    flags: *mut u32,
    out: *mut u32,
    input: *mut u32,
    instances: *mut u32,
) -> i32 {
    assert!(out.is_null() && input.is_null());
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "info") {
            return 0;
        }
        unsafe {
            *flags = m.flags.unwrap_or(12 | u32::from(m.handles[&(h as usize)]));
            *instances = 1;
        }
        1
    })
}
pub(super) unsafe fn GetNamedPipeHandleStateW(
    _: Handle,
    state: *mut u32,
    instances: *mut u32,
    count: *mut u32,
    timeout: *mut u32,
    user: *mut u16,
    length: u32,
) -> i32 {
    assert!(instances.is_null() && count.is_null() && timeout.is_null() && user.is_null());
    assert_eq!(length, 0);
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "state") {
            return 0;
        }
        unsafe {
            *state = m.mode.unwrap_or(2);
        }
        1
    })
}
pub(super) unsafe fn GetHandleInformation(_: Handle, flags: *mut u32) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "inheritance") {
            return 0;
        }
        unsafe {
            *flags = m.inheritance;
        }
        1
    })
}
pub(super) unsafe fn ResetEvent(event: Handle) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "reset") {
            return 0;
        }
        assert!(!m.pending.values().any(|p| p.event == event as usize));
        *m.events.get_mut(&(event as usize)).unwrap() = false;
        1
    })
}
fn issue(
    m: &mut Mock,
    h: Handle,
    kind: Kind,
    buffer: usize,
    count: usize,
    bytes: *mut u32,
    overlap: *mut Overlapped,
    mode: Mode,
) -> i32 {
    assert!(m.handles.contains_key(&(h as usize)));
    assert!(!m.pending.contains_key(&(h as usize)));
    let event = unsafe { (*overlap).event } as usize;
    assert!(m.events.contains_key(&event));
    assert!(!m.events[&event]);
    match mode {
        Mode::Pending => {
            m.pending.insert(
                h as usize,
                Pending {
                    overlap: overlap as usize,
                    buffer,
                    count,
                    kind,
                    event,
                },
            );
            m.error = 997;
            0
        }
        Mode::Error(code) => {
            m.error = code;
            0
        }
        Mode::Immediate => {
            let length = if kind == Kind::Read {
                let length = m.payload.len().min(count);
                unsafe {
                    std::ptr::copy_nonoverlapping(m.payload.as_ptr(), buffer as *mut u8, length);
                }
                length
            } else {
                count
            };
            if !bytes.is_null() {
                unsafe {
                    *bytes = m.immediate_count.unwrap_or(length as u32);
                }
            }
            *m.events.get_mut(&event).unwrap() = true;
            if kind == Kind::Read && m.payload.len() > count {
                m.error = 234;
                0
            } else {
                1
            }
        }
    }
}
pub(super) unsafe fn ConnectNamedPipe(h: Handle, o: *mut Overlapped) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "connect") {
            return 0;
        }
        let mode = m.connect;
        issue(&mut m, h, Kind::Connect, 0, 0, null_mut(), o, mode)
    })
}
pub(super) unsafe fn ReadFile(
    h: Handle,
    b: *mut c_void,
    n: u32,
    bytes: *mut u32,
    o: *mut Overlapped,
) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "read") {
            return 0;
        }
        let mode = m.read;
        issue(
            &mut m,
            h,
            Kind::Read,
            b as usize,
            n as usize,
            bytes,
            o,
            mode,
        )
    })
}
pub(super) unsafe fn WriteFile(
    h: Handle,
    b: *const c_void,
    n: u32,
    bytes: *mut u32,
    o: *mut Overlapped,
) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "write") {
            return 0;
        }
        m.writes
            .push(unsafe { std::slice::from_raw_parts(b.cast::<u8>(), n as usize) }.to_vec());
        let mode = m.write;
        issue(
            &mut m,
            h,
            Kind::Write,
            b as usize,
            n as usize,
            bytes,
            o,
            mode,
        )
    })
}
pub(super) unsafe fn GetOverlappedResult(
    h: Handle,
    o: *mut Overlapped,
    bytes: *mut u32,
    wait: i32,
) -> i32 {
    assert_eq!(wait, 0);
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "result") {
            return 0;
        }
        let p = m.pending[&(h as usize)];
        assert_eq!(p.overlap, o as usize);
        let (code, length) = m.completion.take().unwrap_or((996, 0));
        if code != 996 && (code == 0 || m.signal_errors) {
            if p.kind == Kind::Read && matches!(code, 0 | 234) {
                let len = (length as usize).min(p.count).min(m.payload.len());
                unsafe {
                    std::ptr::copy_nonoverlapping(m.payload.as_ptr(), p.buffer as *mut u8, len);
                }
            }
            *m.events.get_mut(&p.event).unwrap() = true;
            m.pending.remove(&(h as usize));
        }
        unsafe {
            *bytes = length;
        }
        m.error = code;
        i32::from(code == 0)
    })
}
pub(super) unsafe fn CancelIoEx(h: Handle, o: *mut Overlapped) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls.push("cancel");
        let p = m.pending[&(h as usize)];
        assert_eq!(p.overlap, o as usize);
        if !m.defer_cancel {
            m.pending.remove(&(h as usize));
            *m.events.get_mut(&p.event).unwrap() = true;
        }
        if let Some(code) = m.cancel_code {
            m.error = code;
            0
        } else {
            1
        }
    })
}
pub(super) unsafe fn WaitForSingleObject(event: Handle, ms: u32) -> u32 {
    assert_eq!(ms, 0);
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !call(&mut m, "wait") {
            return u32::MAX;
        }
        if m.events[&(event as usize)] { 0 } else { 258 }
    })
}
pub(super) unsafe fn DisconnectNamedPipe(h: Handle) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        assert!(!m.pending.contains_key(&(h as usize)));
        assert!(m.handles[&(h as usize)]);
        i32::from(call(&mut m, "disconnect"))
    })
}
pub(super) unsafe fn CloseHandle(h: Handle) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        let h = h as usize;
        assert!(!m.pending.contains_key(&h));
        assert!(!m.pending.values().any(|p| p.event == h));
        let is_pipe = m.handles.contains_key(&h);
        if !call(&mut m, if is_pipe { "close-pipe" } else { "close-event" }) {
            return 0;
        }
        assert!(m.handles.remove(&h).is_some() || m.events.remove(&h).is_some());
        m.closes.push(h);
        1
    })
}

impl Mock {
    pub(super) fn fresh() -> Self {
        Self {
            signal_errors: true,
            ..Self::default()
        }
    }
}
pub(super) fn finish_pending() {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        let pending = std::mem::take(&mut m.pending);
        for p in pending.values() {
            *m.events.get_mut(&p.event).unwrap() = true;
        }
    });
}

pub(super) unsafe fn GetNamedPipeServerProcessId(handle: Handle, pid: *mut u32) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        assert!(!m.handles[&(handle as usize)]);
        if !call(&mut m, "server-pid") {
            return 0;
        }
        // SAFETY: initialized ULONG output supplied by current synchronous call.
        unsafe {
            *pid = m.server_pid.unwrap_or(42);
        }
        1
    })
}
