#![allow(non_snake_case, clippy::too_many_arguments, clippy::unwrap_used)]
use super::*;
use std::{cell::RefCell, collections::BTreeMap};
#[derive(Clone, Copy)]
struct Pending {
    overlap: usize,
    buffer: usize,
    count: usize,
    write: bool,
}
#[derive(Default)]
pub(super) struct Mock {
    pub fail: Option<&'static str>,
    pub calls: Vec<&'static str>,
    pub closes: Vec<usize>,
    pub last: u32,
    pub completion: Option<Result<u32, u32>>,
    pub payload: Vec<u8>,
    pub writes: Vec<Vec<u8>>,
    pub waits: Vec<u32>,
    pub defer_cancel: bool,
    pub error_signaled: bool,
    pub connect_pending: bool,
    pub pipe_flags: Option<u32>,
    pub pipe_instances: Option<u32>,
    pub pipe_state: Option<u32>,
    pub metadata_handle: Option<usize>,
    events: BTreeMap<usize, bool>,
    pending: BTreeMap<usize, Pending>,
    next_event: usize,
    next_pair: usize,
    synchronous: bool,
}
thread_local! { pub(super) static MOCK:RefCell<Mock>=RefCell::new(Mock {error_signaled:true,..Mock::default()}); }
fn call(name: &'static str) -> bool {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls.push(name);
        let ok = m.fail != Some(name);
        if !ok {
            m.last = 5;
        }
        ok
    })
}
pub(super) unsafe fn GetLastError() -> u32 {
    MOCK.with(|m| m.borrow().last)
}
pub(super) unsafe fn BCryptGenRandom(a: Handle, b: *mut u8, n: u32, f: u32) -> i32 {
    assert!(a.is_null());
    assert_eq!((n, f), (16, 2));
    if !call("rng") {
        return -1;
    }
    // SAFETY: tested caller supplies initialized writable 16-byte RNG output.
    let value = MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.next_pair += 1;
        16 + m.next_pair as u8
    });
    unsafe { std::ptr::write_bytes(b, value, 16) };
    0
}
pub(super) unsafe fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
    s: *const u16,
    r: u32,
    d: *mut *mut c_void,
    n: *mut u32,
) -> i32 {
    assert_eq!(r, 1);
    assert!(n.is_null());
    let expected: Vec<u16> = "D:P(A;;GA;;;OW)(A;;GA;;;SY)\0".encode_utf16().collect();
    // SAFETY: live bounded terminated SDDL input and initialized output.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(s, expected.len()) },
        expected
    );
    if !call("security") {
        return 0;
    }
    unsafe { *d = std::ptr::dangling_mut::<c_void>() };
    1
}
pub(super) unsafe fn LocalFree(p: *mut c_void) -> *mut c_void {
    assert_eq!(p as usize, 1);
    if call("free-security") { null_mut() } else { p }
}
pub(super) unsafe fn CreateNamedPipeW(
    n: *const u16,
    o: u32,
    m: u32,
    i: u32,
    out: u32,
    input: u32,
    t: u32,
    s: *const SecurityAttributes,
) -> Handle {
    assert!(!n.is_null());
    assert!(matches!(o, 0x40080001 | 0x80001));
    assert_eq!((m, i, out, input, t), (8, 1, 4096, 4096, 0));
    MOCK.with(|m| m.borrow_mut().synchronous = o == 0x80001);
    // SAFETY: synchronous caller retains its complete security input.
    let attrs = unsafe { &*s };
    assert_eq!(attrs.length as usize, size_of::<SecurityAttributes>());
    assert_eq!(attrs.inherit, 0);
    assert_eq!(attrs.descriptor as usize, 1);
    if call("server") {
        MOCK.with(|m| (100 + (m.borrow().next_pair - 1) * 10) as Handle)
    } else {
        usize::MAX as Handle
    }
}
pub(super) unsafe fn CreateFileW(
    n: *const u16,
    a: u32,
    s: u32,
    sa: *const SecurityAttributes,
    d: u32,
    f: u32,
    t: Handle,
) -> Handle {
    assert!(!n.is_null());
    assert!(sa.is_null() && t.is_null());
    assert_eq!((a, s, d), (0x40000080, 0, 3));
    assert_eq!(
        f,
        MOCK.with(|m| if m.borrow().synchronous {
            0x100000
        } else {
            0x40100000
        })
    );
    if call("client") {
        MOCK.with(|m| (200 + (m.borrow().next_pair - 1) * 10) as Handle)
    } else {
        usize::MAX as Handle
    }
}
pub(super) unsafe fn GetNamedPipeInfo(
    h: Handle,
    flags: *mut u32,
    output: *mut u32,
    input: *mut u32,
    instances: *mut u32,
) -> i32 {
    assert!((100..300).contains(&(h as usize)));
    assert!(output.is_null() && input.is_null());
    if !call(if (h as usize) < 200 {
        "info-reader"
    } else {
        "info-writer"
    }) {
        return 0;
    }
    MOCK.with(|m| {
        let m = m.borrow();
        let selected = m.metadata_handle.is_none_or(|value| value == h as usize);
        // SAFETY: tested caller retains initialized scalar outputs for the call.
        unsafe {
            *flags = if selected { m.pipe_flags } else { None }
                .unwrap_or(8 | u32::from((h as usize) < 200));
            *instances = if selected { m.pipe_instances } else { None }.unwrap_or(1);
        }
    });
    1
}
pub(super) unsafe fn GetNamedPipeHandleStateW(
    h: Handle,
    state: *mut u32,
    instances: *mut u32,
    count: *mut u32,
    timeout: *mut u32,
    username: *mut u16,
    username_size: u32,
) -> i32 {
    assert!((100..300).contains(&(h as usize)));
    assert!(instances.is_null() && count.is_null() && timeout.is_null() && username.is_null());
    assert_eq!(username_size, 0);
    if !call(if (h as usize) < 200 {
        "state-reader"
    } else {
        "state-writer"
    }) {
        return 0;
    }
    MOCK.with(|m| {
        let m = m.borrow();
        let selected = m.metadata_handle.is_none_or(|value| value == h as usize);
        // SAFETY: tested caller retains initialized DWORD output for this call.
        unsafe { *state = if selected { m.pipe_state } else { None }.unwrap_or(0) };
    });
    1
}
pub(super) unsafe fn CreateEventW(
    a: *const SecurityAttributes,
    m: i32,
    i: i32,
    n: *const u16,
) -> Handle {
    assert!(a.is_null() && n.is_null());
    assert_eq!((m, i), (1, 0));
    if !call("event") {
        return null_mut();
    }
    MOCK.with(|x| {
        let mut x = x.borrow_mut();
        x.next_event += 1;
        let h = 300 + x.next_event;
        x.events.insert(h, false);
        h as Handle
    })
}
pub(super) unsafe fn ResetEvent(e: Handle) -> i32 {
    if !call("reset") {
        return 0;
    }
    MOCK.with(|m| {
        *m.borrow_mut().events.get_mut(&(e as usize)).unwrap() = false;
    });
    1
}
pub(super) unsafe fn ConnectNamedPipe(h: Handle, o: *mut Overlapped) -> i32 {
    if !call("connect") {
        return 0;
    }
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if m.connect_pending {
            m.pending.insert(
                h as usize,
                Pending {
                    overlap: o as usize,
                    buffer: 0,
                    count: 0,
                    write: false,
                },
            );
            m.last = 997;
        } else {
            m.last = 535;
        }
    });
    0
}
pub(super) unsafe fn ReadFile(
    h: Handle,
    b: *mut c_void,
    n: u32,
    c: *mut u32,
    o: *mut Overlapped,
) -> i32 {
    assert!(!MOCK.with(|m| m.borrow().synchronous));
    assert!(c.is_null());
    assert!(n <= 64 && n > 0);
    if !call("read") {
        return 0;
    }
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        assert!(
            m.pending
                .insert(
                    h as usize,
                    Pending {
                        overlap: o as usize,
                        buffer: b as usize,
                        count: n as usize,
                        write: false
                    }
                )
                .is_none()
        );
        m.last = 997;
    });
    0
}
pub(super) unsafe fn WriteFile(
    h: Handle,
    b: *const c_void,
    n: u32,
    c: *mut u32,
    o: *mut Overlapped,
) -> i32 {
    assert!(!MOCK.with(|m| m.borrow().synchronous));
    assert!(c.is_null());
    assert!(n <= 64 && n > 0);
    if !call("write") {
        return 0;
    }
    // SAFETY: live initialized immutable <=64-byte input for synchronous copy.
    let data = unsafe { std::slice::from_raw_parts(b.cast::<u8>(), n as usize) }.to_vec();
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.writes.push(data);
        assert!(
            m.pending
                .insert(
                    h as usize,
                    Pending {
                        overlap: o as usize,
                        buffer: b as usize,
                        count: n as usize,
                        write: true
                    }
                )
                .is_none()
        );
        m.last = 997;
    });
    0
}
pub(super) unsafe fn GetOverlappedResult(
    h: Handle,
    o: *mut Overlapped,
    n: *mut u32,
    w: i32,
) -> i32 {
    assert_eq!(w, 0);
    call("result");
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        let p = *m.pending.get(&(h as usize)).unwrap();
        assert_eq!(p.overlap, o as usize);
        let Some(result) = m.completion.take() else {
            m.last = 996;
            return 0;
        };
        // SAFETY: stored pointer still denotes the caller's pinned operation.
        let event = unsafe { (*o).event } as usize;
        match result {
            Ok(count) => {
                if !p.write && count as usize <= p.count && count > 0 {
                    assert!(m.payload.len() >= count as usize);
                    // SAFETY: mock completes at most requested bytes before signaling.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            m.payload.as_ptr(),
                            p.buffer as *mut u8,
                            count as usize,
                        )
                    };
                }
                // SAFETY: live initialized DWORD provided for this query.
                unsafe { *n = count };
                m.pending.remove(&(h as usize));
                m.events.insert(event, true);
                1
            }
            Err(code) => {
                m.last = code;
                if m.error_signaled {
                    m.events.insert(event, true);
                    m.pending.remove(&(h as usize));
                }
                0
            }
        }
    })
}
pub(super) unsafe fn CancelIoEx(h: Handle, o: *mut Overlapped) -> i32 {
    if !call("cancel") {
        return 0;
    }
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if !m.defer_cancel {
            // SAFETY: owned operation remains live through cancellation.
            let event = unsafe { (*o).event } as usize;
            m.events.insert(event, true);
            m.pending.remove(&(h as usize));
        }
    });
    1
}
pub(super) unsafe fn WaitForSingleObject(e: Handle, ms: u32) -> u32 {
    call("wait");
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.waits.push(ms);
        if m.events.get(&(e as usize)).copied().unwrap_or(false) {
            0
        } else {
            258
        }
    })
}
pub(super) unsafe fn CloseHandle(h: Handle) -> i32 {
    if !call("close") {
        return 0;
    }
    MOCK.with(|m| m.borrow_mut().closes.push(h as usize));
    1
}
