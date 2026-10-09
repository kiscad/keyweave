use super::*;
#[link(name = "kernel32")]
unsafe extern "system" {
    pub(super) fn GetCurrentProcessId() -> u32;
    pub(super) fn GetCurrentProcess() -> Handle;
    pub(super) fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    pub(super) fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    pub(super) fn Process32NextW(snapshot: Handle, entry: *mut ProcessEntry) -> i32;
    pub(super) fn CloseHandle(handle: Handle) -> i32;
    pub(super) fn GetLastError() -> u32;
    pub(super) fn QueryInformationJobObject(
        job: Handle,
        class: i32,
        info: *mut c_void,
        bytes: u32,
        length: *mut u32,
    ) -> i32;
    pub(super) fn GetStartupInfoW(info: *mut Startup);
    pub(super) fn GetStdHandle(kind: u32) -> Handle;
    pub(super) fn GetFileType(handle: Handle) -> u32;
    pub(super) fn GetNamedPipeInfo(
        handle: Handle,
        flags: *mut u32,
        output: *mut u32,
        input: *mut u32,
        instances: *mut u32,
    ) -> i32;
    pub(super) fn GetNamedPipeHandleStateW(
        handle: Handle,
        state: *mut u32,
        instances: *mut u32,
        count: *mut u32,
        timeout: *mut u32,
        username: *mut u16,
        size: u32,
    ) -> i32;
    pub(super) fn GetNamedPipeServerProcessId(handle: Handle, pid: *mut u32) -> i32;
    pub(super) fn GetNamedPipeClientProcessId(handle: Handle, pid: *mut u32) -> i32;
    pub(super) fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
    pub(super) fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: i32,
        flags: u32,
    ) -> i32;
}
#[link(name = "ntdll")]
unsafe extern "system" {
    pub(super) fn NtQueryInformationFile(
        handle: Handle,
        status: *mut IoStatusBlock,
        output: *mut c_void,
        length: u32,
        class: i32,
    ) -> i32;
}
