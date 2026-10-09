use super::*;
#[link(name = "kernel32")]
unsafe extern "system" {
    pub(super) fn GetNamedPipeInfo(
        pipe: Handle,
        flags: *mut u32,
        output_buffer: *mut u32,
        input_buffer: *mut u32,
        instances: *mut u32,
    ) -> i32;
    pub(super) fn GetNamedPipeHandleStateW(
        pipe: Handle,
        state: *mut u32,
        instances: *mut u32,
        collection_count: *mut u32,
        collection_timeout: *mut u32,
        username: *mut u16,
        username_size: u32,
    ) -> i32;
    pub(super) fn CreateNamedPipeW(
        name: *const u16,
        open: u32,
        mode: u32,
        instances: u32,
        out: u32,
        input: u32,
        timeout: u32,
        attributes: *const SecurityAttributes,
    ) -> Handle;
    pub(super) fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        attributes: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    pub(super) fn ConnectNamedPipe(pipe: Handle, overlap: *mut Overlapped) -> i32;
    pub(super) fn CreateEventW(
        attributes: *const SecurityAttributes,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    pub(super) fn ResetEvent(event: Handle) -> i32;
    pub(super) fn ReadFile(
        pipe: Handle,
        buffer: *mut c_void,
        count: u32,
        read: *mut u32,
        overlap: *mut Overlapped,
    ) -> i32;
    pub(super) fn WriteFile(
        pipe: Handle,
        buffer: *const c_void,
        count: u32,
        written: *mut u32,
        overlap: *mut Overlapped,
    ) -> i32;
    pub(super) fn GetOverlappedResult(
        pipe: Handle,
        overlap: *mut Overlapped,
        bytes: *mut u32,
        wait: i32,
    ) -> i32;
    pub(super) fn CancelIoEx(pipe: Handle, overlap: *mut Overlapped) -> i32;
    pub(super) fn WaitForSingleObject(event: Handle, milliseconds: u32) -> u32;
    pub(super) fn CloseHandle(handle: Handle) -> i32;
    pub(super) fn LocalFree(buffer: *mut c_void) -> *mut c_void;
    pub(super) fn GetLastError() -> u32;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    pub(super) fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
}
#[link(name = "bcrypt")]
unsafe extern "system" {
    pub(super) fn BCryptGenRandom(
        algorithm: Handle,
        buffer: *mut u8,
        bytes: u32,
        flags: u32,
    ) -> i32;
}
