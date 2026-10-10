//! Foreground image metadata only; no titles, input state, capture or injection.
use crate::startup::ProbeError;
use keyweave_types::ApplicationContext;
use std::{ffi::c_void, ptr::null_mut};
type Handle = *mut c_void;
const CAPACITY: usize = 4097;
#[cfg(not(test))]
mod api {
    use super::*;
    #[link(name = "user32")]
    unsafe extern "system" {
        pub(super) fn GetForegroundWindow() -> Handle;
        pub(super) fn GetWindowThreadProcessId(window: Handle, process: *mut u32) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn OpenProcess(access: u32, inherit: i32, process: u32) -> Handle;
        pub(super) fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            buffer: *mut u16,
            count: *mut u32,
        ) -> i32;
        pub(super) fn CloseHandle(handle: Handle) -> i32;
        pub(super) fn GetLastError() -> u32;
    }
}
#[cfg(test)]
mod api;
use api::*;
struct Process(Handle);
impl Process {
    fn close(&mut self) -> Result<(), ProbeError> {
        if self.0.is_null() {
            return Ok(());
        }
        let handle = std::mem::replace(&mut self.0, null_mut());
        // SAFETY: uniquely owned real OpenProcess handle; exactly one close
        // attempt, never a borrowed or pseudo process handle.
        if unsafe { CloseHandle(handle) } == 0 {
            // SAFETY: thread-local error read immediately after failed close.
            return Err(ProbeError::Api {
                call: "CloseHandle(application)",
                code: unsafe { GetLastError() },
            });
        }
        Ok(())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn foreground() -> Option<(Handle, u32, u32)> {
    // SAFETY: returns a borrowed HWND, never closed or dereferenced here.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return None;
    }
    let mut pid = 0;
    // SAFETY: borrowed HWND and initialized DWORD output live for this call.
    let tid = unsafe { GetWindowThreadProcessId(window, &mut pid) };
    (pid != 0 && tid != 0).then_some((window, pid, tid))
}
pub(super) fn sample() -> Result<Option<ApplicationContext>, ProbeError> {
    if !super::hook_thread::outside_callback() {
        return Err(ProbeError::InCallback);
    }
    let Some(before) = foreground() else {
        return Ok(None);
    };
    // SAFETY: query-only, noninheritable handle for the sampled process ID.
    // Open can fail/race; that yields Unknown, not stale metadata.
    let handle = unsafe { OpenProcess(0x1000, 0, before.1) };
    if handle.is_null() {
        return Ok(None);
    }
    let mut process = Process(handle);
    let mut buffer = [0u16; CAPACITY];
    let mut count = CAPACITY as u32;
    // SAFETY: unique live process handle, Win32 path flag, bounded writable
    // UTF-16 array and DWORD count. Never interpret an unsuccessful buffer.
    let ok = unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut count) };
    let after = foreground();
    // Preserve close failure even when sampling failed or the foreground moved.
    process.close()?;
    let count = count as usize;
    if ok == 0
        || after != Some(before)
        || count == 0
        || count >= CAPACITY
        || buffer[count] != 0
        || buffer[..count].contains(&0)
    {
        return Ok(None);
    }
    let Ok(path) = String::from_utf16(&buffer[..count]) else {
        return Ok(None);
    };
    if path.len() > 4096 {
        return Ok(None);
    }
    Ok(Some(ApplicationContext {
        bundle_identifier: None,
        file_path: Some(path),
    }))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(super) fn test_sample_mode(query: bool, close: bool) {
    api::MOCK.with(|m| {
        *m.borrow_mut() = api::Mock {
            query,
            close,
            ..Default::default()
        }
    });
}
