//! Windows one-message control transactions, independent of native pipe ownership.
//!
//! Transport implementations must retain pending buffers, enforce message mode,
//! and attest the actual connected peer. These machines do not authenticate a
//! PID, own a handle, perform cleanup in Drop, or renew a worker lease.
use keyweave_progress::session_control::{REPLY_BYTES, REQUEST_BYTES, Reply, Request};
use std::{io, task::Poll, time::Duration};

const TRANSACTION_LIMIT: Duration = Duration::from_secs(1);
const CLOSE_LIMIT: Duration = Duration::from_millis(100);
pub const MESSAGE_CAPACITY: usize = REPLY_BYTES + 1;

/// A whole message or a disconnected peer. `more` preserves ERROR_MORE_DATA;
/// never report a truncated/partial message as complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadResult {
    Message { length: usize, more: bool },
    Disconnected,
}
/// A poll method starts or observes at most one nonblocking native flight.
/// Pending retries must retain the same operation/storage. Native I/O must use
/// adapter-owned stable memory, never retain these borrowed scratch slices.
/// A completed read copies the result into the current scratch slice. Ready write means
/// the entire supplied message was transferred, with the exact count checked.
/// Authorization follows the complete request read and must bind the last-read
/// message's security context and actual connection, not just a process token.
/// It must restore the original thread context before returning (including Pending).
/// `disconnect` includes cancellation completion before handle/memory reuse;
/// errors/timeouts require the owning adapter's independent bounded cleanup.
pub trait ServerTransport {
    fn accept(&mut self) -> Poll<io::Result<()>>;
    fn authorize(&mut self) -> Poll<io::Result<bool>>;
    fn read(&mut self, buffer: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>>;
    fn write(&mut self, message: &[u8; REPLY_BYTES]) -> Poll<io::Result<()>>;
    fn disconnect(&mut self) -> Poll<io::Result<()>>;
}
/// Client write/read obey the same complete-message rules as ServerTransport.
/// `close` returns Ready only after cancellation/completion/owned close is known;
/// it must not block or flush waiting for the server. Connect never auto-launches
/// a host. Authorization must bind the actual connected server, not an argv PID.
pub trait ClientTransport {
    fn connect(&mut self) -> Poll<io::Result<()>>;
    fn authorize(&mut self) -> Poll<io::Result<bool>>;
    fn write(&mut self, message: &[u8; REQUEST_BYTES]) -> Poll<io::Result<()>>;
    fn read(&mut self, buffer: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>>;
    fn close(&mut self) -> Poll<io::Result<()>>;
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("control transaction polled in input callback")]
    InCallback,
    #[error("control transaction clock moved backwards")]
    ClockRegression,
    #[error("control transaction deadline overflow")]
    DeadlineOverflow,
    #[error("control transport failed: {0}")]
    Transport(#[source] io::Error),
    #[error("invalid control request")]
    InvalidRequest,
    #[error("control handler returned invalid or mismatched reply")]
    InvalidReply,
    #[error("control close deadline elapsed; native cleanup remains unresolved")]
    CloseTimeout,
    #[error("control transaction is terminal")]
    Terminal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Completed,
    Unauthorized,
    Malformed,
    PeerClosed,
    TimedOut,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct End {
    pub reason: EndReason,
    /// Handler ran; a Stop may already have taken effect even if reply was lost.
    pub dispatched: bool,
    /// Full write completion; not receipt, exit, or recovery confirmation.
    pub reply_written: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerStep {
    Pending,
    Dispatched,
    Finished(End),
}
#[derive(Clone, Copy)]
enum ServerState {
    Accept,
    Read,
    Authorize(Request),
    Dispatch(Request),
    Write([u8; REPLY_BYTES]),
    WaitClose,
    Disconnect(EndReason),
    Failed,
}
#[derive(Default)]
struct Clock {
    last: Option<Duration>,
}
impl Clock {
    fn observe(&mut self, now: Duration) -> Result<(), Error> {
        if self.last.is_some_and(|last| now < last) {
            return Err(Error::ClockRegression);
        }
        self.last = Some(now);
        Ok(())
    }
}
fn context() -> Result<(), Error> {
    #[cfg(any(windows, test))]
    if !crate::native::outside_callback() {
        return Err(Error::InCallback);
    }
    Ok(())
}
fn deadline(now: Duration, limit: Duration) -> Result<Duration, Error> {
    now.checked_add(limit).ok_or(Error::DeadlineOverflow)
}
/// One sequential client per instance. Multiple native instances require their
/// own machines and an outer bounded fairness budget, never an unbounded loop.
pub struct Server {
    state: ServerState,
    clock: Clock,
    deadline: Option<Duration>,
    dispatched: bool,
    reply_written: bool,
}
impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}
impl Server {
    pub fn new() -> Self {
        Self {
            state: ServerState::Accept,
            clock: Clock::default(),
            deadline: None,
            dispatched: false,
            reply_written: false,
        }
    }
    pub fn next_deadline(&self) -> Option<Duration> {
        if matches!(self.state, ServerState::Failed) {
            None
        } else {
            self.deadline
        }
    }
    pub fn request_dispatched(&self) -> bool {
        self.dispatched
    }
    pub fn reply_written(&self) -> bool {
        self.reply_written
    }
    fn disconnect(&mut self, now: Duration, reason: EndReason) -> Result<(), Error> {
        self.deadline = Some(deadline(now, CLOSE_LIMIT)?);
        self.state = ServerState::Disconnect(reason);
        Ok(())
    }
    /// Poll native supervision first, then call this once with fresh monotonic
    /// time. Handler must be bounded; a Pending request is never dispatched twice.
    pub fn poll(
        &mut self,
        transport: &mut impl ServerTransport,
        now: Duration,
        mut handler: impl FnMut(Request) -> Reply,
    ) -> Result<ServerStep, Error> {
        let result = self.poll_inner(transport, now, &mut handler);
        if result.is_err() {
            self.state = ServerState::Failed;
            self.deadline = None;
        }
        result
    }
    fn poll_inner(
        &mut self,
        transport: &mut impl ServerTransport,
        now: Duration,
        handler: &mut impl FnMut(Request) -> Reply,
    ) -> Result<ServerStep, Error> {
        context()?;
        if matches!(self.state, ServerState::Failed) {
            return Err(Error::Terminal);
        }
        self.clock.observe(now)?;
        if self.deadline.is_some_and(|limit| now >= limit) {
            if matches!(self.state, ServerState::Disconnect(_)) {
                return Err(Error::CloseTimeout);
            }
            self.disconnect(now, EndReason::TimedOut)?;
            return Ok(ServerStep::Pending);
        }
        match self.state {
            ServerState::Accept => match transport.accept() {
                Poll::Pending => (),
                Poll::Ready(result) => {
                    result.map_err(Error::Transport)?;
                    self.deadline = Some(deadline(now, TRANSACTION_LIMIT)?);
                    self.state = ServerState::Read;
                }
            },
            ServerState::Authorize(request) => match transport.authorize() {
                Poll::Pending => (),
                Poll::Ready(result) => {
                    if result.map_err(Error::Transport)? {
                        self.state = ServerState::Dispatch(request);
                    } else {
                        self.disconnect(now, EndReason::Unauthorized)?;
                    }
                }
            },
            ServerState::Read => {
                let mut buffer = [0; MESSAGE_CAPACITY];
                if let Poll::Ready(result) = transport.read(&mut buffer) {
                    match result.map_err(Error::Transport)? {
                        ReadResult::Message { length, more } => {
                            let request = buffer
                                .get(..length)
                                .filter(|_| !more)
                                .and_then(|bytes| Request::decode(bytes).ok());
                            if let Some(request) = request {
                                self.state = ServerState::Authorize(request);
                            } else {
                                self.disconnect(now, EndReason::Malformed)?;
                            }
                        }
                        ReadResult::Disconnected => self.disconnect(now, EndReason::PeerClosed)?,
                    }
                }
            }
            ServerState::Dispatch(request) => {
                // Set before callback: even an invalid response must not replay
                // an operation which may already have changed host state.
                self.dispatched = true;
                self.state = ServerState::Failed;
                let reply = handler(request);
                if !reply.matches(request) {
                    return Err(Error::InvalidReply);
                }
                self.state = ServerState::Write(reply.encode().map_err(|_| Error::InvalidReply)?);
                return Ok(ServerStep::Dispatched);
            }
            ServerState::Write(bytes) => {
                if let Poll::Ready(result) = transport.write(&bytes) {
                    result.map_err(Error::Transport)?;
                    self.reply_written = true;
                    self.state = ServerState::WaitClose;
                }
            }
            ServerState::WaitClose => {
                let mut buffer = [0; MESSAGE_CAPACITY];
                if let Poll::Ready(result) = transport.read(&mut buffer) {
                    let reason = match result.map_err(Error::Transport)? {
                        ReadResult::Disconnected => EndReason::Completed,
                        ReadResult::Message { .. } => EndReason::Malformed,
                    };
                    self.disconnect(now, reason)?;
                }
            }
            ServerState::Disconnect(reason) => {
                if let Poll::Ready(result) = transport.disconnect() {
                    result.map_err(Error::Transport)?;
                    let end = End {
                        reason,
                        dispatched: self.dispatched,
                        reply_written: self.reply_written,
                    };
                    self.state = ServerState::Accept;
                    self.deadline = None;
                    self.dispatched = false;
                    self.reply_written = false;
                    return Ok(ServerStep::Finished(end));
                }
            }
            ServerState::Failed => unreachable!(),
        }
        Ok(ServerStep::Pending)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientStep {
    Pending,
    /// Own transport close confirmed. Reply still does not prove worker exit.
    Finished {
        reason: EndReason,
        reply: Option<Reply>,
    },
}
#[derive(Clone, Copy)]
enum ClientState {
    Connect,
    Authorize,
    Write,
    Read,
    Close(EndReason),
    Finished,
    Failed,
}
pub struct Client {
    state: ClientState,
    request: Request,
    bytes: [u8; REQUEST_BYTES],
    reply: Option<Reply>,
    clock: Clock,
    deadline: Duration,
}
impl Client {
    /// `now` must be the caller's same monotonic clock used for subsequent poll.
    pub fn new(request: Request, now: Duration) -> Result<Self, Error> {
        context()?;
        let bytes = request.encode().map_err(|_| Error::InvalidRequest)?;
        Ok(Self {
            state: ClientState::Connect,
            request,
            bytes,
            reply: None,
            clock: Clock { last: Some(now) },
            deadline: deadline(now, TRANSACTION_LIMIT)?,
        })
    }
    pub fn next_deadline(&self) -> Option<Duration> {
        if matches!(self.state, ClientState::Finished | ClientState::Failed) {
            None
        } else {
            Some(self.deadline)
        }
    }
    fn close(&mut self, now: Duration, reason: EndReason) -> Result<(), Error> {
        self.deadline = deadline(now, CLOSE_LIMIT)?;
        self.state = ClientState::Close(reason);
        Ok(())
    }
    pub fn poll(
        &mut self,
        transport: &mut impl ClientTransport,
        now: Duration,
    ) -> Result<ClientStep, Error> {
        let result = self.poll_inner(transport, now);
        if result.is_err() {
            self.state = ClientState::Failed;
        }
        result
    }
    fn poll_inner(
        &mut self,
        transport: &mut impl ClientTransport,
        now: Duration,
    ) -> Result<ClientStep, Error> {
        context()?;
        if matches!(self.state, ClientState::Failed | ClientState::Finished) {
            return Err(Error::Terminal);
        }
        self.clock.observe(now)?;
        if now >= self.deadline {
            if matches!(self.state, ClientState::Close(_)) {
                return Err(Error::CloseTimeout);
            }
            self.close(now, EndReason::TimedOut)?;
            return Ok(ClientStep::Pending);
        }
        match self.state {
            ClientState::Connect => {
                if let Poll::Ready(result) = transport.connect() {
                    result.map_err(Error::Transport)?;
                    self.state = ClientState::Authorize;
                }
            }
            ClientState::Authorize => {
                if let Poll::Ready(result) = transport.authorize() {
                    if result.map_err(Error::Transport)? {
                        self.state = ClientState::Write;
                    } else {
                        self.close(now, EndReason::Unauthorized)?;
                    }
                }
            }
            ClientState::Write => {
                if let Poll::Ready(result) = transport.write(&self.bytes) {
                    result.map_err(Error::Transport)?;
                    self.state = ClientState::Read;
                }
            }
            ClientState::Read => {
                let mut buffer = [0; MESSAGE_CAPACITY];
                if let Poll::Ready(result) = transport.read(&mut buffer) {
                    let reason = match result.map_err(Error::Transport)? {
                        ReadResult::Disconnected => EndReason::PeerClosed,
                        ReadResult::Message { length, more } => {
                            let reply = buffer
                                .get(..length)
                                .filter(|_| !more)
                                .and_then(|bytes| Reply::decode(bytes).ok())
                                .filter(|reply| reply.matches(self.request));
                            if let Some(reply) = reply {
                                self.reply = Some(reply);
                                EndReason::Completed
                            } else {
                                EndReason::Malformed
                            }
                        }
                    };
                    self.close(now, reason)?;
                }
            }
            ClientState::Close(reason) => {
                if let Poll::Ready(result) = transport.close() {
                    result.map_err(Error::Transport)?;
                    self.state = ClientState::Finished;
                    return Ok(ClientStep::Finished {
                        reason,
                        reply: self.reply,
                    });
                }
            }
            ClientState::Finished | ClientState::Failed => unreachable!(),
        }
        Ok(ClientStep::Pending)
    }
}
#[cfg(test)]
mod tests;
