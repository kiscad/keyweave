//! Public configuration transactions. No OS transport, parser or progress source.
//! Backend adapters must exclusively bind tickets to actual private reload ACKs.
use crate::{
    number, reload,
    session_control::{Phase, Snapshot},
};
use keyweave_types::MonoTime;
pub const FRAME_BYTES: usize = 48;
pub const MAX_BYTES: u64 = reload::MAX_UPDATE_BYTES;
pub const MAX_TIMEOUT_NS: u64 = reload::MAX_TIMEOUT_NS;
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid public apply frame")]
    Frame,
    #[error("invalid configuration byte length")]
    Length,
    #[error("invalid apply deadline or clock overflow")]
    Deadline,
    #[error("apply clock moved backwards")]
    Clock,
    #[error("apply deadline expired; submission may already have taken effect")]
    Expired,
    #[error("invalid or changed host session facts")]
    Session,
    #[error("invalid backend completion binding or generation")]
    Completion,
    #[error("backend failed; submission may already have taken effect")]
    Backend,
    #[error("apply transaction is closed and cannot be retried")]
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub session: u64,
    pub base: u64,
    pub target: u64,
    pub byte_length: u64,
}
fn encode(magic: &[u8; 4], kind: u8, fields: [u64; 5]) -> [u8; FRAME_BYTES] {
    let mut bytes = [0; FRAME_BYTES];
    bytes[..4].copy_from_slice(magic);
    bytes[4] = 1;
    bytes[5] = kind;
    for (index, value) in fields.into_iter().enumerate() {
        bytes[8 + index * 8..16 + index * 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
fn frame(bytes: &[u8], magic: &[u8; 4]) -> Result<(), Error> {
    if bytes.len() != FRAME_BYTES || &bytes[..4] != magic || bytes[4] != 1 || bytes[6..8] != [0; 2]
    {
        return Err(Error::Frame);
    }
    Ok(())
}
impl Request {
    pub fn validate(self) -> Result<(), Error> {
        if self.id == 0
            || self.session == 0
            || self.base == 0
            || self.base.checked_add(1) != Some(self.target)
        {
            return Err(Error::Frame);
        }
        if !(1..=MAX_BYTES).contains(&self.byte_length) {
            return Err(Error::Length);
        }
        Ok(())
    }
    pub fn encode(self) -> Result<[u8; FRAME_BYTES], Error> {
        self.validate()?;
        Ok(encode(
            b"KWAQ",
            0,
            [
                self.id,
                self.session,
                self.base,
                self.target,
                self.byte_length,
            ],
        ))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        frame(bytes, b"KWAQ")?;
        if bytes[5] != 0 {
            return Err(Error::Frame);
        }
        let request = Self {
            id: number(bytes, 8),
            session: number(bytes, 16),
            base: number(bytes, 24),
            target: number(bytes, 32),
            byte_length: number(bytes, 40),
        };
        request.validate()?;
        Ok(request)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Committed,
    Rejected,
    Cancelled,
    Busy,
    Unavailable,
    StaleSession,
    StaleGeneration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub id: u64,
    pub session: u64,
    pub base: u64,
    pub target: u64,
    pub observed: Option<u64>,
    pub outcome: Outcome,
}
impl Reply {
    pub fn encode(self) -> Result<[u8; FRAME_BYTES], Error> {
        if self.id == 0
            || self.session == 0
            || self.base == 0
            || self.base.checked_add(1) != Some(self.target)
            || self.observed == Some(0)
        {
            return Err(Error::Frame);
        }
        let kind = match self.outcome {
            Outcome::Committed if self.observed == Some(self.target) => 1,
            Outcome::Rejected if self.observed == Some(self.base) => 2,
            Outcome::Cancelled if self.observed == Some(self.base) => 3,
            Outcome::Busy => 4,
            Outcome::Unavailable => 5,
            Outcome::StaleSession => 6,
            Outcome::StaleGeneration if self.observed != Some(self.base) => 7,
            _ => return Err(Error::Completion),
        };
        Ok(encode(
            b"KWAR",
            kind,
            [
                self.id,
                self.session,
                self.base,
                self.target,
                self.observed.unwrap_or(0),
            ],
        ))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        frame(bytes, b"KWAR")?;
        let outcome = match bytes[5] {
            1 => Outcome::Committed,
            2 => Outcome::Rejected,
            3 => Outcome::Cancelled,
            4 => Outcome::Busy,
            5 => Outcome::Unavailable,
            6 => Outcome::StaleSession,
            7 => Outcome::StaleGeneration,
            _ => return Err(Error::Frame),
        };
        let observed = number(bytes, 40);
        let reply = Self {
            id: number(bytes, 8),
            session: number(bytes, 16),
            base: number(bytes, 24),
            target: number(bytes, 32),
            observed: (observed != 0).then_some(observed),
            outcome,
        };
        reply.encode()?;
        Ok(reply)
    }
    /// Validate both wire facts and request identity before exposing completion.
    /// Receipt of this reply says nothing about subsequent edits or process exit.
    pub fn matches(self, request: Request) -> bool {
        request.validate().is_ok()
            && self.encode().is_ok()
            && self.id == request.id
            && self.base == request.base
            && self.target == request.target
            && match self.outcome {
                Outcome::StaleSession => self.session != request.session,
                _ => self.session == request.session,
            }
    }
}
/// Definitive admission refusal: this callback did not queue/activate the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Busy,
    Unavailable,
    Rejected,
}
pub enum Admission<T> {
    Queued(T),
    NotSubmitted(Refusal),
}
pub struct Completion<T> {
    pub ticket: T,
    pub outcome: reload::Outcome,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("apply backend observation failed")]
pub struct BackendError;
/// Trusted host boundary. Tickets must bind actual owned private transactions,
/// exact bytes/base/target, and completion ACKs. No other reload consumer may
/// steal these outcomes. Methods run outside input callbacks and must be bounded.
pub trait Backend {
    type Ticket: Eq;
    fn submit(
        &mut self,
        body: Box<[u8]>,
        base: u64,
        target: u64,
    ) -> Result<Admission<Self::Ticket>, BackendError>;
    fn completion(
        &mut self,
        ticket: &Self::Ticket,
    ) -> Result<Option<Completion<Self::Ticket>>, BackendError>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub error: Error,
    pub may_have_applied: bool,
}
enum State<T> {
    Ready,
    Calling,
    Awaiting(T),
    Finished(Reply),
    Failed(Failure),
}
pub struct Transaction<T> {
    request: Request,
    body: Option<Box<[u8]>>,
    deadline: MonoTime,
    last: MonoTime,
    submitted: bool,
    state: State<T>,
}
impl<T: Eq> Transaction<T> {
    /// The transport must validate the header before buffering a bounded body,
    /// authenticate its peer, and prove message termination before constructing
    /// this transaction. `started` is first request observation, not body EOF.
    pub fn new(
        request: Request,
        body: Box<[u8]>,
        started: MonoTime,
        timeout_ns: u64,
    ) -> Result<Self, Error> {
        request.validate()?;
        if body.len() as u64 != request.byte_length {
            return Err(Error::Length);
        }
        if !(1..=MAX_TIMEOUT_NS).contains(&timeout_ns) {
            return Err(Error::Deadline);
        }
        let deadline = started.checked_add(timeout_ns).ok_or(Error::Deadline)?;
        Ok(Self {
            request,
            body: Some(body),
            deadline,
            last: started,
            submitted: false,
            state: State::Ready,
        })
    }
    pub fn may_have_applied(&self) -> bool {
        self.submitted
    }
    fn fail(&mut self, error: Error) -> Failure {
        let failure = Failure {
            error,
            may_have_applied: self.submitted,
        };
        self.body = None;
        self.state = State::Failed(failure);
        failure
    }
    fn check(&mut self, now: MonoTime) -> Result<(), Failure> {
        if now < self.last {
            return Err(self.fail(Error::Clock));
        }
        if now >= self.deadline {
            return Err(self.fail(Error::Expired));
        }
        self.last = now;
        Ok(())
    }
    fn finish(
        &mut self,
        snapshot: Snapshot,
        outcome: Outcome,
        observed: Option<u64>,
    ) -> Result<Option<Reply>, Failure> {
        let reply = Reply {
            id: self.request.id,
            session: snapshot.session,
            base: self.request.base,
            target: self.request.target,
            outcome,
            observed,
        };
        if !reply.matches(self.request) {
            return Err(self.fail(Error::Completion));
        }
        self.body = None;
        if matches!(outcome, Outcome::Rejected | Outcome::Cancelled) {
            self.submitted = false;
        }
        self.state = State::Finished(reply);
        Ok(Some(reply))
    }
    /// At most one backend call per step. Pending never extends the deadline.
    /// Backend panic leaves Calling/submitted state so subsequent steps cannot
    /// invoke the callback again. Host retains original supervision regardless
    /// of transaction/transport destruction; Drop is not cancellation/rollback.
    pub fn step<B: Backend<Ticket = T>>(
        &mut self,
        snapshot: Snapshot,
        backend: &mut B,
        mut clock: impl FnMut() -> MonoTime,
    ) -> Result<Option<Reply>, Failure> {
        match self.state {
            State::Finished(reply) => return Ok(Some(reply)),
            State::Failed(failure) => return Err(failure),
            State::Calling => return Err(self.fail(Error::Closed)),
            _ => {}
        }
        self.check(clock())?;
        if snapshot.validate().is_err() {
            return Err(self.fail(Error::Session));
        }
        if snapshot.session != self.request.session {
            if self.submitted {
                return Err(self.fail(Error::Session));
            }
            return self.finish(snapshot, Outcome::StaleSession, snapshot.generation);
        }
        if matches!(self.state, State::Ready) {
            if snapshot.generation != Some(self.request.base) {
                return self.finish(snapshot, Outcome::StaleGeneration, snapshot.generation);
            }
            if snapshot.phase != Phase::Active || snapshot.fault {
                return self.finish(snapshot, Outcome::Unavailable, snapshot.generation);
            }
            let Some(body) = self.body.take() else {
                return Err(self.fail(Error::Closed));
            };
            self.state = State::Calling;
            self.submitted = true;
            let admission = backend.submit(body, self.request.base, self.request.target);
            // Only an explicit trusted NotSubmitted result clears ambiguity.
            if matches!(admission, Ok(Admission::NotSubmitted(_))) {
                self.submitted = false;
            }
            self.check(clock())?;
            match admission {
                Ok(Admission::Queued(ticket)) => {
                    self.state = State::Awaiting(ticket);
                    Ok(None)
                }
                Ok(Admission::NotSubmitted(reason)) => self.finish(
                    snapshot,
                    match reason {
                        Refusal::Busy => Outcome::Busy,
                        Refusal::Unavailable => Outcome::Unavailable,
                        Refusal::Rejected => Outcome::Rejected,
                    },
                    snapshot.generation,
                ),
                Err(_) => Err(self.fail(Error::Backend)),
            }
        } else {
            let State::Awaiting(ticket) = std::mem::replace(&mut self.state, State::Calling) else {
                return Err(self.fail(Error::Closed));
            };
            let result = backend.completion(&ticket);
            self.check(clock())?;
            match result {
                Ok(None) => {
                    self.state = State::Awaiting(ticket);
                    Ok(None)
                }
                Ok(Some(completion)) => {
                    if completion.ticket != ticket {
                        return Err(self.fail(Error::Completion));
                    }
                    let (outcome, expected) = match completion.outcome {
                        reload::Outcome::Accepted => (Outcome::Committed, self.request.target),
                        reload::Outcome::Rejected => (Outcome::Rejected, self.request.base),
                        reload::Outcome::Cancelled => (Outcome::Cancelled, self.request.base),
                    };
                    if completion.generation != expected {
                        return Err(self.fail(Error::Completion));
                    }
                    self.finish(snapshot, outcome, Some(completion.generation))
                }
                Err(_) => Err(self.fail(Error::Backend)),
            }
        }
    }
}
#[cfg(test)]
mod tests;
