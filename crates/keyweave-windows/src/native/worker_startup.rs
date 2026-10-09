//! One-shot worker admission before input or other threads. No input APIs.
use super::{
    job_process::{ExtendedLimits, Startup},
    progress_pipe::{PipeReader, PipeWriter},
};
use std::{
    cell::UnsafeCell, ffi::c_void, marker::PhantomData, mem::size_of, ptr::null_mut, rc::Rc,
};
type Handle = *mut c_void;
#[cfg(not(test))]
mod api;
#[cfg(test)]
mod api_mock;
use api::*;
#[cfg(test)]
use api_mock as api;
#[cfg(not(test))]
static CLAIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
thread_local! { static CLAIMED: std::cell::Cell<bool> = const {std::cell::Cell::new(false)}; }

#[derive(Debug, thiserror::Error)]
pub enum WorkerStartupError {
    #[error("worker startup forbidden in keyboard callback")]
    InCallback,
    #[error("worker startup already attempted; retry is forbidden")]
    AlreadyClaimed,
    #[error("invalid worker parent/run identity")]
    Identity,
    #[error("worker creating parent differs or is missing from bounded snapshot")]
    Parent,
    #[error("worker Job protection or membership does not match startup contract")]
    JobProtection,
    #[error("worker standard handles do not match startup contract")]
    StandardHandles,
    #[error("worker pipe metadata, creator or peer does not match startup contract")]
    PipeMetadata,
    #[error("worker progress pipe is synchronous or has unexpected mode")]
    PipeMode,
    #[error("worker progress pipe access direction is invalid")]
    PipeAccess,
    #[error("worker progress pipe name or distinct identity is invalid")]
    PipeIdentity,
    #[error("config file claim was already attempted")]
    ConfigClaimed,
    #[error("inherited config handle, mode or access differs from startup contract")]
    ConfigHandle,
    #[error("inherited config metadata failed: {0}")]
    Config(#[from] super::config_artifact::ArtifactError),
    #[error("{call} failed: {code}")]
    Api { call: &'static str, code: u32 },
    #[error("Native file query class {class} returned {status:#010x}; outputs retained until exit")]
    NativeQuery { class: i32, status: i32 },
    #[error("Native file query returned invalid completion or length")]
    NativeReply,
    #[error("private progress endpoint construction failed: {0}")]
    PipeConstruction(#[from] std::io::Error),
}
fn error(call: &'static str) -> WorkerStartupError {
    // SAFETY: immediate same-thread last-error scalar.
    WorkerStartupError::Api {
        call,
        code: unsafe { GetLastError() },
    }
}
fn claim_once() -> bool {
    #[cfg(not(test))]
    {
        !CLAIMED.swap(true, std::sync::atomic::Ordering::AcqRel)
    }
    #[cfg(test)]
    {
        !CLAIMED.replace(true)
    }
}
#[repr(C)]
struct ProcessEntry {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    parent: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
}
#[repr(C)]
struct ProcessList {
    assigned: u32,
    count: u32,
    ids: [usize; 1],
}
#[repr(C)]
union IoStatus {
    status: i32,
    pointer: *mut c_void,
}
#[repr(C)]
struct IoStatusBlock {
    status: IoStatus,
    information: usize,
}
#[repr(C)]
struct Query {
    status: UnsafeCell<IoStatusBlock>,
    bytes: UnsafeCell<[u8; 256]>,
}
struct Reply {
    bytes: [u8; 256],
    count: usize,
}
fn query(handle: Handle, class: i32, length: u32) -> Result<Reply, WorkerStartupError> {
    if !matches!((class, length), (8 | 16, 4) | (9, 256)) {
        return Err(WorkerStartupError::NativeReply);
    }
    let owner = Box::new(Query {
        status: UnsafeCell::new(IoStatusBlock {
            status: IoStatus {
                pointer: null_mut(),
            },
            information: 0,
        }),
        bytes: UnsafeCell::new([0; 256]),
    });
    // SAFETY: borrowed original std pipe stays open for process lifetime;
    // stable heap outputs with UnsafeCell, bounded 4/256-byte request. No data IO
    // has started. Nonzero return retains memory without reading OS outputs.
    let status = unsafe {
        NtQueryInformationFile(
            handle,
            owner.status.get(),
            owner.bytes.get().cast(),
            length,
            class,
        )
    };
    if status != 0 {
        std::mem::forget(owner);
        return Err(WorkerStartupError::NativeQuery { class, status });
    }
    // SAFETY: STATUS_SUCCESS is final completion, so OS no longer accesses
    // these outputs. Union Status is the documented completion status field.
    let io = unsafe { &*owner.status.get() };
    if unsafe { io.status.status } != 0 || io.information == 0 || io.information > length as usize {
        return Err(WorkerStartupError::NativeReply);
    }
    // SAFETY: confirmed completion; whole buffer was initialized before the API.
    Ok(Reply {
        bytes: unsafe { *owner.bytes.get() },
        count: io.information,
    })
}
fn scalar(handle: Handle, class: i32) -> Result<u32, WorkerStartupError> {
    let reply = query(handle, class, 4)?;
    if reply.count != 4 {
        return Err(WorkerStartupError::NativeReply);
    }
    Ok(u32::from_le_bytes([
        reply.bytes[0],
        reply.bytes[1],
        reply.bytes[2],
        reply.bytes[3],
    ]))
}
fn pipe_identity(handle: Handle) -> Result<[u8; 16], WorkerStartupError> {
    let reply = query(handle, 9, 256)?;
    const PREFIX: &str = "\\keyweave-progress-";
    let expected = (PREFIX.len() + 32) * 2;
    let length = u32::from_le_bytes([
        reply.bytes[0],
        reply.bytes[1],
        reply.bytes[2],
        reply.bytes[3],
    ]) as usize;
    if reply.count != expected + 4 || length != expected {
        return Err(WorkerStartupError::PipeIdentity);
    }
    let unit = |i: usize| u16::from_le_bytes([reply.bytes[4 + i * 2], reply.bytes[5 + i * 2]]);
    if !PREFIX
        .bytes()
        .enumerate()
        .all(|(i, b)| unit(i) == u16::from(b))
    {
        return Err(WorkerStartupError::PipeIdentity);
    }
    let nibble = |i| match unit(i) {
        48..=57 => Ok((unit(i) - 48) as u8),
        97..=102 => Ok((unit(i) - 87) as u8),
        _ => Err(WorkerStartupError::PipeIdentity),
    };
    let mut id = [0; 16];
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = (nibble(PREFIX.len() + i * 2)? << 4) | nibble(PREFIX.len() + i * 2 + 1)?;
    }
    Ok(id)
}
struct Snapshot(Handle);
impl Snapshot {
    fn close(&mut self) -> Result<(), WorkerStartupError> {
        if self.0.is_null() {
            return Ok(());
        }
        // SAFETY: unique snapshot, no enumeration call remains active.
        if unsafe { CloseHandle(self.0) } == 0 {
            return Err(error("CloseHandle(snapshot)"));
        }
        self.0 = null_mut();
        Ok(())
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn verify_parent(pid: u32, parent: u32) -> Result<(), WorkerStartupError> {
    // SAFETY: process-only read snapshot; no target process handles or input.
    let handle = unsafe { CreateToolhelp32Snapshot(2, 0) };
    if handle as usize == usize::MAX {
        return Err(error("CreateToolhelp32Snapshot"));
    }
    let mut snapshot = Snapshot(handle);
    // SAFETY: C struct contains only integers and array; fully initialized.
    let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
    entry.size = size_of::<ProcessEntry>() as u32;
    let result = (|| {
        // SAFETY: unique snapshot and complete initialized output of SDK size.
        if unsafe { Process32FirstW(handle, &mut entry) } == 0 {
            return Err(error("Process32FirstW"));
        }
        for _ in 0..4096 {
            if entry.size as usize != size_of::<ProcessEntry>() {
                return Err(WorkerStartupError::Parent);
            }
            if entry.pid == pid {
                return if entry.parent == parent {
                    Ok(())
                } else {
                    Err(WorkerStartupError::Parent)
                };
            }
            // SAFETY: same live snapshot/output. exe/name fields never read.
            if unsafe { Process32NextW(handle, &mut entry) } == 0 {
                return Err(error("Process32NextW"));
            }
        }
        Err(WorkerStartupError::Parent)
    })();
    let cleanup = snapshot.close();
    result.and(cleanup)
}
fn verify_job(pid: u32) -> Result<(), WorkerStartupError> {
    let mut limits = ExtendedLimits::default();
    let mut length = 0;
    // SAFETY: NULL selects immediate current Job without taking a handle;
    // initialized SDK-sized limit struct and scalar return length.
    if unsafe {
        QueryInformationJobObject(
            null_mut(),
            9,
            (&mut limits as *mut ExtendedLimits).cast(),
            size_of::<ExtendedLimits>() as u32,
            &mut length,
        )
    } == 0
    {
        return Err(error("QueryInformationJobObject(limits)"));
    }
    if length as usize != size_of::<ExtendedLimits>()
        || limits.basic.flags != 0x2008
        || limits.basic.active != 1
    {
        return Err(WorkerStartupError::JobProtection);
    }
    let mut list = ProcessList {
        assigned: 0,
        count: 0,
        ids: [0],
    };
    length = 0;
    // SAFETY: one-process contract; bounded SDK list fits exactly one PID.
    if unsafe {
        QueryInformationJobObject(
            null_mut(),
            3,
            (&mut list as *mut ProcessList).cast(),
            size_of::<ProcessList>() as u32,
            &mut length,
        )
    } == 0
    {
        return Err(error("QueryInformationJobObject(processes)"));
    }
    if length as usize != size_of::<ProcessList>()
        || list.assigned != 1
        || list.count != 1
        || list.ids[0] != pid as usize
    {
        return Err(WorkerStartupError::JobProtection);
    }
    Ok(())
}
fn inspect_pipe(handle: Handle, server: bool, parent: u32) -> Result<[u8; 16], WorkerStartupError> {
    let mut flags = 0;
    let mut instances = 0;
    let mut state = 0;
    // SAFETY: validated borrowed PIPE std handle, only scalar outputs.
    if unsafe { GetNamedPipeInfo(handle, &mut flags, null_mut(), null_mut(), &mut instances) } == 0
    {
        return Err(error("GetNamedPipeInfo"));
    }
    if flags != (8 | u32::from(server)) || instances != 1 {
        return Err(WorkerStartupError::PipeMetadata);
    }
    // SAFETY: only state, all collection/account outputs NULL.
    if unsafe {
        GetNamedPipeHandleStateW(
            handle,
            &mut state,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            0,
        )
    } == 0
    {
        return Err(error("GetNamedPipeHandleStateW"));
    }
    if state != 0 {
        return Err(WorkerStartupError::PipeMetadata);
    }
    let mut creator = 0;
    let mut peer = 0;
    // SAFETY: local pipe instance; borrowed scalar PID outputs only.
    if unsafe { GetNamedPipeServerProcessId(handle, &mut creator) } == 0 {
        return Err(error("GetNamedPipeServerProcessId"));
    }
    if unsafe { GetNamedPipeClientProcessId(handle, &mut peer) } == 0 {
        return Err(error("GetNamedPipeClientProcessId"));
    }
    if creator != parent || peer != parent {
        return Err(WorkerStartupError::PipeMetadata);
    }
    if scalar(handle, 16)? != 0 {
        return Err(WorkerStartupError::PipeMode);
    }
    let access = scalar(handle, 8)?;
    if access & 3 != if server { 1 } else { 2 } || access & 0x80 == 0 {
        return Err(WorkerStartupError::PipeAccess);
    }
    pipe_identity(handle)
}
fn duplicate(handle: Handle) -> Result<Handle, WorkerStartupError> {
    let mut copy = null_mut();
    // SAFETY: current-process pseudo handle is borrowed, never closed/exported.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: only verified progress pipe is duplicated, SAME_ACCESS and no
    // inheritance. Original std handle remains borrowed until process exit.
    if unsafe { DuplicateHandle(process, handle, process, &mut copy, 0, 0, 2) } == 0 {
        return Err(error("DuplicateHandle(progress)"));
    }
    Ok(copy)
}
/// Metadata-only native diagnostic. Creates private synchronous pipes, confirms
/// worker inspection rejects their mode, then closes them; never starts data IO.
pub fn check_synchronous_progress_pipe_rejected() -> Result<(), WorkerStartupError> {
    if !super::outside_callback() {
        return Err(WorkerStartupError::InCallback);
    }
    let (mut reader, mut writer) = PipeReader::synchronous_fixture()?;
    // SAFETY: current-process scalar is only used for our own fixture peer.
    let parent = unsafe { GetCurrentProcessId() };
    let result = inspect_pipe(reader.transfer_state().0, true, parent);
    let read_cleanup = reader.close()?;
    let write_cleanup = writer.close()?;
    if !read_cleanup.complete() || !write_cleanup.complete() {
        return Err(WorkerStartupError::PipeMetadata);
    }
    match result {
        Err(WorkerStartupError::PipeMode) => Ok(()),
        Err(cause) => Err(cause),
        Ok(_) => Err(WorkerStartupError::PipeMode),
    }
}
pub struct WorkerParentGuard {
    parent: u32,
    pid: u32,
    std_handles: [usize; 3],
    config_attempted: bool,
    _thread: PhantomData<Rc<()>>,
}
impl std::fmt::Debug for WorkerParentGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerParentGuard")
            .field("parent", &self.parent)
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}
impl WorkerParentGuard {
    /// Real main, after parent/Job claim and before input/threads. A failed
    /// attempt requires exit; rejected numeric handles remain borrowed/unclosed.
    /// Structural admission is not cross-user authentication or an OS main proof.
    pub fn claim_config_reader(
        &mut self,
        value: usize,
    ) -> Result<super::config_artifact::ConfigReader, WorkerStartupError> {
        if !super::outside_callback() {
            return Err(WorkerStartupError::InCallback);
        }
        if self.config_attempted {
            return Err(WorkerStartupError::ConfigClaimed);
        }
        self.config_attempted = true;
        if value == 0 || value == usize::MAX || self.std_handles.contains(&value) {
            return Err(WorkerStartupError::ConfigHandle);
        }
        let handle = value as Handle;
        // Borrowed handle only. Native queries retain stable buffers on nonzero
        // status as in pipe admission; never read a pending output or close it.
        if scalar(handle, 8)? != 0x120089 || scalar(handle, 16)? & 0x30 != 0x20 {
            return Err(WorkerStartupError::ConfigHandle);
        }
        super::config_artifact::ConfigReader::inspect_inherited(handle)?;
        // SAFETY: admitted distinct inherited sync disk file; clear inheritance
        // before taking unique ownership, not a borrowed std or pseudo handle.
        if unsafe { SetHandleInformation(handle, 1, 0) } == 0 {
            return Err(error("SetHandleInformation(config)"));
        }
        // SAFETY: no Rust owner exists; exact READ, synchronous mode, disk/local
        // metadata and inheritance verified; real parent guard already admitted.
        Ok(unsafe { super::config_artifact::ConfigReader::adopt_inherited(handle) })
    }
    pub fn parent_pid(&self) -> u32 {
        self.parent
    }
    pub fn worker_pid(&self) -> u32 {
        self.pid
    }
}
pub struct WorkerStartup {
    guard: WorkerParentGuard,
    run_id: u64,
    reader: PipeReader,
    writer: PipeWriter,
}
impl std::fmt::Debug for WorkerStartup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerStartup")
            .field("guard", &self.guard)
            .field("run_id", &self.run_id)
            .finish_non_exhaustive()
    }
}
impl WorkerStartup {
    /// Call on real main before input, other threads or any std-stream IO.
    /// Any failed attempt requires process exit; do not retry or log to stderr.
    pub fn claim(parent: u32, run_id: u64) -> Result<Self, WorkerStartupError> {
        if !super::outside_callback() {
            return Err(WorkerStartupError::InCallback);
        }
        if !claim_once() {
            return Err(WorkerStartupError::AlreadyClaimed);
        }
        if parent == 0 || run_id == 0 {
            return Err(WorkerStartupError::Identity);
        }
        // SAFETY: current-process scalar only.
        let pid = unsafe { GetCurrentProcessId() };
        if parent == pid {
            return Err(WorkerStartupError::Identity);
        }
        verify_parent(pid, parent)?;
        verify_job(pid)?;
        // SAFETY: fully initialized C scalar/pointer output; no string read.
        let mut startup: Startup = unsafe { std::mem::zeroed() };
        // SAFETY: live complete STARTUPINFO output and documented std-handle
        // identifiers; all returned handles are borrowed, never owned/closed.
        let handles = unsafe {
            GetStartupInfoW(&mut startup);
            [
                GetStdHandle(0xfffffff6),
                GetStdHandle(0xfffffff5),
                GetStdHandle(0xfffffff4),
            ]
        };
        if ![
            size_of::<Startup>(),
            size_of::<Startup>() + size_of::<usize>(),
        ]
        .contains(&(startup.size as usize))
            || startup.numbers[7] & 0x100 == 0
            || handles != [startup.stdin, startup.stdout, startup.stderr]
            || handles
                .iter()
                .any(|&h| h.is_null() || h as usize == usize::MAX)
            || handles[0] == handles[1]
            || handles[0] == handles[2]
            || handles[1] == handles[2]
        {
            return Err(WorkerStartupError::StandardHandles);
        }
        for (handle, expected) in handles.into_iter().zip([3, 2, 3]) {
            // SAFETY: borrowed standard handles, read-only type query.
            if unsafe { GetFileType(handle) } != expected {
                return Err(WorkerStartupError::StandardHandles);
            }
        }
        let reader_id = inspect_pipe(handles[0], true, parent)?;
        let writer_id = inspect_pipe(handles[2], false, parent)?;
        if reader_id == writer_id {
            return Err(WorkerStartupError::PipeIdentity);
        }
        for handle in handles {
            // SAFETY: clear only inheritance on original borrowed std handles;
            // never close them, install handlers or read/write protocol here.
            if unsafe { SetHandleInformation(handle, 1, 0) } == 0 {
                return Err(error("SetHandleInformation(std)"));
            }
        }
        let reader = PipeReader::adopt_progress(duplicate(handles[0])?, reader_id)?;
        let writer = PipeWriter::adopt_progress(duplicate(handles[2])?, writer_id)?;
        Ok(Self {
            guard: WorkerParentGuard {
                parent,
                pid,
                std_handles: handles.map(|h| h as usize),
                config_attempted: false,
                _thread: PhantomData,
            },
            run_id,
            reader,
            writer,
        })
    }
    pub fn claim_config_reader(
        &mut self,
        value: usize,
    ) -> Result<super::config_artifact::ConfigReader, WorkerStartupError> {
        self.guard.claim_config_reader(value)
    }
    pub fn guard(&self) -> &WorkerParentGuard {
        &self.guard
    }
    pub fn run_id(&self) -> u64 {
        self.run_id
    }
    pub fn into_parts(self) -> (WorkerParentGuard, u64, PipeReader, PipeWriter) {
        (self.guard, self.run_id, self.reader, self.writer)
    }
}
#[cfg(test)]
mod tests;
