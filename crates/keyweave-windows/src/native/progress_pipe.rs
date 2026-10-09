//! Private, owned byte pipes with one bounded overlapped flight per endpoint.
use std::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
    io::{self, Read, Write},
    marker::PhantomData,
    mem::size_of,
    ptr::null_mut,
    rc::Rc,
};
type Handle = *mut c_void;
const CAPACITY: usize = 64;
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
    descriptor: *mut c_void,
    inherit: i32,
}
#[cfg(not(test))]
mod api;
#[cfg(test)]
mod api_mock;
use api::*;
#[cfg(test)]
use api_mock as api;
thread_local! { static POISONED: Cell<bool> = const {Cell::new(false)}; }
fn context() -> io::Result<()> {
    if super::outside_callback() {
        Ok(())
    } else {
        Err(io::Error::other("pipe I/O in keyboard callback"))
    }
}
fn last_error() -> u32 {
    // SAFETY: immediate same-thread error scalar, no pointers/handles.
    unsafe { GetLastError() }
}
fn os_error(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}
fn blocked() -> io::Error {
    io::ErrorKind::WouldBlock.into()
}
struct Security(*mut c_void);
impl Security {
    fn new() -> io::Result<Self> {
        let sddl: Vec<u16> = "D:P(A;;GA;;;OW)(A;;GA;;;SY)\0".encode_utf16().collect();
        let mut descriptor = null_mut();
        // SAFETY: bounded terminated SDDL and live out pointer; revision=1.
        // On success owned LocalAlloc buffer is freed with LocalFree.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(os_error(last_error()));
        }
        if descriptor.is_null() {
            return Err(io::Error::other("null pipe security descriptor"));
        }
        Ok(Self(descriptor))
    }
    fn release(&mut self) -> io::Result<()> {
        if self.0.is_null() {
            return Ok(());
        }
        // SAFETY: unique allocation from matching conversion API; no pipe uses
        // this input descriptor after synchronous CreateNamedPipeW returns.
        if unsafe { LocalFree(self.0) }.is_null() {
            self.0 = null_mut();
            Ok(())
        } else {
            Err(os_error(last_error()))
        }
    }
}
impl Drop for Security {
    fn drop(&mut self) {
        if self.release().is_err() {
            POISONED.set(true);
        }
    }
}
struct Operation {
    overlap: UnsafeCell<Overlapped>,
    buffer: UnsafeCell<[u8; CAPACITY]>,
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipeCloseReport {
    pub cancel_error: Option<u32>,
    pub wait_error: Option<u32>,
    pub completion_pending: bool,
    pub pipe_close_error: Option<u32>,
    pub event_close_error: Option<u32>,
}
impl PipeCloseReport {
    pub fn complete(&self) -> bool {
        !self.completion_pending
            && self.cancel_error.is_none()
            && self.wait_error.is_none()
            && self.pipe_close_error.is_none()
            && self.event_close_error.is_none()
    }
}
struct Endpoint {
    handle: Handle,
    event: Handle,
    operation: Option<Box<Operation>>,
    flight: Option<Flight>,
    ready: (usize, usize),
    shadow: [u8; CAPACITY],
    dead: bool,
    eof: bool,
    _thread: PhantomData<Rc<()>>,
}
impl Endpoint {
    fn verify_shape(&self, server: bool) -> io::Result<()> {
        let mut flags = 0;
        let mut instances = 0;
        // SAFETY: live owned local pipe; initialized DWORD outputs borrowed only
        // for this call. Buffer-size outputs are intentionally not requested.
        if unsafe {
            GetNamedPipeInfo(
                self.handle,
                &mut flags,
                null_mut(),
                null_mut(),
                &mut instances,
            )
        } == 0
        {
            return Err(os_error(last_error()));
        }
        // Windows also reports PIPE_REJECT_REMOTE_CLIENTS in these flags; keep
        // that creation restriction rather than treating it as an unknown bit.
        if flags != (8 | u32::from(server)) || instances != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected progress pipe type, endpoint or instance limit",
            ));
        }
        let mut state = 0;
        // SAFETY: owned local endpoint; only scalar state is requested. All
        // collection/timeout/account outputs are NULL; no user string is read.
        if unsafe {
            GetNamedPipeHandleStateW(
                self.handle,
                &mut state,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                0,
            )
        } == 0
        {
            return Err(os_error(last_error()));
        }
        // PIPE_WAIT | PIPE_READMODE_BYTE. Nonblocking operation is provided by
        // our FILE_FLAG_OVERLAPPED creation contract, not by PIPE_NOWAIT. These
        // metadata queries alone cannot attest another handle's async mode.
        if state != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected progress pipe read/wait mode",
            ));
        }
        Ok(())
    }
    fn new(handle: Handle) -> io::Result<Self> {
        let mut owner = Self {
            handle,
            event: null_mut(),
            operation: None,
            flight: None,
            ready: (0, 0),
            shadow: [0; CAPACITY],
            dead: false,
            eof: false,
            _thread: PhantomData,
        };
        // SAFETY: unnamed, noninheritable manual-reset event, initially unset.
        owner.event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if owner.event.is_null() {
            return Err(os_error(last_error()));
        }
        owner.operation = Some(Box::new(Operation {
            overlap: UnsafeCell::new(Overlapped {
                event: owner.event,
                ..Overlapped::default()
            }),
            buffer: UnsafeCell::new([0; CAPACITY]),
        }));
        Ok(owner)
    }
    fn pointers(&self) -> io::Result<(*mut Overlapped, *mut u8)> {
        let Some(operation) = self.operation.as_ref() else {
            return Err(io::Error::other("closed pipe operation"));
        };
        Ok((operation.overlap.get(), operation.buffer.get().cast()))
    }
    fn prepare(&mut self) -> io::Result<(*mut Overlapped, *mut u8)> {
        if self.dead || self.handle.is_null() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if self.flight.is_some() || self.ready.0 != self.ready.1 {
            return Err(io::Error::other("pipe operation still occupied"));
        }
        let (overlap, buffer) = self.pointers()?;
        // SAFETY: no outstanding IO or unread result; uniquely owned event and
        // stable UnsafeCell storage may now be reset before the next operation.
        if unsafe { ResetEvent(self.event) } == 0 {
            self.dead = true;
            return Err(os_error(last_error()));
        }
        unsafe {
            *overlap = Overlapped {
                event: self.event,
                ..Overlapped::default()
            };
        }
        Ok((overlap, buffer))
    }
    /// One completion query. On an ambiguous API error retain the flight.
    fn poll(&mut self) -> io::Result<usize> {
        let Some(flight) = self.flight else {
            return Err(io::Error::other("missing pipe flight"));
        };
        let (overlap, _) = self.pointers()?;
        let mut bytes = 0;
        // SAFETY: stable flight/handle and live DWORD output. FALSE never waits.
        let result = unsafe { GetOverlappedResult(self.handle, overlap, &mut bytes, 0) };
        if result == 0 {
            let code = last_error();
            if code == 996 {
                return Err(blocked());
            }
            // SAFETY: own manual-reset event, reset before this flight. A signaled
            // event proves completion even when the result is an IO failure.
            if unsafe { WaitForSingleObject(self.event, 0) } == 0 {
                self.flight = None;
            }
            self.dead = true;
            return Err(os_error(code));
        }
        self.flight = None;
        // ConnectNamedPipe has no defined byte count; only read/write results
        // may be checked against the requested byte length.
        if flight.kind == Kind::Connect {
            return Ok(0);
        }
        if bytes as usize > flight.count {
            self.dead = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "excessive pipe completion",
            ));
        }
        Ok(bytes as usize)
    }
    fn connect(&mut self) -> io::Result<()> {
        let (overlap, _) = self.prepare()?;
        // SAFETY: live owned server and stable overlapped/event; client has been
        // opened before this call. Never provide NULL on an overlapped handle.
        let result = unsafe { ConnectNamedPipe(self.handle, overlap) };
        if result != 0 {
            return Ok(());
        }
        match last_error() {
            535 => Ok(()),
            997 => {
                self.flight = Some(Flight {
                    kind: Kind::Connect,
                    count: 0,
                });
                Ok(())
            }
            code => {
                self.dead = true;
                Err(os_error(code))
            }
        }
    }
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        context()?;
        if out.is_empty() {
            return Ok(0);
        }
        if self.eof {
            return Ok(0);
        }
        if self.dead {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if let Some(flight) = self.flight {
            match self.poll() {
                Ok(count) => {
                    if flight.kind == Kind::Read {
                        self.ready = (0, count)
                    }
                }
                Err(error) => {
                    if error.raw_os_error() == Some(109) && self.flight.is_none() {
                        self.eof = true;
                        return Ok(0);
                    }
                    return Err(error);
                }
            }
            if flight.kind == Kind::Connect {
                return Err(blocked());
            }
        } else if self.ready.0 == self.ready.1 {
            let (overlap, buffer) = self.prepare()?;
            let count = out.len().min(CAPACITY);
            // SAFETY: stable writable buffer/count <=64 and OVERLAPPED, no other
            // IO; NULL count pointer per async API contract. Box lives to completion.
            let result = unsafe {
                ReadFile(
                    self.handle,
                    buffer.cast(),
                    count as u32,
                    null_mut(),
                    overlap,
                )
            };
            if result == 0 {
                let code = last_error();
                if code == 109 {
                    self.dead = true;
                    self.eof = true;
                    return Ok(0);
                }
                if code != 997 {
                    self.dead = true;
                    return Err(os_error(code));
                }
            }
            self.flight = Some(Flight {
                kind: Kind::Read,
                count,
            });
            // Yield even after immediate completion; next call owns the query.
            return Err(blocked());
        }
        let count = (self.ready.1 - self.ready.0).min(out.len());
        let (_, buffer) = self.pointers()?;
        // SAFETY: read result completed and no flight exists; ready bounds were
        // checked against the requested count <=64. No OS writes remain.
        let source = unsafe { std::slice::from_raw_parts(buffer.add(self.ready.0), count) };
        out[..count].copy_from_slice(source);
        self.ready.0 += count;
        Ok(count)
    }
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        context()?;
        if input.is_empty() {
            return Ok(0);
        }
        if self.dead {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if let Some(flight) = self.flight {
            if flight.kind != Kind::Write
                || input.len() < flight.count
                || input[..flight.count] != self.shadow[..flight.count]
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "changed pending pipe write",
                ));
            }
            return self.poll();
        }
        let (overlap, buffer) = self.prepare()?;
        let count = input.len().min(CAPACITY);
        self.shadow[..count].copy_from_slice(&input[..count]);
        // SAFETY: no outstanding operation; uniquely owned stable buffer stores
        // <=64 bytes, then remains untouched until OS completion is confirmed.
        unsafe {
            std::ptr::copy_nonoverlapping(input.as_ptr(), buffer, count);
        }
        // SAFETY: stable input/OVERLAPPED and owned write handle; async NULL count.
        let result = unsafe {
            WriteFile(
                self.handle,
                buffer.cast(),
                count as u32,
                null_mut(),
                overlap,
            )
        };
        if result == 0 {
            let code = last_error();
            if code != 997 {
                self.dead = true;
                return Err(os_error(code));
            }
        }
        self.flight = Some(Flight {
            kind: Kind::Write,
            count,
        });
        Err(blocked())
    }
    fn close_inner(&mut self, millis: u32) -> PipeCloseReport {
        self.dead = true;
        let mut report = PipeCloseReport::default();
        if self.flight.is_some() {
            let Ok((overlap, _)) = self.pointers() else {
                report.completion_pending = true;
                return report;
            };
            // SAFETY: cancel only this owned flight, stable pointer remains live.
            if unsafe { CancelIoEx(self.handle, overlap) } == 0 {
                let code = last_error();
                if code != 1168 {
                    report.cancel_error = Some(code);
                }
            }
            // SAFETY: owned event; 0..20ms bound. Cancel request is not completion.
            let result = unsafe { WaitForSingleObject(self.event, millis.min(20)) };
            if result != 0 {
                if result != 258 {
                    report.wait_error = Some(if result == u32::MAX {
                        last_error()
                    } else {
                        result
                    });
                }
                report.completion_pending = true;
                return report;
            }
            self.flight = None;
        }
        if !self.handle.is_null() {
            // SAFETY: unique pipe, no outstanding IO. Retain handle on failure.
            if unsafe { CloseHandle(self.handle) } == 0 {
                report.pipe_close_error = Some(last_error());
            } else {
                self.handle = null_mut();
            }
        }
        if !self.event.is_null() {
            // SAFETY: unique event with no outstanding OS operation.
            if unsafe { CloseHandle(self.event) } == 0 {
                report.event_close_error = Some(last_error());
            } else {
                self.event = null_mut();
            }
        }
        report
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        let wait = if super::outside_callback() { 20 } else { 0 };
        let report = self.close_inner(wait);
        if report.completion_pending {
            // Do not free pending OS buffers or close its event. Bounded fallback
            // deliberately retains these resources until process exit.
            if let Some(operation) = self.operation.take() {
                std::mem::forget(operation);
            }
        }
        if !report.complete() {
            POISONED.set(true);
        }
    }
}
pub struct PipeReader(Endpoint);
pub struct PipeWriter(Endpoint);
impl PipeReader {
    pub fn pair() -> io::Result<(Self, PipeWriter)> {
        context()?;
        if POISONED.get() {
            return Err(io::Error::other("pipe resource cleanup unresolved"));
        }
        let mut random = [0u8; 16];
        // SAFETY: system RNG NULL algorithm, flag2, fixed live 16-byte output.
        let status = unsafe { BCryptGenRandom(null_mut(), random.as_mut_ptr(), 16, 2) };
        if status != 0 {
            return Err(io::Error::other(format!("pipe RNG status {status}")));
        }
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let name: Vec<u16> = format!("\\\\.\\pipe\\keyweave-progress-{suffix}\0")
            .encode_utf16()
            .collect();
        let mut security = Security::new()?;
        let attributes = SecurityAttributes {
            length: size_of::<SecurityAttributes>() as u32,
            descriptor: security.0,
            inherit: 0,
        };
        // SAFETY: terminated random local name, live protected descriptor, one
        // byte-mode inbound instance, first-instance/overlapped/reject-remote.
        let server = unsafe {
            CreateNamedPipeW(name.as_ptr(), 0x40080001, 8, 1, 4096, 4096, 0, &attributes)
        };
        if server as usize == usize::MAX {
            return Err(os_error(last_error()));
        }
        let mut reader = Endpoint::new(server)?;
        // SAFETY: open our local pipe, no inheritance; async, SECURITY_ANONYMOUS
        // SQOS prevents server impersonation. FILE_READ_ATTRIBUTES is needed
        // for write-only pipe metadata queries; it does not grant payload reads.
        // Name not exported to external peers.
        let client = unsafe {
            CreateFileW(
                name.as_ptr(),
                0x40000080,
                0,
                std::ptr::null(),
                3,
                0x40100000,
                null_mut(),
            )
        };
        if client as usize == usize::MAX {
            return Err(os_error(last_error()));
        }
        let writer = Endpoint::new(client)?;
        reader.verify_shape(true)?;
        writer.verify_shape(false)?;
        reader.connect()?;
        security.release()?;
        Ok((Self(reader), PipeWriter(writer)))
    }
    pub fn close(&mut self) -> io::Result<PipeCloseReport> {
        context()?;
        Ok(self.0.close_inner(20))
    }
}
impl PipeWriter {
    pub fn close(&mut self) -> io::Result<PipeCloseReport> {
        context()?;
        Ok(self.0.close_inner(20))
    }
}
impl Read for PipeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.0.read(out)
    }
}
impl Write for PipeWriter {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.0.write(input)
    }
    fn flush(&mut self) -> io::Result<()> {
        context()?;
        if self.0.dead {
            Err(io::ErrorKind::BrokenPipe.into())
        } else if self.0.flight.is_some() {
            Err(blocked())
        } else {
            Ok(())
        }
    }
}
#[cfg(test)]
mod tests;
