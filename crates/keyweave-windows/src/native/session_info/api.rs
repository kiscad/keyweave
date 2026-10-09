//! Same-source WTS replacements, including unique native allocation ownership.
#![allow(non_snake_case)]
use super::*;
use std::{cell::RefCell, collections::BTreeSet};
pub(crate) struct Mock {
    pub caller: u32,
    pub foreground: u32,
    pub state: i32,
    pub flags: i32,
    pub logon: i64,
    pub level: u32,
    pub returned_id: Option<u32>,
    pub bytes: u32,
    pub null: bool,
    pub fail_pid: bool,
    pub fail_query: bool,
    pub queries: usize,
    pub freed: usize,
    pub processes: usize,
    pub change_after: Option<usize>,
    pub callback: Option<crate::keys::HookRecord>,
    live: BTreeSet<usize>,
}
impl Default for Mock {
    fn default() -> Self {
        Self {
            caller: 1,
            foreground: 1,
            state: 0,
            flags: 1,
            logon: 1_000_000,
            level: 1,
            returned_id: None,
            bytes: size_of::<InfoEx>() as u32,
            null: false,
            fail_pid: false,
            fail_query: false,
            queries: 0,
            freed: 0,
            processes: 0,
            change_after: None,
            callback: None,
            live: BTreeSet::new(),
        }
    }
}
thread_local! { pub(crate) static MOCK: RefCell<Mock> = RefCell::new(Mock::default()); }
pub(super) unsafe fn GetCurrentProcessId() -> u32 {
    200
}
pub(super) unsafe fn GetLastError() -> u32 {
    5
}
pub(super) unsafe fn ProcessIdToSessionId(pid: u32, id: *mut u32) -> i32 {
    MOCK.with(|cell| {
        let mut mock = cell.borrow_mut();
        mock.processes += 1;
        if mock.fail_pid {
            return 0;
        }
        // SAFETY: tested caller supplies one writable DWORD output.
        unsafe {
            id.write(if pid == 200 {
                mock.caller
            } else {
                mock.foreground
            });
        }
        1
    })
}
pub(super) unsafe fn WTSQuerySessionInformationW(
    server: Handle,
    id: u32,
    class: i32,
    buffer: *mut *mut u16,
    bytes: *mut u32,
) -> i32 {
    assert!(server.is_null());
    assert_eq!(class, 25);
    let (ok, callback) = MOCK.with(|cell| {
        let mut mock = cell.borrow_mut();
        mock.queries += 1;
        let callback = mock.callback.take();
        if mock.fail_query {
            return (0, callback);
        }
        let shifted = mock.change_after.is_some_and(|at| mock.queries >= at);
        let info = InfoEx {
            level: mock.level,
            data: Level1 {
                session_id: mock.returned_id.unwrap_or(id),
                state: mock.state,
                flags: mock.flags,
                logon: mock.logon + i64::from(shifted),
                station: [0; 33],
                user: [0; 21],
                domain: [0; 18],
                connect: 0,
                disconnect: 0,
                last_input: 0,
                current: 0,
                counters: [0; 6],
            },
        };
        let pointer = if mock.null {
            null_mut()
        } else {
            Box::into_raw(Box::new(info))
        };
        if !pointer.is_null() {
            assert!(mock.live.insert(pointer as usize));
        }
        // SAFETY: tested caller supplies writable pointer and DWORD outputs;
        // successful non-null buffer stays uniquely owned until WTSFreeMemory.
        unsafe {
            buffer.write(pointer.cast());
            bytes.write(mock.bytes);
        }
        (1, callback)
    });
    if let Some(record) = callback {
        assert_eq!(super::super::hook_thread::test_record(record), 9);
    }
    ok
}
pub(super) unsafe fn WTSFreeMemory(buffer: *mut c_void) {
    MOCK.with(|cell| {
        let mut mock = cell.borrow_mut();
        assert!(mock.live.remove(&(buffer as usize)));
        mock.freed += 1;
    });
    // SAFETY: exact unique pointer allocated above, freed once after the owner
    // leaves scope; no other reference/pointer is retained by this replacement.
    unsafe {
        drop(Box::from_raw(buffer.cast::<InfoEx>()));
    }
}
pub(crate) fn reset(configure: impl FnOnce(&mut Mock)) {
    MOCK.with(|cell| {
        let mut m = cell.borrow_mut();
        assert!(m.live.is_empty());
        *m = Mock::default();
        configure(&mut m);
    });
}
pub(crate) fn counts() -> (usize, usize, usize) {
    MOCK.with(|cell| {
        let m = cell.borrow();
        (m.queries, m.freed, m.live.len())
    })
}
