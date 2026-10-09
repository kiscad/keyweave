#![allow(non_snake_case, clippy::too_many_arguments)]
use super::*;
use std::cell::RefCell;
#[derive(Default)]
pub(super) struct Mock {
    pub last_error: u32,
    pub query_bytes: Option<usize>,
    pub query_success: bool,
    pub initialized_list: usize,
    pub job_value: usize,
    pub fail: Option<&'static str>,
    pub calls: Vec<&'static str>,
    pub resumed: bool,
    pub wait: u32,
    pub exit: u32,
    pub resume_count: Option<u32>,
    pub limits: Option<(u32, u32)>,
    pub waits: Vec<u32>,
    pub closes: Vec<usize>,
}
thread_local! {pub(super) static MOCK: RefCell<Mock> = RefCell::new(Mock {wait:258, ..Mock::default()});}
fn call(name: &'static str) -> bool {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.calls.push(name);
        let success = m.fail != Some(name);
        if !success {
            m.last_error = 5;
        }
        success
    })
}
pub(super) unsafe fn GetLastError() -> u32 {
    MOCK.with(|m| m.borrow().last_error)
}
pub(super) unsafe fn CreateJobObjectW(a: *const c_void, n: *const u16) -> Handle {
    assert!(a.is_null() && n.is_null());
    if call("job") {
        1usize as Handle
    } else {
        null_mut()
    }
}
pub(super) unsafe fn SetInformationJobObject(
    job: Handle,
    class: i32,
    info: *const c_void,
    bytes: u32,
) -> i32 {
    assert_eq!(job as usize, 1);
    assert_eq!(class, 9);
    assert_eq!(bytes as usize, size_of::<ExtendedLimits>());
    // SAFETY: tested caller passes its initialized extended limits for this call.
    let limits = unsafe { &*info.cast::<ExtendedLimits>() };
    MOCK.with(|m| m.borrow_mut().limits = Some((limits.basic.flags, limits.basic.active)));
    i32::from(call("limits"))
}
pub(super) unsafe fn CreateProcessW(
    app: *const u16,
    line: *mut u16,
    pa: *const c_void,
    ta: *const c_void,
    inherit: i32,
    flags: u32,
    env: *const c_void,
    dir: *const u16,
    start: *const Startup,
    info: *mut ProcessInfo,
) -> i32 {
    assert!(!app.is_null() && !line.is_null());
    assert!(pa.is_null() && ta.is_null() && env.is_null() && dir.is_null());
    assert_eq!((inherit, flags), (0, 0x08080004));
    // SAFETY: initialized input and output are live for synchronous mock call.
    assert_eq!(unsafe { (*start).size } as usize, size_of::<StartupEx>());
    // SAFETY: caller passes a complete StartupEx via its repr(C) first field.
    let ex = unsafe { &*start.cast::<StartupEx>() };
    MOCK.with(|m| {
        let m = m.borrow();
        assert_ne!(m.initialized_list, 0);
        assert_eq!(ex.attributes as usize, m.initialized_list);
        assert_ne!(m.job_value, 0);
        // SAFETY: live one-HANDLE array owned by Attributes through Create call.
        assert_eq!(unsafe { *(m.job_value as *const Handle) } as usize, 1);
    });
    if !call("create") {
        return 0;
    }
    unsafe {
        *info = ProcessInfo {
            process: 2usize as Handle,
            thread: 3usize as Handle,
            process_id: 20,
            thread_id: 30,
        };
    }
    1
}
pub(super) unsafe fn InitializeProcThreadAttributeList(
    list: *mut c_void,
    count: u32,
    flags: u32,
    bytes: *mut usize,
) -> i32 {
    assert_eq!((count, flags), (1, 0));
    if list.is_null() {
        let ok = call("attribute-size");
        MOCK.with(|m| {
            let mut m = m.borrow_mut();
            // SAFETY: caller provides a live initialized SIZE_T for this query.
            unsafe {
                *bytes = m.query_bytes.unwrap_or(96);
            }
            if ok {
                m.last_error = 122;
            }
            i32::from(m.query_success)
        })
    } else {
        assert_eq!(list as usize % 16, 0);
        if !call("attribute-init") {
            return 0;
        }
        MOCK.with(|m| m.borrow_mut().initialized_list = list as usize);
        1
    }
}
pub(super) unsafe fn UpdateProcThreadAttribute(
    list: *mut c_void,
    flags: u32,
    attribute: usize,
    value: *mut c_void,
    bytes: usize,
    previous: *mut c_void,
    returned: *mut usize,
) -> i32 {
    assert_eq!((flags, attribute, bytes), (0, 0x2000d, size_of::<Handle>()));
    assert!(previous.is_null() && returned.is_null());
    assert_eq!(list as usize, MOCK.with(|m| m.borrow().initialized_list));
    assert!(!value.is_null());
    // SAFETY: mock caller supplies a live, aligned one-HANDLE array.
    assert_eq!(unsafe { *value.cast::<Handle>() } as usize, 1);
    if !call("attribute-job") {
        return 0;
    }
    MOCK.with(|m| m.borrow_mut().job_value = value as usize);
    1
}
pub(super) unsafe fn DeleteProcThreadAttributeList(list: *mut c_void) {
    call("attribute-delete");
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        assert_eq!(list as usize, m.initialized_list);
        assert_ne!(m.initialized_list, 0);
        if m.job_value != 0 {
            // SAFETY: owned array must remain alive through list deletion.
            assert_eq!(unsafe { *(m.job_value as *const Handle) } as usize, 1);
        }
        m.initialized_list = 0;
        m.job_value = 0;
    });
}
pub(super) unsafe fn ResumeThread(thread: Handle) -> u32 {
    assert_eq!(thread as usize, 3);
    if !call("resume") {
        return u32::MAX;
    }
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.resumed = true;
        m.resume_count.unwrap_or(1)
    })
}
pub(super) unsafe fn TerminateJobObject(job: Handle, _: u32) -> i32 {
    assert_eq!(job as usize, 1);
    i32::from(call("kill-job"))
}
pub(super) unsafe fn WaitForSingleObject(process: Handle, timeout: u32) -> u32 {
    assert_eq!(process as usize, 2);
    MOCK.with(|m| m.borrow_mut().waits.push(timeout));
    if !call("wait") {
        u32::MAX
    } else {
        MOCK.with(|m| m.borrow().wait)
    }
}
pub(super) unsafe fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32 {
    assert_eq!(process as usize, 2);
    if !call("exit") {
        return 0;
    } // SAFETY: caller supplies live initialized DWORD.
    unsafe {
        *code = MOCK.with(|m| m.borrow().exit);
    }
    1
}
pub(super) unsafe fn CloseHandle(handle: Handle) -> i32 {
    MOCK.with(|m| m.borrow_mut().closes.push(handle as usize));
    i32::from(call("close"))
}
