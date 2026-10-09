#![allow(non_snake_case, clippy::too_many_arguments)]
use super::*;
use std::cell::RefCell;
#[derive(Default)]
pub(super) struct Mock {
    pub fail: Option<&'static str>,
    pub last: u32,
    pub calls: Vec<&'static str>,
    pub closes: Vec<usize>,
    pub parent: Option<u32>,
    pub missing_current: bool,
    pub flags: Option<u32>,
    pub active: Option<u32>,
    pub list_pid: Option<usize>,
    pub list_assigned: Option<u32>,
    pub list_count: Option<u32>,
    pub job_length: Option<u32>,
    pub startup_flags: Option<u32>,
    pub std_mismatch: bool,
    pub file_type: Option<u32>,
    pub pipe_flags: Option<u32>,
    pub pipe_mode: Option<u32>,
    pub instances: Option<u32>,
    pub peer: Option<u32>,
    pub native_mode: Option<u32>,
    pub access: Option<u32>,
    pub same_name: bool,
    pub bad_name: bool,
    pub status: Option<i32>,
    pub information: Option<usize>,
    pub io_status: Option<i32>,
    pub query_class: Option<i32>,
    pub query_handle: Option<usize>,
    pub queries: Vec<(usize, i32)>,
    pub retained: Option<(usize, usize)>,
    pub duplicates: Vec<usize>,
}
thread_local! {pub(super) static MOCK:RefCell<Mock>=RefCell::new(Mock::default());}
fn call(name: &'static str) -> bool {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls.push(name);
        if m.fail == Some(name) {
            m.last = 5;
            false
        } else {
            true
        }
    })
}
pub(super) unsafe fn GetLastError() -> u32 {
    MOCK.with(|m| m.borrow().last)
}
pub(super) unsafe fn GetCurrentProcessId() -> u32 {
    call("self-pid");
    77
}
pub(super) unsafe fn GetCurrentProcess() -> Handle {
    usize::MAX as Handle
}
pub(super) unsafe fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle {
    assert_eq!((flags, pid), (2, 0));
    if call("snapshot") {
        600usize as Handle
    } else {
        usize::MAX as Handle
    }
}
pub(super) unsafe fn Process32FirstW(snapshot: Handle, entry: *mut ProcessEntry) -> i32 {
    assert_eq!(snapshot as usize, 600);
    if !call("process-first") {
        return 0;
    }
    MOCK.with(|m| {
        let m = m.borrow();
        // SAFETY: caller supplies complete initialized ProcessEntry output.
        unsafe {
            assert_eq!((*entry).size as usize, size_of::<ProcessEntry>());
            (*entry).pid = if m.missing_current { 88 } else { 77 };
            (*entry).parent = m.parent.unwrap_or(76);
        }
    });
    1
}
pub(super) unsafe fn Process32NextW(snapshot: Handle, _entry: *mut ProcessEntry) -> i32 {
    assert_eq!(snapshot as usize, 600);
    call("process-next");
    MOCK.with(|m| m.borrow_mut().last = 18);
    0
}
pub(super) unsafe fn CloseHandle(handle: Handle) -> i32 {
    assert_eq!(handle as usize, 600);
    MOCK.with(|m| m.borrow_mut().closes.push(600));
    i32::from(call("close-snapshot"))
}
pub(super) unsafe fn QueryInformationJobObject(
    job: Handle,
    class: i32,
    info: *mut c_void,
    bytes: u32,
    length: *mut u32,
) -> i32 {
    assert!(job.is_null());
    if !call(if class == 9 {
        "job-limits"
    } else {
        "job-processes"
    }) {
        return 0;
    }
    MOCK.with(|m| {
        let m = m.borrow();
        // SAFETY: caller passes the matching live SDK-sized typed outputs.
        unsafe {
            *length = m.job_length.unwrap_or(bytes);
            if class == 9 {
                assert_eq!(bytes as usize, size_of::<ExtendedLimits>());
                let v = &mut *info.cast::<ExtendedLimits>();
                v.basic.flags = m.flags.unwrap_or(0x2008);
                v.basic.active = m.active.unwrap_or(1);
            } else {
                assert_eq!(class, 3);
                assert_eq!(bytes as usize, size_of::<ProcessList>());
                *info.cast::<ProcessList>() = ProcessList {
                    assigned: m.list_assigned.unwrap_or(1),
                    count: m.list_count.unwrap_or(1),
                    ids: [m.list_pid.unwrap_or(77)],
                };
            }
        }
    });
    1
}
pub(super) unsafe fn GetStartupInfoW(info: *mut Startup) {
    call("startup");
    MOCK.with(|m| {
        let m = m.borrow();
        // SAFETY: complete initialized STARTUPINFO output remains live.
        unsafe {
            (*info).size = size_of::<Startup>() as u32;
            (*info).numbers[7] = m.startup_flags.unwrap_or(0x100);
            (*info).stdin = 100usize as Handle;
            (*info).stdout = 400usize as Handle;
            (*info).stderr = 210usize as Handle;
        }
    });
}
pub(super) unsafe fn GetStdHandle(kind: u32) -> Handle {
    call("std");
    match kind {
        0xfffffff6 => 100usize as Handle,
        0xfffffff5 => 400usize as Handle,
        0xfffffff4 => MOCK.with(|m| {
            if m.borrow().std_mismatch {
                211usize as Handle
            } else {
                210usize as Handle
            }
        }),
        _ => panic!("std kind"),
    }
}
pub(super) unsafe fn GetFileType(handle: Handle) -> u32 {
    call("file-type");
    MOCK.with(|m| {
        m.borrow()
            .file_type
            .unwrap_or(if handle as usize == 400 { 2 } else { 3 })
    })
}
pub(super) unsafe fn GetNamedPipeInfo(
    h: Handle,
    flags: *mut u32,
    out: *mut u32,
    input: *mut u32,
    instances: *mut u32,
) -> i32 {
    assert!(out.is_null() && input.is_null());
    if !call("pipe-info") {
        return 0;
    }
    MOCK.with(|m| {
        let m = m.borrow();
        // SAFETY: caller supplies initialized scalar outputs.
        unsafe {
            *flags = m
                .pipe_flags
                .unwrap_or(if h as usize == 100 { 9 } else { 8 });
            *instances = m.instances.unwrap_or(1);
        }
    });
    1
}
pub(super) unsafe fn GetNamedPipeHandleStateW(
    _h: Handle,
    state: *mut u32,
    instances: *mut u32,
    count: *mut u32,
    timeout: *mut u32,
    user: *mut u16,
    size: u32,
) -> i32 {
    assert!(instances.is_null() && count.is_null() && timeout.is_null() && user.is_null());
    assert_eq!(size, 0);
    if !call("pipe-state") {
        return 0;
    }
    // SAFETY: live scalar state output supplied by caller.
    unsafe {
        *state = MOCK.with(|m| m.borrow().pipe_mode.unwrap_or(0));
    }
    1
}
pub(super) unsafe fn GetNamedPipeServerProcessId(_h: Handle, pid: *mut u32) -> i32 {
    if !call("pipe-server") {
        return 0;
    }
    // SAFETY: live scalar PID output.
    unsafe {
        *pid = MOCK.with(|m| m.borrow().peer.unwrap_or(76));
    }
    1
}
pub(super) unsafe fn GetNamedPipeClientProcessId(_h: Handle, pid: *mut u32) -> i32 {
    if !call("pipe-client") {
        return 0;
    }
    // SAFETY: live scalar PID output.
    unsafe {
        *pid = MOCK.with(|m| m.borrow().peer.unwrap_or(76));
    }
    1
}
pub(super) unsafe fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32 {
    assert!(matches!(handle as usize, 100 | 210 | 400));
    assert_eq!((mask, flags), (1, 0));
    i32::from(call("clear-inherit"))
}
pub(super) unsafe fn DuplicateHandle(
    source_process: Handle,
    source: Handle,
    target_process: Handle,
    target: *mut Handle,
    access: u32,
    inherit: i32,
    flags: u32,
) -> i32 {
    assert_eq!(source_process as usize, usize::MAX);
    assert_eq!(target_process, source_process);
    assert_eq!((access, inherit, flags), (0, 0, 2));
    assert!(matches!(source as usize, 100 | 210));
    if !call("duplicate") {
        return 0;
    }
    MOCK.with(|m| m.borrow_mut().duplicates.push(source as usize));
    // SAFETY: live initialized handle output; no original std handle is returned.
    unsafe {
        *target = if source as usize == 100 {
            501usize as Handle
        } else {
            502usize as Handle
        };
    }
    1
}
pub(super) unsafe fn NtQueryInformationFile(
    h: Handle,
    io: *mut IoStatusBlock,
    out: *mut c_void,
    length: u32,
    class: i32,
) -> i32 {
    assert!(matches!(h as usize, 100 | 210));
    assert!(matches!((class, length), (8 | 16, 4) | (9, 256)));
    call("native-query");
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.queries.push((h as usize, class));
        let selected = m.query_class.is_none_or(|value| value == class)
            && m.query_handle.is_none_or(|value| value == h as usize);
        if selected && let Some(status) = m.status {
            m.retained = Some((io as usize, out as usize));
            return status;
        }
        let mut data = Vec::new();
        if class == 16 {
            data.extend_from_slice(
                &if selected {
                    m.native_mode.unwrap_or(0)
                } else {
                    0u32
                }
                .to_le_bytes(),
            );
        }
        if class == 8 {
            data.extend_from_slice(
                &if selected { m.access } else { None }
                    .unwrap_or(if h as usize == 100 {
                        0x1a0089
                    } else {
                        0x120196
                    })
                    .to_le_bytes(),
            );
        }
        if class == 9 {
            let digit = if m.bad_name {
                'Z'
            } else if h as usize == 100 || m.same_name {
                '1'
            } else {
                '2'
            };
            let name = format!("\\keyweave-progress-{}", digit.to_string().repeat(32));
            let units: Vec<u16> = name.encode_utf16().collect();
            data.extend_from_slice(&((units.len() * 2) as u32).to_le_bytes());
            for u in units {
                data.extend_from_slice(&u.to_le_bytes());
            }
        }
        // SAFETY: caller retains stable initialized output arrays and status of
        // the specified length. Copy is within that bound and completes here.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), out.cast(), data.len());
            (*io).status.status = if selected {
                m.io_status.unwrap_or(0)
            } else {
                0
            };
            (*io).information = if selected {
                m.information.unwrap_or(data.len())
            } else {
                data.len()
            };
        }
        0
    })
}
