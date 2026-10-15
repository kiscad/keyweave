#![allow(non_snake_case)]
use super::*;
use std::{cell::RefCell, collections::BTreeMap};
pub(super) struct Mock {
    pub current_session: u32,
    pub peer_only_failure: bool,
    pub wrong_process_after: bool,
    pub process_reads: usize,
    pub token_type_bad: bool,
    pub short_stats: bool,
    pub impersonating: bool,
    pub thread_error: u32,
    pub fail_class: i32,
    pub bad_pointer: bool,
    pub bad_length: bool,
    pub bad_sid: bool,
    pub changed: bool,
    pub wrong_text: bool,
    pub peer_sid: bool,
    pub peer_logon: bool,
    pub peer_session: bool,
    pub session_zero: bool,
    pub wrong_process: bool,
    pub fail_close: bool,
    pub fail_free: bool,
    pub fail_descriptor: bool,
    pub calls: usize,
    pub closed: usize,
    pub sddl: String,
    pub handles: BTreeMap<usize, bool>,
    pub strings: BTreeMap<usize, Box<[u16]>>,
    pub descriptors: BTreeMap<usize, Box<u64>>,
    next: usize,
    error: u32,
    stats: BTreeMap<usize, usize>,
}
impl Default for Mock {
    fn default() -> Self {
        Self {
            current_session: 1,
            peer_only_failure: false,
            wrong_process_after: false,
            process_reads: 0,
            token_type_bad: false,
            short_stats: false,
            impersonating: false,
            thread_error: 1008,
            fail_class: 0,
            bad_pointer: false,
            bad_length: false,
            bad_sid: false,
            changed: false,
            wrong_text: false,
            peer_sid: false,
            peer_logon: false,
            peer_session: false,
            session_zero: false,
            wrong_process: false,
            fail_close: false,
            fail_free: false,
            fail_descriptor: false,
            calls: 0,
            closed: 0,
            sddl: String::new(),
            handles: BTreeMap::new(),
            strings: BTreeMap::new(),
            descriptors: BTreeMap::new(),
            next: 100,
            error: 0,
            stats: BTreeMap::new(),
        }
    }
}
thread_local! {pub(super)static MOCK:RefCell<Mock>=RefCell::new(Mock::default());}
pub(super) unsafe fn GetLastError() -> u32 {
    MOCK.with(|m| m.borrow().error)
}
pub(super) unsafe fn GetCurrentThread() -> Handle {
    (-2isize) as Handle
}
pub(super) unsafe fn GetCurrentProcess() -> Handle {
    (-1isize) as Handle
}
pub(super) unsafe fn OpenThreadToken(
    _: Handle,
    access: u32,
    as_self: i32,
    out: *mut Handle,
) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls += 1;
        if access != 8 || as_self != 1 {
            m.error = 87;
            return 0;
        }
        if !m.impersonating {
            m.error = m.thread_error;
            return 0;
        }
        m.next += 1;
        let h = m.next;
        m.handles.insert(h, false);
        // SAFETY: caller provides one writable initialized HANDLE output.
        unsafe {
            *out = h as Handle;
        }
        1
    })
}
pub(super) unsafe fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls += 1;
        if access != 0x1000 || inherit != 0 || pid != 42 {
            m.error = 5;
            return null_mut();
        }
        m.next += 1;
        let h = m.next;
        m.handles.insert(h, true);
        h as Handle
    })
}
pub(super) unsafe fn GetProcessId(_: Handle) -> u32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.process_reads += 1;
        if m.wrong_process || m.wrong_process_after && m.process_reads > 1 {
            43
        } else {
            42
        }
    })
}
pub(super) unsafe fn OpenProcessToken(process: Handle, access: u32, out: *mut Handle) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls += 1;
        if access != 8 {
            m.error = 87;
            return 0;
        }
        let peer = process as isize != -1;
        m.next += 1;
        let h = m.next;
        m.handles.insert(h, peer);
        // SAFETY: live HANDLE output, successful mock creates one owned handle.
        unsafe {
            *out = h as Handle;
        }
        1
    })
}
pub(super) unsafe fn CloseHandle(handle: Handle) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.closed += 1;
        if m.fail_close
            && (!m.peer_only_failure || m.handles.get(&(handle as usize)) == Some(&true))
        {
            m.error = 6;
            return 0;
        }
        if m.handles.remove(&(handle as usize)).is_some() {
            1
        } else {
            m.error = 6;
            0
        }
    })
}
pub(super) unsafe fn GetTokenInformation(
    token: Handle,
    class: i32,
    data: *mut c_void,
    len: u32,
    needed: *mut u32,
) -> i32 {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls += 1;
        let peer = m.handles.get(&(token as usize)).copied().unwrap_or(false);
        if len != 1024 || class == m.fail_class && (!m.peer_only_failure || peer) {
            m.error = 5;
            return 0;
        }
        // SAFETY: query owns an initialized aligned 1024-byte allocation and live
        // DWORD output throughout this synchronous mocked SDK call.
        let bytes = unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), 1024) };
        let count = match class {
            1 => {
                let sid = Sid::text(if peer && m.peer_sid {
                    "S-1-5-21-200"
                } else {
                    "S-1-5-21-100"
                })
                .unwrap_or_else(|_| unreachable!());
                let offset = size_of::<SidAttributes>();
                bytes[offset..offset + sid.bytes().len()].copy_from_slice(sid.bytes());
                if m.bad_sid {
                    bytes[offset] = 0;
                }
                let pointer = if m.bad_pointer {
                    data as usize + 2048
                } else {
                    data as usize + offset
                };
                bytes[..size_of::<usize>()].copy_from_slice(&pointer.to_ne_bytes());
                offset + sid.bytes().len()
            }
            10 => {
                let times = m.stats.entry(token as usize).or_default();
                *times += 1;
                let changed = *times > 1 && m.changed;
                bytes[..8].copy_from_slice(&(token as u64).to_le_bytes());
                bytes[8..16].copy_from_slice(
                    &(if peer && m.peer_logon { 56u64 } else { 55u64 }).to_le_bytes(),
                );
                bytes[24..28]
                    .copy_from_slice(&(if m.token_type_bad { 2u32 } else { 1u32 }).to_le_bytes());
                bytes[48..56].copy_from_slice(&u64::from(changed).to_le_bytes());
                56
            }
            12 => {
                bytes[..4].copy_from_slice(
                    &(if m.session_zero {
                        0u32
                    } else if peer && m.peer_session {
                        2u32
                    } else {
                        m.current_session
                    })
                    .to_le_bytes(),
                );
                4
            }
            _ => {
                m.error = 87;
                return 0;
            }
        };
        // SAFETY: live DWORD output as specified above.
        unsafe {
            *needed = if m.bad_length {
                2048
            } else if class == 10 && m.short_stats {
                55
            } else {
                count as u32
            };
        }
        1
    })
}
pub(super) unsafe fn ConvertSidToStringSidW(sid: *const c_void, out: *mut *mut u16) -> i32 {
    // SAFETY: production parser proved the complete SID length inside its live Box.
    let count = unsafe { *sid.cast::<u8>().add(1) } as usize;
    // SAFETY: complete SID is at least 8+count*4 bytes under that same proof.
    let raw = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), 8 + count * 4) };
    let rid = if count >= 2 {
        u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]])
    } else {
        0
    };
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        let text = if m.wrong_text {
            "S-1-5-21-999".to_owned()
        } else {
            format!("S-1-5-21-{rid}")
        };
        let mut words = text
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let ptr = words.as_mut_ptr();
        m.strings.insert(ptr as usize, words);
        // SAFETY: live WCHAR* output, allocation retained until LocalFree.
        unsafe {
            *out = ptr;
        }
        1
    })
}
pub(super) unsafe fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
    text: *const u16,
    revision: u32,
    out: *mut Handle,
    _: *mut u32,
) -> i32 {
    let mut words = Vec::new();
    for i in 0..512 {
        // SAFETY: production passes its live terminated bounded SDDL buffer.
        let word = unsafe { *text.add(i) };
        if word == 0 {
            break;
        }
        words.push(word);
    }
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.sddl = String::from_utf16_lossy(&words);
        if revision != 1 || m.fail_descriptor {
            m.error = 1336;
            return 0;
        }
        let mut value = Box::new(1u64);
        let ptr = (&mut *value as *mut u64).cast::<c_void>();
        m.descriptors.insert(ptr as usize, value);
        // SAFETY: successful SDK transfers one owned LocalAlloc-like descriptor.
        unsafe {
            *out = ptr;
        }
        1
    })
}
pub(super) unsafe fn LocalFree(memory: Handle) -> Handle {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        if m.fail_free {
            m.error = 5;
            return memory;
        }
        let key = memory as usize;
        if m.strings.remove(&key).is_some() || m.descriptors.remove(&key).is_some() {
            null_mut()
        } else {
            m.error = 6;
            memory
        }
    })
}
