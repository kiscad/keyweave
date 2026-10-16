//! Unauthenticated message-pipe I/O ownership, not a public control service.
use super::control_identity::ControlIdentity;
use crate::{
    control_identity::ControlIdentityError,
    session_control::{MESSAGE_CAPACITY, ReadResult},
};
use std::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
    io,
    marker::PhantomData,
    mem::size_of,
    ptr::null_mut,
    rc::Rc,
    task::Poll,
};
type Handle = *mut c_void;
#[repr(C)]
#[derive(Default)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: Handle,
    inherit: i32,
}
struct Operation {
    overlap: UnsafeCell<Overlapped>,
    buffer: UnsafeCell<[u8; MESSAGE_CAPACITY]>,
    count: UnsafeCell<u32>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Connect,
    Read,
    Write,
}
#[derive(Clone, Copy)]
struct Flight {
    kind: Kind,
    count: usize,
}
#[cfg(not(test))]
mod api;
#[cfg(test)]
mod api_mock;
#[cfg(test)]
use api_mock as api;
thread_local! {static POISONED:Cell<bool>=const{Cell::new(false)};}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlPipeCleanup {
    pub cancel_error: Option<u32>,
    pub wait_error: Option<u32>,
    pub pipe_close_error: Option<u32>,
    pub event_close_error: Option<u32>,
    pub in_callback: bool,
    pub completion_pending: bool,
    pub resources_released: bool,
}
impl ControlPipeCleanup {
    pub fn complete(&self) -> bool {
        self.resources_released
            && !self.completion_pending
            && !self.in_callback
            && self.cancel_error.is_none()
            && self.wait_error.is_none()
            && self.pipe_close_error.is_none()
            && self.event_close_error.is_none()
    }
}
#[derive(Debug, thiserror::Error)]
#[error(
    "control pipe creation failed: {operation}; cleanup={cleanup:?}; security_cleanup={security_cleanup:?}"
)]
pub struct ControlPipeCreateError {
    #[source]
    pub operation: io::Error,
    pub cleanup: ControlPipeCleanup,
    pub security_cleanup: Option<ControlIdentityError>,
}
impl ControlPipeCreateError {
    fn new(operation: io::Error) -> Self {
        Self {
            operation,
            cleanup: ControlPipeCleanup {
                resources_released: true,
                ..ControlPipeCleanup::default()
            },
            security_cleanup: None,
        }
    }
}
fn context() -> io::Result<()> {
    if !super::outside_callback() {
        Err(io::Error::other("control pipe operation in input callback"))
    } else {
        Ok(())
    }
}
fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}
fn last() -> u32 {
    // SAFETY: same-thread scalar immediately after failed API.
    unsafe { api::GetLastError() }
}
pub(super) fn name(value: &str) -> io::Result<Vec<u16>> {
    let suffix = value
        .strip_prefix(r"\\.\pipe\keyweave-control-")
        .ok_or_else(|| io::Error::other("invalid local control pipe name"))?;
    if suffix.len() != 32
        || !suffix
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(io::Error::other("invalid local control pipe name"));
    }
    Ok(value.encode_utf16().chain(Some(0)).collect())
}
/// Low-level unauthenticated message I/O. No raw handle/adoption, no Clone/Send/
/// Sync. Does not implement the trusted public-control transport contracts.
pub struct ControlMessagePipe {
    handle: Handle,
    event: Handle,
    operation: Option<Box<Operation>>,
    flight: Option<Flight>,
    server: bool,
    connected: bool,
    message_ready: bool,
    eof: bool,
    failed: bool,
    closing: bool,
    cancel_requested: bool,
    shadow: [u8; 64],
    cleanup: ControlPipeCleanup,
    _thread: PhantomData<Rc<()>>,
}
impl std::fmt::Debug for ControlMessagePipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlMessagePipe").finish_non_exhaustive()
    }
}
impl ControlMessagePipe {
    /// Create one explicit local message instance. DACL is necessary but does
    /// not attest the peer's logon/session or last message security context.
    pub fn listen(value: &str, identity: &ControlIdentity) -> Result<Self, ControlPipeCreateError> {
        context().map_err(ControlPipeCreateError::new)?;
        if POISONED.get() {
            return Err(ControlPipeCreateError::new(io::Error::other(
                "control pipe cleanup unresolved",
            )));
        }
        let name = name(value).map_err(ControlPipeCreateError::new)?;
        let mut security = identity
            .security_descriptor()
            .map_err(|e| ControlPipeCreateError::new(io::Error::other(e)))?;
        let result = security
            .with_descriptor(|descriptor| {
                let attributes = SecurityAttributes {
                    length: size_of::<SecurityAttributes>() as u32,
                    descriptor,
                    inherit: 0,
                };
                // SAFETY: bounded terminated local name, live synchronous DACL,
                // noninherit duplex/first-instance/overlapped, message/read-message/
                // wait/reject-remote, exactly one instance, fixed small buffers.
                let handle = unsafe {
                    api::CreateNamedPipeW(
                        name.as_ptr(),
                        0x4008_0003,
                        14,
                        1,
                        128,
                        128,
                        0,
                        &attributes,
                    )
                };
                if handle as usize == usize::MAX || handle.is_null() {
                    Err(os_error(last()))
                } else {
                    Ok(handle)
                }
            })
            .map_err(io::Error::other)
            .and_then(|r| r);
        let mut owner = match result {
            Ok(handle) => Self::initialize(handle, true),
            Err(operation) => Err(ControlPipeCreateError::new(operation)),
        };
        if let Some(error) = security.close().err() {
            match &mut owner {
                Ok(pipe) => {
                    let cleanup = pipe.close();
                    return Err(ControlPipeCreateError {
                        operation: io::Error::other("control pipe security cleanup failed"),
                        cleanup,
                        security_cleanup: Some(error),
                    });
                }
                Err(failure) => failure.security_cleanup = Some(error),
            }
        }
        owner
    }
    /// Open without waiting/retrying/launching a host. PIPE_BUSY/NOT_FOUND are
    /// returned to the bounded outer connect state. This does not verify server.
    pub fn open(value: &str) -> Result<Self, ControlPipeCreateError> {
        context().map_err(ControlPipeCreateError::new)?;
        if POISONED.get() {
            return Err(ControlPipeCreateError::new(io::Error::other(
                "control pipe cleanup unresolved",
            )));
        }
        let name = name(value).map_err(ControlPipeCreateError::new)?;
        // SAFETY: bounded local name, no inherited handle, OPEN_EXISTING,
        // overlapped + SQOS_PRESENT/IDENTIFICATION, duplex payload access.
        let handle = unsafe {
            api::CreateFileW(
                name.as_ptr(),
                0xc000_0000,
                0,
                null_mut(),
                3,
                0x4011_0000,
                null_mut(),
            )
        };
        if handle as usize == usize::MAX || handle.is_null() {
            return Err(ControlPipeCreateError::new(os_error(last())));
        }
        Self::initialize(handle, false)
    }
    fn initialize(handle: Handle, server: bool) -> Result<Self, ControlPipeCreateError> {
        let mut pipe = Self {
            handle,
            event: null_mut(),
            operation: Some(Box::new(Operation {
                overlap: UnsafeCell::new(Overlapped::default()),
                buffer: UnsafeCell::new([0; MESSAGE_CAPACITY]),
                count: UnsafeCell::new(0),
            })),
            flight: None,
            server,
            connected: !server,
            message_ready: false,
            eof: false,
            failed: false,
            closing: false,
            cancel_requested: false,
            shadow: [0; 64],
            cleanup: ControlPipeCleanup::default(),
            _thread: PhantomData,
        };
        let result = (|| {
            // SAFETY: unnamed, noninheritable manual-reset event, initially clear.
            pipe.event = unsafe { api::CreateEventW(null_mut(), 1, 0, null_mut()) };
            if pipe.event.is_null() {
                return Err(os_error(last()));
            }
            if !server {
                let mode = 2;
                // SAFETY: live new client, set message-read mode only. Null
                // collection/timeout outputs do not enable synchronous waiting.
                if unsafe { api::SetNamedPipeHandleState(handle, &mode, null_mut(), null_mut()) }
                    == 0
                {
                    return Err(os_error(last()));
                }
            }
            pipe.verify()
        })();
        match result {
            Ok(()) => Ok(pipe),
            Err(operation) => {
                let cleanup = pipe.close();
                Err(ControlPipeCreateError {
                    operation,
                    cleanup,
                    security_cleanup: None,
                })
            }
        }
    }
    fn verify(&self) -> io::Result<()> {
        let mut flags = 0;
        let mut instances = 0;
        let mut mode = 0;
        let mut inheritance = 0;
        // SAFETY: fresh owned handle, fixed initialized scalar outputs. No
        // external handle adoption or inference about its async creation mode.
        if unsafe {
            api::GetNamedPipeInfo(
                self.handle,
                &mut flags,
                null_mut(),
                null_mut(),
                &mut instances,
            )
        } == 0
        {
            return Err(os_error(last()));
        }
        if flags != (4 | 8 | u32::from(self.server)) || instances != 1 {
            return Err(io::Error::other("unexpected control message pipe shape"));
        }
        if unsafe {
            api::GetNamedPipeHandleStateW(
                self.handle,
                &mut mode,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                0,
            )
        } == 0
        {
            return Err(os_error(last()));
        }
        if mode != 2 {
            return Err(io::Error::other("unexpected control pipe read mode"));
        }
        if unsafe { api::GetHandleInformation(self.handle, &mut inheritance) } == 0 {
            return Err(os_error(last()));
        }
        if inheritance != 0 {
            return Err(io::Error::other(
                "inheritable or protected control pipe handle",
            ));
        }
        Ok(())
    }
    fn pointers(&self) -> io::Result<(*mut Overlapped, *mut u8, *mut u32)> {
        let op = self
            .operation
            .as_ref()
            .ok_or_else(|| io::Error::other("closed control pipe operation"))?;
        Ok((op.overlap.get(), op.buffer.get().cast(), op.count.get()))
    }
    fn prepare(&mut self) -> io::Result<(*mut Overlapped, *mut u8, *mut u32)> {
        context()?;
        if self.closing || self.failed || self.handle.is_null() || self.flight.is_some() {
            return Err(io::Error::other("control pipe operation unavailable"));
        }
        let (overlap, buffer, count) = self.pointers()?;
        // SAFETY: no outstanding operation; uniquely owned event/storage.
        if unsafe { api::ResetEvent(self.event) } == 0 {
            self.failed = true;
            return Err(os_error(last()));
        }
        unsafe {
            *overlap = Overlapped {
                event: self.event,
                ..Overlapped::default()
            };
            *count = 0;
        }
        Ok((overlap, buffer, count))
    }
    fn completion(&mut self, kind: Kind, count: usize) -> Poll<io::Result<(usize, bool, bool)>> {
        let Some(flight) = self.flight else {
            return Poll::Ready(Err(io::Error::other("missing control pipe flight")));
        };
        if flight.kind != kind || flight.count != count {
            return Poll::Ready(Err(io::Error::other("changed pending control operation")));
        }
        let (overlap, _, _) = match self.pointers() {
            Ok(p) => p,
            Err(e) => return Poll::Ready(Err(e)),
        };
        let mut bytes = 0;
        // SAFETY: live stable pending flight; FALSE never waits.
        let success = unsafe { api::GetOverlappedResult(self.handle, overlap, &mut bytes, 0) };
        let code = if success == 0 { last() } else { 0 };
        if code == 996 {
            return Poll::Pending;
        }
        if success == 0 {
            // SAFETY: event is private/manual-reset and cleared before this
            // sole flight. Only a signal proves even an error is completed.
            if unsafe { api::WaitForSingleObject(self.event, 0) } != 0 {
                self.failed = true;
                return Poll::Ready(Err(os_error(code)));
            }
        }
        self.flight = None;
        self.result(kind, count, bytes, code)
    }
    fn result(
        &mut self,
        kind: Kind,
        count: usize,
        bytes: u32,
        code: u32,
    ) -> Poll<io::Result<(usize, bool, bool)>> {
        if kind == Kind::Read && matches!(code, 109 | 232 | 233) {
            self.eof = true;
            return Poll::Ready(Ok((0, false, true)));
        }
        if code != 0 && !(kind == Kind::Read && code == 234) {
            self.failed = true;
            return Poll::Ready(Err(os_error(code)));
        }
        if kind == Kind::Connect {
            self.connected = true;
            return Poll::Ready(Ok((0, false, false)));
        }
        if bytes as usize > count || kind == Kind::Write && bytes as usize != count {
            self.failed = true;
            return Poll::Ready(Err(io::Error::other(
                "invalid control pipe completion count",
            )));
        }
        Poll::Ready(Ok((bytes as usize, code == 234, false)))
    }
    pub fn poll_accept(&mut self) -> Poll<io::Result<()>> {
        if let Err(e) = context() {
            return Poll::Ready(Err(e));
        }
        if !self.server || self.connected || self.failed || self.closing {
            return Poll::Ready(Err(io::Error::other("control pipe is not accepting")));
        }
        if self.flight.is_some() {
            return self.completion(Kind::Connect, 0).map(|r| r.map(|_| ()));
        }
        let (overlap, _, _) = match self.prepare() {
            Ok(p) => p,
            Err(e) => return Poll::Ready(Err(e)),
        };
        // SAFETY: fresh server and stable unique OVERLAPPED/event.
        let success = unsafe { api::ConnectNamedPipe(self.handle, overlap) };
        let code = if success == 0 { last() } else { 0 };
        match code {
            0 | 535 => {
                self.connected = true;
                Poll::Ready(Ok(()))
            }
            997 => {
                self.flight = Some(Flight {
                    kind: Kind::Connect,
                    count: 0,
                });
                Poll::Pending
            }
            _ => {
                self.failed = true;
                Poll::Ready(Err(os_error(code)))
            }
        }
    }
    pub fn poll_read(&mut self, out: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>> {
        if let Err(e) = context() {
            return Poll::Ready(Err(e));
        }
        if self.closing || self.failed || !self.connected {
            return Poll::Ready(Err(io::Error::other("control pipe is not readable")));
        }
        if self.eof {
            return Poll::Ready(Ok(ReadResult::Disconnected));
        }
        self.message_ready = false;
        let result = if self.flight.is_some() {
            self.completion(Kind::Read, MESSAGE_CAPACITY)
        } else {
            let (overlap, buffer, count) = match self.prepare() {
                Ok(p) => p,
                Err(e) => return Poll::Ready(Err(e)),
            };
            // SAFETY: fixed stable writable buffer, OVERLAPPED, and DWORD storage
            // all remain live through completion. Immediate DWORD is read only
            // when not IO_PENDING, matching SDK overlapped server example.
            let success = unsafe {
                api::ReadFile(
                    self.handle,
                    buffer.cast(),
                    MESSAGE_CAPACITY as u32,
                    count,
                    overlap,
                )
            };
            let code = if success == 0 { last() } else { 0 };
            if code == 997 {
                self.flight = Some(Flight {
                    kind: Kind::Read,
                    count: MESSAGE_CAPACITY,
                });
                return Poll::Pending;
            }
            self.result(Kind::Read, MESSAGE_CAPACITY, unsafe { *count }, code)
        };
        result.map(|r| {
            r.map(|(length, more, eof)| {
                self.message_ready = self.server && !eof && !more && length == 32;
                if eof {
                    ReadResult::Disconnected
                } else {
                    // SAFETY: completed read, checked count <= buffer capacity, no
                    // flight; copy only now into this invocation's borrowed scratch.
                    let (_, buffer, _) = self.pointers().unwrap_or_else(|_| unreachable!());
                    unsafe {
                        std::ptr::copy_nonoverlapping(buffer, out.as_mut_ptr(), length);
                    }
                    ReadResult::Message { length, more }
                }
            })
        })
    }
    /// Authenticate only the owning server's complete last-read request once.
    /// This consumes the gate even on denial/error, never accepts a supplied PID.
    pub(super) fn authorize_message(
        &mut self,
        identity: &ControlIdentity,
    ) -> Result<bool, crate::control_identity::ControlIdentityFailure> {
        use crate::control_identity::{
            ControlIdentityError as Error, ControlIdentityFailure as Failure,
        };
        if !super::outside_callback() {
            return Err(Failure::operation(Error::InCallback));
        }
        let ready = std::mem::replace(&mut self.message_ready, false);
        if !ready
            || !self.server
            || !self.connected
            || self.closing
            || self.failed
            || self.flight.is_some()
        {
            return Err(Failure::operation(Error::Malformed));
        }
        identity.authorize_message(self.handle)
    }
    pub fn poll_write(&mut self, input: &[u8]) -> Poll<io::Result<()>> {
        if let Err(e) = context() {
            return Poll::Ready(Err(e));
        }
        let expected = if self.server { 64 } else { 32 };
        if input.len() != expected || self.failed || self.closing || !self.connected || self.eof {
            return Poll::Ready(Err(io::Error::other("invalid control message write")));
        }
        if self.flight.is_some() {
            if input != &self.shadow[..expected] {
                return Poll::Ready(Err(io::Error::other("changed pending control write")));
            }
            return self
                .completion(Kind::Write, expected)
                .map(|r| r.map(|_| ()));
        }
        let (overlap, buffer, count) = match self.prepare() {
            Ok(p) => p,
            Err(e) => return Poll::Ready(Err(e)),
        };
        self.shadow[..expected].copy_from_slice(input);
        // SAFETY: stable owned storage, no flight. Never modify until completion.
        unsafe {
            std::ptr::copy_nonoverlapping(input.as_ptr(), buffer, expected);
        }
        let success =
            unsafe { api::WriteFile(self.handle, buffer.cast(), expected as u32, count, overlap) };
        let code = if success == 0 { last() } else { 0 };
        if code == 997 {
            self.flight = Some(Flight {
                kind: Kind::Write,
                count: expected,
            });
            return Poll::Pending;
        }
        self.result(Kind::Write, expected, unsafe { *count }, code)
            .map(|r| r.map(|_| ()))
    }
    fn drain(&mut self) -> Poll<io::Result<()>> {
        if self.flight.is_none() {
            return Poll::Ready(Ok(()));
        }
        let (overlap, _, _) = match self.pointers() {
            Ok(p) => p,
            Err(e) => return Poll::Ready(Err(e)),
        };
        if !self.cancel_requested {
            self.cancel_requested = true;
            // SAFETY: cancel only this unique live stable flight, never NULL.
            if unsafe { api::CancelIoEx(self.handle, overlap) } == 0 {
                let code = last();
                if code != 1168 {
                    self.cleanup.cancel_error = Some(code);
                    return Poll::Ready(Err(os_error(code)));
                }
            }
            return Poll::Pending;
        }
        // SAFETY: private manual-reset event for sole flight, zero timeout.
        match unsafe { api::WaitForSingleObject(self.event, 0) } {
            0 => {
                self.flight = None;
                Poll::Ready(Ok(()))
            }
            258 => Poll::Pending,
            value => {
                let code = if value == u32::MAX { last() } else { value };
                self.cleanup.wait_error = Some(code);
                Poll::Ready(Err(os_error(code)))
            }
        }
    }
    pub fn poll_disconnect(&mut self) -> Poll<io::Result<()>> {
        self.message_ready = false;
        if let Err(e) = context() {
            return Poll::Ready(Err(e));
        }
        if !self.server || self.closing || self.handle.is_null() {
            return Poll::Ready(Err(io::Error::other("control pipe is not disconnectable")));
        }
        match self.drain() {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => (),
        }
        // SAFETY: no outstanding IO; disconnect only this owned server instance.
        if unsafe { api::DisconnectNamedPipe(self.handle) } == 0 {
            let code = last();
            if code != 233 {
                self.failed = true;
                return Poll::Ready(Err(os_error(code)));
            }
        }
        if self.cleanup.cancel_error.is_some() || self.cleanup.wait_error.is_some() {
            self.failed = true;
            return Poll::Ready(Err(io::Error::other(
                "control disconnect had cleanup failure",
            )));
        }
        self.connected = false;
        self.eof = false;
        self.failed = false;
        self.cancel_requested = false;
        Poll::Ready(Ok(()))
    }
    /// Nonblocking cleanup snapshot. Retry only within the caller's absolute
    /// close deadline. Historical independent failures remain in the report.
    pub fn close(&mut self) -> ControlPipeCleanup {
        self.closing = true;
        // Dropping a previously closed owner in a callback has no resources or
        // native work left; do not poison an otherwise clean thread.
        if self.flight.is_none() && self.handle.is_null() && self.event.is_null() {
            return self.snapshot();
        }
        if !super::outside_callback() {
            self.cleanup.in_callback = true;
            return self.snapshot();
        }
        match self.drain() {
            Poll::Pending | Poll::Ready(Err(_)) => return self.snapshot(),
            Poll::Ready(Ok(())) => (),
        }
        for (handle, slot) in [
            (&mut self.handle, &mut self.cleanup.pipe_close_error),
            (&mut self.event, &mut self.cleanup.event_close_error),
        ] {
            if !handle.is_null() {
                // SAFETY: unique resource, no OS operation can retain storage.
                if unsafe { api::CloseHandle(*handle) } == 0 {
                    *slot = Some(last());
                } else {
                    *handle = null_mut();
                }
            }
        }
        self.snapshot()
    }
    fn snapshot(&self) -> ControlPipeCleanup {
        ControlPipeCleanup {
            completion_pending: self.flight.is_some(),
            resources_released: self.flight.is_none()
                && self.handle.is_null()
                && self.event.is_null(),
            ..self.cleanup
        }
    }
}
impl Drop for ControlMessagePipe {
    fn drop(&mut self) {
        let report = self.close();
        if report.completion_pending
            && let Some(operation) = self.operation.take()
        {
            std::mem::forget(operation);
        }
        if !report.complete() {
            POISONED.set(true);
        }
    }
}
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) fn reset_server_test_pipe(payload: Vec<u8>) {
    POISONED.set(false);
    api_mock::MOCK.with(|m| {
        let mut state = api_mock::Mock::fresh();
        state.payload = payload;
        *m.borrow_mut() = state;
    });
}
#[cfg(test)]
pub(super) fn server_test_peer_closed() {
    api_mock::MOCK.with(|m| m.borrow_mut().read = api_mock::Mode::Error(109));
}
#[cfg(test)]
pub(super) fn server_test_writes() -> Vec<Vec<u8>> {
    api_mock::MOCK.with(|m| m.borrow().writes.clone())
}

