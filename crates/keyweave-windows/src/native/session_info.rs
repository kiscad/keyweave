//! Local WTS metadata only. No key sampling, account strings or remote server.
use crate::startup::ProbeError;
use std::{
    ffi::c_void,
    mem::size_of,
    ptr::{addr_of, null_mut},
};
type Handle = *mut c_void;
#[repr(C)]
struct Level1 {
    session_id: u32,
    state: i32,
    flags: i32,
    station: [u16; 33],
    user: [u16; 21],
    domain: [u16; 18],
    logon: i64,
    connect: i64,
    disconnect: i64,
    last_input: i64,
    current: i64,
    counters: [u32; 6],
}
#[repr(C)]
struct InfoEx {
    level: u32,
    // WTSINFOEX_LEVEL_W currently has just this member. Accept Level=1 only.
    data: Level1,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SessionIdentity {
    pub(super) id: u32,
    pub(super) logon: i64,
}
#[cfg(not(test))]
mod api {
    use super::*;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn GetCurrentProcessId() -> u32;
        pub(super) fn ProcessIdToSessionId(process: u32, session: *mut u32) -> i32;
        pub(super) fn GetLastError() -> u32;
    }
    #[link(name = "wtsapi32")]
    unsafe extern "system" {
        pub(super) fn WTSQuerySessionInformationW(
            server: Handle,
            session: u32,
            class: i32,
            buffer: *mut *mut u16,
            bytes: *mut u32,
        ) -> i32;
        pub(super) fn WTSFreeMemory(buffer: *mut c_void);
    }
}
use api::*;
struct WtsBuffer(*mut u16);
impl Drop for WtsBuffer {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: uniquely owned successful WTSQuerySessionInformationW
            // allocation; no raw pointer escapes, never freed by Rust/CloseHandle.
            unsafe {
                WTSFreeMemory(self.0.cast());
            }
        }
    }
}
fn process_session(pid: u32) -> Result<u32, ProbeError> {
    let mut id = 0;
    // SAFETY: readonly process/session metadata and initialized DWORD output.
    if unsafe { ProcessIdToSessionId(pid, &mut id) } == 0 {
        return Err(ProbeError::Api {
            call: "ProcessIdToSessionId",
            code: unsafe { GetLastError() },
        });
    }
    if id == 0 || id == u32::MAX {
        return Err(ProbeError::InactiveSession);
    }
    Ok(id)
}
pub(super) fn own_session() -> Result<SessionIdentity, ProbeError> {
    if !super::hook_thread::outside_callback() {
        return Err(ProbeError::InCallback);
    }
    // SAFETY: numeric current-process id, no pointers or ownership transfer.
    let id = process_session(unsafe { GetCurrentProcessId() })?;
    let mut buffer = null_mut();
    let mut bytes = 0;
    // SAFETY: local-server pseudo handle=NULL, explicit nonreserved session id;
    // WTSSessionInfoEx=25 outputs a uniquely owned allocation and DWORD length.
    if unsafe { WTSQuerySessionInformationW(null_mut(), id, 25, &mut buffer, &mut bytes) } == 0 {
        // SAFETY: retrieve the failure immediately, before any cleanup/API.
        return Err(ProbeError::Api {
            call: "WTSQuerySessionInformationW",
            code: unsafe { GetLastError() },
        });
    }
    let buffer = WtsBuffer(buffer);
    if buffer.0.is_null() || bytes as usize != size_of::<InfoEx>() {
        return Err(ProbeError::Malformed);
    }
    let info = buffer.0.cast::<InfoEx>();
    // SAFETY: successful native allocation length covers the SDK-checked full
    // InfoEx layout. Read just initialized scalar fields without making a Rust
    // reference to the struct or copying account strings/uninitialized padding.
    // Unaligned reads do not require allocator alignment. Level is checked before
    // selecting the level1 member; all scalar bit patterns are representable.
    unsafe {
        if addr_of!((*info).level).read_unaligned() != 1 {
            return Err(ProbeError::Malformed);
        }
        let actual_id = addr_of!((*info).data.session_id).read_unaligned();
        let state = addr_of!((*info).data.state).read_unaligned();
        let flags = addr_of!((*info).data.flags).read_unaligned();
        let logon = addr_of!((*info).data.logon).read_unaligned();
        if actual_id != id {
            return Err(ProbeError::Changed);
        }
        if state != 0 {
            return Err(ProbeError::InactiveSession);
        }
        match flags {
            0 => return Err(ProbeError::LockedSession),
            1 => {}
            _ => return Err(ProbeError::UnknownSession),
        }
        if logon <= 0 {
            return Err(ProbeError::UnknownSession);
        }
        Ok(SessionIdentity { id, logon })
    }
}
pub(super) fn check_foreground(pid: u32, identity: SessionIdentity) -> Result<(), ProbeError> {
    if process_session(pid)? != identity.id {
        return Err(ProbeError::Changed);
    }
    Ok(())
}
#[cfg(test)]
pub(super) mod api;
#[cfg(test)]
mod tests;
