//! Read-only access checks and aggregate key observation; never capture-ready.
use crate::startup::{InputSample, ProbeError};
use std::{ffi::c_void, mem::size_of, ptr::null_mut};
type Handle = *mut c_void;
#[repr(C)]
#[derive(Default)]
struct UserFlags {
    inherit: i32,
    reserved: i32,
    flags: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Label {
    sid: usize,
    attributes: u32,
}
#[cfg(not(test))]
mod api {
    use super::*;
    #[link(name = "user32")]
    unsafe extern "system" {
        pub(super) fn GetProcessWindowStation() -> Handle;
        pub(super) fn GetThreadDesktop(thread: u32) -> Handle;
        pub(super) fn OpenInputDesktop(flags: u32, inherit: i32, access: u32) -> Handle;
        pub(super) fn CloseDesktop(desktop: Handle) -> i32;
        pub(super) fn GetUserObjectInformationW(
            object: Handle,
            index: i32,
            data: *mut c_void,
            length: u32,
            needed: *mut u32,
        ) -> i32;
        pub(super) fn GetForegroundWindow() -> Handle;
        pub(super) fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
        pub(super) fn GetAsyncKeyState(key: i32) -> i16;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn GetCurrentThreadId() -> u32;
        pub(super) fn GetCurrentThread() -> Handle;
        pub(super) fn GetCurrentProcess() -> Handle;
        pub(super) fn GetCurrentProcessId() -> u32;
        pub(super) fn OpenProcess(access: u32, inherit: i32, process: u32) -> Handle;
        pub(super) fn CloseHandle(handle: Handle) -> i32;
        pub(super) fn GetLastError() -> u32;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        pub(super) fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        pub(super) fn OpenThreadToken(
            thread: Handle,
            access: u32,
            as_self: i32,
            token: *mut Handle,
        ) -> i32;
        pub(super) fn GetTokenInformation(
            token: Handle,
            class: i32,
            data: *mut c_void,
            length: u32,
            needed: *mut u32,
        ) -> i32;
    }
}
use api::*;
fn error(call: &'static str) -> ProbeError {
    // SAFETY: thread-local GetLastError, called immediately after a failed API.
    ProbeError::Api {
        call,
        code: unsafe { GetLastError() },
    }
}
struct Owned {
    handle: Handle,
    desktop: bool,
}
impl Owned {
    fn new(handle: Handle, desktop: bool, call: &'static str) -> Result<Self, ProbeError> {
        if handle.is_null() {
            return Err(error(call));
        }
        Ok(Self { handle, desktop })
    }
    fn close(&mut self) -> Result<(), ProbeError> {
        if self.handle.is_null() {
            return Ok(());
        }
        let handle = std::mem::replace(&mut self.handle, null_mut());
        // SAFETY: unique acquired handle, matching close family; only one attempt.
        let ok = unsafe {
            if self.desktop {
                CloseDesktop(handle)
            } else {
                CloseHandle(handle)
            }
        };
        if ok == 0 {
            return Err(error(if self.desktop {
                "CloseDesktop"
            } else {
                "CloseHandle"
            }));
        }
        Ok(())
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: this unique handle came from OpenInputDesktop/OpenProcess or
        // OpenProcessToken/OpenThreadToken, never a borrowed/pseudo handle.
        // Each owner attempts close once; no raw handle escapes the module.
        let _ = self.close();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Name {
    words: [u16; 64],
    length: usize,
}
fn name(handle: Handle) -> Result<Name, ProbeError> {
    let mut value = Name {
        words: [0; 64],
        length: 0,
    };
    let mut needed = 0;
    // SAFETY: live borrowed/owned USER handle; writable initialized WCHAR array
    // of exactly 128 bytes and DWORD length output. UOI_NAME=2, no pointer retained.
    let ok = unsafe {
        GetUserObjectInformationW(handle, 2, value.words.as_mut_ptr().cast(), 128, &mut needed)
    };
    if ok == 0 {
        return Err(error("GetUserObjectInformationW(name)"));
    }
    if !(4..=128).contains(&needed) || needed % 2 != 0 {
        return Err(ProbeError::Malformed);
    }
    value.length = needed as usize / 2 - 1;
    if value.words[value.length] != 0 || value.words[..value.length].contains(&0) {
        return Err(ProbeError::Malformed);
    }
    // Ignore any native tail padding, keeping identity comparison deterministic.
    value.words[value.length..].fill(0);
    Ok(value)
}
fn no_impersonation() -> Result<(), ProbeError> {
    let mut token = null_mut();
    // SAFETY: current-thread pseudo handle is borrowed. TOKEN_QUERY=8, TRUE
    // OpenAsSelf asks using process security context; writable HANDLE output.
    let ok = unsafe { OpenThreadToken(GetCurrentThread(), 8, 1, &mut token) };
    if ok != 0 {
        let _token = Owned::new(token, false, "OpenThreadToken")?;
        return Err(ProbeError::Impersonating);
    }
    // SAFETY: immediate thread-local error, ERROR_NO_TOKEN is the only allowed
    // failure. Access-denied cannot be mistaken for absent impersonation.
    let code = unsafe { GetLastError() };
    if code != 1008 {
        return Err(ProbeError::Api {
            call: "OpenThreadToken",
            code,
        });
    }
    Ok(())
}
fn integrity(process: Handle) -> Result<u32, ProbeError> {
    let mut token = null_mut();
    // SAFETY: borrowed live process handle/pseudo handle; TOKEN_QUERY only,
    // writable initialized HANDLE output becomes uniquely owned on success.
    if unsafe { OpenProcessToken(process, 8, &mut token) } == 0 {
        return Err(error("OpenProcessToken"));
    }
    let mut token = Owned::new(token, false, "OpenProcessToken")?;
    let mut needed = 0;
    // SAFETY: null/zero size query for TokenIntegrityLevel=25 and DWORD output.
    let ok = unsafe { GetTokenInformation(token.handle, 25, null_mut(), 0, &mut needed) };
    if ok != 0 {
        return Err(ProbeError::Malformed);
    }
    // SAFETY: immediate thread-local error, no intervening API.
    if unsafe { GetLastError() } != 122 {
        return Err(error("GetTokenInformation(size)"));
    }
    if needed < size_of::<Label>() as u32 || needed > 256 {
        return Err(ProbeError::Malformed);
    }
    let mut buffer = [0u64; 32];
    // SAFETY: 256 initialized writable bytes aligned to 8, enough for x86/x64
    // TOKEN_MANDATORY_LABEL and embedded SID; actual length is validated below.
    if unsafe {
        GetTokenInformation(
            token.handle,
            25,
            buffer.as_mut_ptr().cast(),
            256,
            &mut needed,
        )
    } == 0
    {
        return Err(error("GetTokenInformation(label)"));
    }
    let result = parse_integrity(&buffer, needed as usize);
    let closed = token.close();
    let rid = result?;
    closed?;
    Ok(rid)
}
fn parse_integrity(buffer: &[u64; 32], length: usize) -> Result<u32, ProbeError> {
    if length < size_of::<Label>() || length > 256 {
        return Err(ProbeError::Malformed);
    }
    // SAFETY: length covers the repr(C) Label, buffer is initialized and aligned
    // to 8; usize/u32 fields admit all bit patterns. No SID is dereferenced here.
    let label = unsafe { std::ptr::read(buffer.as_ptr().cast::<Label>()) };
    let offset = label
        .sid
        .checked_sub(buffer.as_ptr() as usize)
        .ok_or(ProbeError::Malformed)?;
    let end = offset.checked_add(12).ok_or(ProbeError::Malformed)?;
    if label.attributes & 0x20 == 0 || offset < size_of::<Label>() || end > length {
        return Err(ProbeError::Malformed);
    }
    // SAFETY: borrow only the verified returned prefix within this live buffer;
    // immutable byte inspection, SID pointer was reduced to a bounded offset.
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), length) };
    let sid = &bytes[offset..end];
    if sid[0] != 1 || sid[1] != 1 || sid[2..8] != [0, 0, 0, 0, 0, 16] {
        return Err(ProbeError::Malformed);
    }
    let rid = u32::from_le_bytes(sid[8..12].try_into().map_err(|_| ProbeError::Malformed)?);
    Ok(rid)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Context {
    station: Name,
    desktop: Name,
    foreground: usize,
    pid: u32,
    caller_rid: u32,
    foreground_rid: u32,
}
fn context() -> Result<(Context, Owned), ProbeError> {
    no_impersonation()?;
    // SAFETY: these USER/thread handles are borrowed; they are never closed.
    let (station, thread) = unsafe {
        (
            GetProcessWindowStation(),
            GetThreadDesktop(GetCurrentThreadId()),
        )
    };
    if station.is_null() || thread.is_null() {
        return Err(ProbeError::NotInteractive);
    }
    let mut flags = UserFlags::default();
    let mut needed = 0;
    // SAFETY: UOI_FLAGS=1 output is exactly the repr(C) 12-byte USEROBJECTFLAGS.
    if unsafe {
        GetUserObjectInformationW(
            station,
            1,
            (&mut flags as *mut UserFlags).cast(),
            size_of::<UserFlags>() as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(error("GetUserObjectInformationW(flags)"));
    }
    if needed != 12 {
        return Err(ProbeError::Malformed);
    }
    let station_name = name(station)?;
    let is_winsta0 = station_name.length == 7
        && station_name.words[..7]
            .iter()
            .zip(b"winsta0")
            .all(|(word, expected)| {
                u8::try_from(*word).is_ok_and(|byte| byte.to_ascii_lowercase() == *expected)
            });
    if flags.flags & 1 == 0 || !is_winsta0 {
        return Err(ProbeError::NotInteractive);
    }
    let mut is_input = 0i32;
    // SAFETY: UOI_IO=6 writes one BOOL to the initialized 4-byte output.
    if unsafe {
        GetUserObjectInformationW(
            thread,
            6,
            (&mut is_input as *mut i32).cast(),
            4,
            &mut needed,
        )
    } == 0
    {
        return Err(error("GetUserObjectInformationW(input)"));
    }
    if needed != 4 {
        return Err(ProbeError::Malformed);
    }
    if is_input == 0 {
        return Err(ProbeError::InactiveDesktop);
    }
    // SAFETY: readonly desktop metadata plus DESKTOP_HOOKCONTROL access check;
    // flags=0, inherit=FALSE. No switching/ACL changes. Owned close uses CloseDesktop.
    let input = Owned::new(
        unsafe { OpenInputDesktop(0, 0, 9) },
        true,
        "OpenInputDesktop",
    )?;
    let desktop = name(input.handle)?;
    if desktop != name(thread)? {
        return Err(ProbeError::InactiveDesktop);
    }
    // SAFETY: borrowed foreground HWND; no title/window content is read.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return Err(ProbeError::NoForeground);
    }
    let mut pid = 0;
    // SAFETY: borrowed HWND and initialized writable process-id output.
    if unsafe { GetWindowThreadProcessId(foreground, &mut pid) } == 0 || pid == 0 {
        return Err(ProbeError::NoForeground);
    }
    // SAFETY: current-process pseudo handle is borrowed and never closed.
    let caller_rid = integrity(unsafe { GetCurrentProcess() })?;
    // SAFETY: numeric current pid query with no pointers.
    let foreground_rid = if pid == unsafe { GetCurrentProcessId() } {
        caller_rid
    } else {
        // SAFETY: readonly QUERY_LIMITED_INFORMATION=0x1000, noninheritable;
        // uniquely owned successful HANDLE, never used for write/terminate.
        let mut process = Owned::new(unsafe { OpenProcess(0x1000, 0, pid) }, false, "OpenProcess")?;
        let result = integrity(process.handle);
        let closed = process.close();
        let rid = result?;
        closed?;
        rid
    };
    if foreground_rid > caller_rid {
        return Err(ProbeError::HigherIntegrity);
    }
    Ok((
        Context {
            station: station_name,
            desktop,
            foreground: foreground as usize,
            pid,
            caller_rid,
            foreground_rid,
        },
        input,
    ))
}
/// Only startup/recovery, outside callbacks. IdleObserved is not a neutral token;
/// combine with passive Hook bindings and a complete activation transaction.
pub fn sample_input_state() -> Result<InputSample, ProbeError> {
    if !super::hook_thread::outside_callback() {
        return Err(ProbeError::InCallback);
    }
    let (before, mut before_handle) = context()?;
    let mut held = false;
    for vk in 1..=254 {
        // SAFETY: supported integer VK range, no pointer. High bit only; low bit
        // is unreliable. Context access prerequisites are checked pre/post.
        held |= unsafe { GetAsyncKeyState(vk) } < 0;
    }
    let (after, mut after_handle) = context()?;
    if before != after {
        return Err(ProbeError::Changed);
    }
    before_handle.close()?;
    after_handle.close()?;
    Ok(if held {
        InputSample::Held
    } else {
        InputSample::IdleObserved
    })
}

/// Metadata-only runtime check. Never samples keys or changes desktop/token.
/// Context transitions during the two observations conservatively stop capture.
pub(super) fn check_input_access() -> Result<(), ProbeError> {
    if !super::hook_thread::outside_callback() {
        return Err(ProbeError::InCallback);
    }
    let (before, mut before_handle) = context()?;
    let (after, mut after_handle) = context()?;
    if before != after {
        return Err(ProbeError::Changed);
    }
    before_handle.close()?;
    after_handle.close()?;
    Ok(())
}

#[cfg(test)]
mod api;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(super) fn test_probe(key: i16, active: bool) {
    api::MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        mock.key = key;
        mock.active = active;
    });
}
#[cfg(test)]
pub(super) fn test_access_counts() -> (usize, usize) {
    api::MOCK.with(|mock| {
        let mock = mock.borrow();
        (mock.key_reads, mock.foreground_reads)
    })
}
#[cfg(test)]
pub(super) fn test_elevated() {
    api::MOCK.with(|mock| mock.borrow_mut().elevated = true);
}
