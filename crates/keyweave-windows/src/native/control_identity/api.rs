use super::*;
#[link(name = "kernel32")]
unsafe extern "system" {
    pub(super) fn GetCurrentThread() -> Handle;
    pub(super) fn GetCurrentProcess() -> Handle;
    pub(super) fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    pub(super) fn GetProcessId(process: Handle) -> u32;
    pub(super) fn CloseHandle(handle: Handle) -> i32;
    pub(super) fn GetLastError() -> u32;
    pub(super) fn LocalFree(memory: Handle) -> Handle;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    pub(super) fn OpenThreadToken(
        thread: Handle,
        access: u32,
        as_self: i32,
        token: *mut Handle,
    ) -> i32;
    pub(super) fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    pub(super) fn GetTokenInformation(
        token: Handle,
        class: i32,
        data: *mut c_void,
        length: u32,
        needed: *mut u32,
    ) -> i32;
    pub(super) fn ConvertSidToStringSidW(sid: *const c_void, string: *mut *mut u16) -> i32;
    pub(super) fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        descriptor: *mut Handle,
        length: *mut u32,
    ) -> i32;
}
