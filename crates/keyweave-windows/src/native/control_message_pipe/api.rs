use super::*;
#[link(name = "kernel32")]
unsafe extern "system" {
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
    pub(super) fn CreateEventW(
        attributes: *const SecurityAttributes,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    pub(super) fn SetNamedPipeHandleState(
        pipe: Handle,
        mode: *const u32,
        count: *const u32,
        timeout: *const u32,
    ) -> i32;
    pub(super) fn GetNamedPipeInfo(
        pipe: Handle,
        flags: *mut u32,
        out: *mut u32,
        input: *mut u32,
        instances: *mut u32,
    ) -> i32;
    pub(super) fn GetNamedPipeHandleStateW(
        pipe: Handle,
        state: *mut u32,
        instances: *mut u32,
        count: *mut u32,
        timeout: *mut u32,
        user: *mut u16,
        length: u32,
    ) -> i32;
    pub(super) fn GetHandleInformation(handle: Handle, flags: *mut u32) -> i32;
    pub(super) fn ResetEvent(event: Handle) -> i32;
    pub(super) fn ConnectNamedPipe(pipe: Handle, overlap: *mut Overlapped) -> i32;
    pub(super) fn DisconnectNamedPipe(pipe: Handle) -> i32;
    pub(super) fn ReadFile(
        pipe: Handle,
        buffer: *mut c_void,
        count: u32,
        bytes: *mut u32,
        overlap: *mut Overlapped,
    ) -> i32;
    pub(super) fn WriteFile(
        pipe: Handle,
        buffer: *const c_void,
        count: u32,
        bytes: *mut u32,
        overlap: *mut Overlapped,
    ) -> i32;
    pub(super) fn GetOverlappedResult(
        pipe: Handle,
        overlap: *mut Overlapped,
        bytes: *mut u32,
        wait: i32,
    ) -> i32;
    pub(super) fn CancelIoEx(pipe: Handle, overlap: *mut Overlapped) -> i32;
    pub(super) fn WaitForSingleObject(event: Handle, ms: u32) -> u32;
    pub(super) fn CloseHandle(handle: Handle) -> i32;
    pub(super) fn GetLastError() -> u32;
}