impl ControlMessagePipe {
    /// Kernel-reported creator PID for this actual client connection. It must
    /// be compared with an independently retained expected process instance.
    pub(super) fn server_process_id(&self) -> io::Result<u32> {
        context()?;
        if self.server || !self.connected || self.closing || self.failed || self.handle.is_null() {
            return Err(io::Error::other("invalid control client peer query"));
        }
        let mut pid = 0;
        // SAFETY: owned live local client handle, initialized ULONG output.
        if unsafe { api::GetNamedPipeServerProcessId(self.handle, &mut pid) } == 0 {
            return Err(os_error(last()));
        }
        if pid == 0 {
            return Err(io::Error::other("missing control pipe server PID"));
        }
        Ok(pid)
    }
}

#[cfg(test)]
pub(super) fn change_client_test_pipe(change: impl FnOnce(&mut api_mock::Mock)) {
    api_mock::MOCK.with(|m| change(&mut m.borrow_mut()));
}

#[cfg(test)]
pub(super) fn server_test_open_count() -> usize {
    api_mock::MOCK.with(|m| {
        m.borrow()
            .calls
            .iter()
            .filter(|call| **call == "open-client")
            .count()
    })
}

#[cfg(test)]
pub(super) fn client_test_pending_read() {
    api_mock::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.read = api_mock::Mode::Pending;
        m.defer_cancel = true;
    });
}
#[cfg(test)]
pub(super) fn client_test_finish_pending() {
    api_mock::finish_pending();
}
