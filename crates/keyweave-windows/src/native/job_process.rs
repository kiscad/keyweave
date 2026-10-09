//! Owned suspended worker, atomically assigned to its Job at creation. No input APIs.
mod attributes;
mod channels;
use super::progress_pipe::{PipeReader, PipeWriter};
use attributes::Attributes;
pub use channels::ChannelCleanupReport;
use channels::ChildProgress;
use std::ffi::OsString;
use std::{
    ffi::c_void, marker::PhantomData, mem::size_of, path::Path, ptr::null_mut, rc::Rc,
    time::Duration,
};
type Handle = *mut c_void;
#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    minimum_ws: usize,
    maximum_ws: usize,
    active: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io: [u64; 6],
    memory: [usize; 4],
}
#[repr(C)]
struct Startup {
    size: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    numbers: [u32; 8],
    show: u16,
    reserved_bytes: u16,
    reserved_ptr: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}
#[repr(C)]
struct StartupEx {
    info: Startup,
    attributes: *mut c_void,
}
#[repr(C)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("invalid or excessive worker executable/arguments")]
    Command,
    #[error("invalid process attribute-list buffer size")]
    AttributeSize,
    #[error("progress endpoints must be distinct, idle, open pipe pairs")]
    PipeState,
    #[error("parent-side worker channel cleanup is incomplete")]
    ChannelCleanup,
    #[error("job process operation is not valid in its current state")]
    State,
    #[error("job operations are forbidden in a keyboard callback")]
    InCallback,
    #[error("{call} failed: {code}")]
    Api { call: &'static str, code: u32 },
    #[error("unexpected primary-thread suspension count: {0}")]
    SuspendCount(u32),
    #[error("unexpected process wait result: {0}")]
    Wait(u32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobProcessState {
    /// The Job exists; process creation has not succeeded.
    Prepared,
    Suspended,
    /// A child was created but startup failed; cleanup can never permit resume.
    StartupFailed,
    /// Resume was attempted; retrying could execute an unverified worker.
    ResumeAttempted,
    Running,
    StopRequested,
    Exited(u32),
}
#[derive(Debug, thiserror::Error)]
#[error("worker startup failed: {cause}; rollback: {rollback:?}")]
pub struct JobSpawnError {
    pub cause: JobError,
    pub rollback: Option<JobError>,
    pub channel_cleanup: Option<Box<ChannelCleanupReport>>,
    /// Partial resources remain owned. Prepared means no process was created.
    /// Other states may retain a child pending termination or observed exited.
    pub owner: Option<Box<OwnedJobProcess>>,
}
#[cfg(not(test))]
mod api {
    use super::*;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub(super) fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        pub(super) fn SetInformationJobObject(
            job: Handle,
            class: i32,
            information: *const c_void,
            bytes: u32,
        ) -> i32;
        pub(super) fn CreateProcessW(
            application: *const u16,
            command: *mut u16,
            process_attributes: *const c_void,
            thread_attributes: *const c_void,
            inherit: i32,
            flags: u32,
            environment: *const c_void,
            directory: *const u16,
            startup: *const Startup,
            process: *mut ProcessInfo,
        ) -> i32;
        pub(super) fn InitializeProcThreadAttributeList(
            list: *mut c_void,
            count: u32,
            flags: u32,
            size: *mut usize,
        ) -> i32;
        pub(super) fn UpdateProcThreadAttribute(
            list: *mut c_void,
            flags: u32,
            attribute: usize,
            value: *mut c_void,
            size: usize,
            previous: *mut c_void,
            returned: *mut usize,
        ) -> i32;
        pub(super) fn DeleteProcThreadAttributeList(list: *mut c_void);
        pub(super) fn ResumeThread(thread: Handle) -> u32;
        pub(super) fn TerminateJobObject(job: Handle, code: u32) -> i32;
        pub(super) fn WaitForSingleObject(process: Handle, milliseconds: u32) -> u32;
        pub(super) fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
        pub(super) fn CloseHandle(handle: Handle) -> i32;
        pub(super) fn SetHandleInformation(handle: Handle, mask: u32, flags: u32) -> i32;
        pub(super) fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            attributes: *const super::super::progress_pipe::SecurityAttributes,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        pub(super) fn GetLastError() -> u32;
    }
}
use api::*;
fn last_error_code() -> u32 {
    // SAFETY: immediate thread-local last error, no pointers or ownership.
    unsafe { GetLastError() }
}
fn error(call: &'static str) -> JobError {
    JobError::Api {
        call,
        code: last_error_code(),
    }
}
fn context() -> Result<(), JobError> {
    if super::hook_thread::outside_callback() {
        Ok(())
    } else {
        Err(JobError::InCallback)
    }
}
fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        value.encode_wide().take(32768).collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        value.to_string_lossy().encode_utf16().take(32768).collect()
    }
}
fn command(path: &Path, args: &[OsString]) -> Result<(Vec<u16>, Vec<u16>), JobError> {
    if !path.is_absolute()
        || !path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
        || args.len() > 128
    {
        return Err(JobError::Command);
    }
    let mut application = wide(path.as_os_str());
    if application.is_empty()
        || application.len() > 32766
        || application.contains(&0)
        || application.contains(&34)
    {
        return Err(JobError::Command);
    }
    let mut parts = vec![application.clone()];
    parts.extend(args.iter().map(|s| wide(s)));
    let line = quote(&parts)?;
    application.push(0);
    Ok((application, line))
}
fn quote(parts: &[Vec<u16>]) -> Result<Vec<u16>, JobError> {
    let mut out = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if part.len() > 32766 || part.contains(&0) {
            return Err(JobError::Command);
        }
        if index != 0 {
            out.push(32);
        }
        out.push(34);
        let mut slashes = 0;
        for &unit in part {
            if unit == 92 {
                slashes += 1;
                continue;
            }
            let count = if unit == 34 { slashes * 2 + 1 } else { slashes };
            out.extend(std::iter::repeat_n(92, count));
            out.push(unit);
            slashes = 0;
            if out.len() > 32766 {
                return Err(JobError::Command);
            }
        }
        out.extend(std::iter::repeat_n(92, slashes * 2));
        out.push(34);
        if out.len() > 32766 {
            return Err(JobError::Command);
        }
    }
    out.push(0);
    Ok(out)
}
pub struct OwnedJobProcess {
    job: Handle,
    process: Handle,
    thread: Handle,
    state: JobProcessState,
    channels: Option<Box<ChildProgress>>,
    channel_cleanup: Option<Box<ChannelCleanupReport>>,
    _thread: PhantomData<Rc<()>>,
}
impl std::fmt::Debug for OwnedJobProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedJobProcess")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}
impl OwnedJobProcess {
    pub fn spawn_suspended(path: &Path, args: &[OsString]) -> Result<Self, JobSpawnError> {
        Self::start(path, args, None)
    }
    /// Consumes the two idle worker ends; the caller retains the parent ends.
    /// stdin/stderr are reserved for binary progress, stdout is NUL. Must not
    /// overlap another ambient inheriting spawn on another parent thread.
    pub fn spawn_suspended_with_progress(
        path: &Path,
        args: &[OsString],
        reader: PipeReader,
        writer: PipeWriter,
    ) -> Result<Self, JobSpawnError> {
        Self::start(path, args, Some(ChildProgress::new(reader, writer)))
    }
    fn start(
        path: &Path,
        args: &[OsString],
        channels: Option<ChildProgress>,
    ) -> Result<Self, JobSpawnError> {
        Self::spawn(path, args, channels).map_err(|(cause, owner)| {
            let mut owner = owner;
            if let Some(owner) = owner.as_mut()
                && owner.state == JobProcessState::Suspended
            {
                owner.state = JobProcessState::StartupFailed;
            }
            let channel_cleanup = owner.as_mut().and_then(|owner| {
                if let Some(report) = owner.channel_cleanup.as_deref() {
                    Some(*report)
                } else if owner.channels.is_some() {
                    owner.finish_channel_transfer().ok()
                } else {
                    None
                }
            });
            let rollback = if let Some(owner) = owner.as_mut() {
                owner
                    .request_stop()
                    .and_then(|()| owner.poll_exit(Duration::from_millis(20)).map(|_| ()))
                    .err()
            } else {
                None
            };
            JobSpawnError {
                cause,
                rollback,
                channel_cleanup: channel_cleanup.map(Box::new),
                owner: owner.map(Box::new),
            }
        })
    }
    fn spawn(
        path: &Path,
        args: &[OsString],
        channels: Option<ChildProgress>,
    ) -> Result<Self, (JobError, Option<Self>)> {
        let mut owner = Self {
            job: null_mut(),
            process: null_mut(),
            thread: null_mut(),
            state: JobProcessState::Prepared,
            channels: channels.map(Box::new),
            channel_cleanup: None,
            _thread: PhantomData,
        };
        let early = context().and_then(|()| command(path, args));
        let (application, mut line) = match early {
            Ok(command) => command,
            Err(cause) => return Err((cause, owner.channels.is_some().then_some(owner))),
        };
        // SAFETY: null security attributes make the new unnamed Job handle
        // noninheritable; no existing named object or borrowed handle is opened.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err((
                error("CreateJobObjectW"),
                owner.channels.is_some().then_some(owner),
            ));
        }
        owner.job = job;
        let mut limits = ExtendedLimits::default();
        limits.basic.flags = 0x2008;
        limits.basic.active = 1;
        // SAFETY: initialized SDK-checked extended limit struct; class=9,
        // borrowed only for this synchronous call. No callback/user pointers.
        if unsafe {
            SetInformationJobObject(
                job,
                9,
                &limits as *const _ as *const c_void,
                size_of::<ExtendedLimits>() as u32,
            )
        } == 0
        {
            return Err((error("SetInformationJobObject"), Some(owner)));
        }
        let handles = if let Some(channels) = owner.channels.as_mut() {
            match channels.prepare() {
                Ok(handles) => Some(handles),
                Err(cause) => return Err((cause, Some(owner))),
            }
        } else {
            None
        };
        let mut attributes = match Attributes::new(job, handles) {
            Ok(attributes) => attributes,
            Err(cause) => return Err((cause, Some(owner))),
        };
        // SAFETY: these structs contain only scalar integers/raw pointers; zero
        // is valid for every field, no reference/enum/owned resource is created.
        let mut startup: StartupEx = unsafe { std::mem::zeroed() };
        startup.info.size = size_of::<StartupEx>() as u32;
        if let Some([stdin, stdout, stderr]) = handles {
            startup.info.numbers[7] = 0x100;
            startup.info.stdin = stdin;
            startup.info.stdout = stdout;
            startup.info.stderr = stderr;
        }
        startup.attributes = attributes.pointer();
        let mut info: ProcessInfo = unsafe { std::mem::zeroed() };
        // SAFETY: bounded nul-terminated absolute application and mutable command
        // buffers live through CreateProcessW; exact SDK STARTUPINFOEX/PROCESS_INFO.
        // Attribute storage and Job array stay pinned/alive through the call;
        // JOB_LIST assigns this private Job as part of process creation.
        // Inheritance only with our exact three-handle list; no supplied env/cwd.
        // plus no-window flags; returned handles are uniquely owned on success.
        if unsafe {
            CreateProcessW(
                application.as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                i32::from(handles.is_some()),
                0x08080004,
                std::ptr::null(),
                std::ptr::null(),
                &startup.info,
                &mut info,
            )
        } == 0
        {
            return Err((error("CreateProcessW"), Some(owner)));
        }
        owner.process = info.process;
        owner.thread = info.thread;
        owner.state = JobProcessState::Suspended;
        // Attribute values must remain alive until deletion, before closing
        // any of the handles referenced by HANDLE_LIST.
        drop(attributes);
        if owner.channels.is_some() {
            let report = match owner.finish_channel_transfer() {
                Ok(report) => report,
                Err(cause) => return Err((cause, Some(owner))),
            };
            if !report.complete() {
                return Err((JobError::ChannelCleanup, Some(owner)));
            }
        }
        Ok(owner)
    }
    /// Retry parent-side endpoint cleanup. A complete report does not prove
    /// child exit; failed startup still requires request_stop/poll_exit.
    pub fn finish_channel_transfer(&mut self) -> Result<ChannelCleanupReport, JobError> {
        context()?;
        let Some(channels) = self.channels.as_mut() else {
            return Ok(self.channel_cleanup.as_deref().copied().unwrap_or_default());
        };
        let report = channels.close();
        self.channel_cleanup = Some(Box::new(report));
        if report.complete() {
            self.channels = None;
        }
        Ok(report)
    }
    pub fn state(&self) -> JobProcessState {
        self.state
    }
    pub fn resume(&mut self) -> Result<(), JobError> {
        context()?;
        if self.state != JobProcessState::Suspended
            || self.thread.is_null()
            || self.channels.is_some()
        {
            return Err(JobError::State);
        }
        // SAFETY: uniquely retained primary thread, created suspended and already
        // assigned to the job. No other code can obtain or resume this handle.
        self.state = JobProcessState::ResumeAttempted;
        let previous = unsafe { ResumeThread(self.thread) };
        if previous != 1 {
            let cause = if previous == u32::MAX {
                error("ResumeThread")
            } else {
                JobError::SuspendCount(previous)
            };
            let _ = self.request_stop();
            return Err(cause);
        }
        self.state = JobProcessState::Running;
        self.close_thread()
    }
    pub fn request_stop(&mut self) -> Result<(), JobError> {
        context()?;
        if matches!(
            self.state,
            JobProcessState::StopRequested | JobProcessState::Exited(_)
        ) || self.process.is_null()
        {
            return Ok(());
        }
        // SAFETY: every successfully created process was atomically assigned
        // to this uniquely owned Job. No arbitrary PID or shared job is used.
        if unsafe { TerminateJobObject(self.job, 0xe04b5701) } == 0 {
            return Err(error("TerminateJobObject"));
        }
        self.state = JobProcessState::StopRequested;
        Ok(())
    }
    pub fn poll_exit(&mut self, timeout: Duration) -> Result<Option<u32>, JobError> {
        context()?;
        if let JobProcessState::Exited(code) = self.state {
            return Ok(Some(code));
        }
        if self.process.is_null() {
            return Ok(None);
        }
        let millis = timeout.as_millis().min(20) as u32;
        // SAFETY: uniquely owned process handle supports SYNCHRONIZE; finite
        // 0..20ms wait, handle remains alive throughout the synchronous call.
        match unsafe { WaitForSingleObject(self.process, millis) } {
            258 => Ok(None),
            0 => {
                let mut code = 0;
                // SAFETY: signaled owned process, initialized DWORD output; 259
                // can be a genuine exit value, never used here as liveness proof.
                if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
                    return Err(error("GetExitCodeProcess"));
                }
                self.state = JobProcessState::Exited(code);
                Ok(Some(code))
            }
            u32::MAX => Err(error("WaitForSingleObject")),
            value => Err(JobError::Wait(value)),
        }
    }
    fn close_thread(&mut self) -> Result<(), JobError> {
        if self.thread.is_null() {
            return Ok(());
        }
        // SAFETY: unique owned primary-thread handle, no raw pointer escapes.
        if unsafe { CloseHandle(self.thread) } == 0 {
            return Err(error("CloseHandle(thread)"));
        }
        self.thread = null_mut();
        Ok(())
    }
}
impl Drop for OwnedJobProcess {
    fn drop(&mut self) {
        let _ = self.request_stop();
        let _ = self.poll_exit(Duration::from_millis(20));
        let _ = self.finish_channel_transfer();
        let _ = self.close_thread();
        // SAFETY: this uniquely owned private job closes before the process
        // handle, so KILL_ON_JOB_CLOSE remains a fallback when termination failed.
        // No handle copies exist. Drop is not an exit acknowledgement.
        if !self.job.is_null() {
            unsafe {
                CloseHandle(self.job);
            };
            self.job = null_mut();
        }
        let _ = self.poll_exit(Duration::from_millis(20));
        if !self.process.is_null() {
            // SAFETY: unique process handle is no longer needed after bounded
            // shutdown attempts. Closing it is not proof of process exit.
            unsafe {
                CloseHandle(self.process);
            };
            self.process = null_mut();
        }
    }
}
#[cfg(test)]
mod api;
#[cfg(test)]
mod tests;
