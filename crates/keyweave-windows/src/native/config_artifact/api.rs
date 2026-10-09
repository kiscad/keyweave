use super::super::progress_pipe::Overlapped;
use super::*;
#[link(name = "kernel32")]
unsafe extern "system" {
    pub(super) fn GetLastError() -> u32;
    pub(super) fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *const SecurityAttributes,
        creation: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    pub(super) fn CloseHandle(handle: Handle) -> i32;
    pub(super) fn GetFileType(handle: Handle) -> u32;
    pub(super) fn GetHandleInformation(handle: Handle, flags: *mut u32) -> i32;
    pub(super) fn GetFinalPathNameByHandleW(
        handle: Handle,
        path: *mut u16,
        capacity: u32,
        flags: u32,
    ) -> u32;
    pub(super) fn GetVolumePathNameW(path: *const u16, volume: *mut u16, capacity: u32) -> i32;
    pub(super) fn GetDriveTypeW(root: *const u16) -> u32;
    pub(super) fn GetVolumeInformationW(
        root: *const u16,
        name: *mut u16,
        name_size: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs_name: *mut u16,
        fs_size: u32,
    ) -> i32;
    pub(super) fn ReadFile(
        handle: Handle,
        buffer: *mut c_void,
        size: u32,
        read: *mut u32,
        overlap: *mut Overlapped,
    ) -> i32;
    pub(super) fn WriteFile(
        handle: Handle,
        buffer: *const c_void,
        size: u32,
        written: *mut u32,
        overlap: *mut Overlapped,
    ) -> i32;
    pub(super) fn DuplicateHandle(
        source: Handle,
        file: Handle,
        target: Handle,
        duplicate: *mut Handle,
        access: u32,
        inherit: i32,
        flags: u32,
    ) -> i32;
    pub(super) fn SetFilePointerEx(
        file: Handle,
        distance: LargeInteger,
        new_position: *mut LargeInteger,
        origin: u32,
    ) -> i32;
    pub(super) fn LocalFree(pointer: *mut c_void) -> *mut c_void;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    pub(super) fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        descriptor: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
}
#[link(name = "bcrypt")]
unsafe extern "system" {
    pub(super) fn BCryptGenRandom(algorithm: Handle, buffer: *mut u8, size: u32, flags: u32)
    -> i32;
}
