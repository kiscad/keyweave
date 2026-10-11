//! Bounded streams for the independent reload lane. Native adapters must prove
//! nonblocking IO and endpoint ownership. These wrappers do not parse config.
use super::{Ack, Client, FRAME_BYTES, Outcome, ReloadError, Request, Worker};
use keyweave_types::{ConfigGeneration, MonoTime};
use std::io::{self, Read, Write};
pub const CHUNK_BYTES: usize = 64 * 1024;
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    #[error(transparent)]
    Protocol(#[from] ReloadError),
    #[error("reload endpoint failed: {0}")]
    Io(#[source] io::Error),
    #[error("reload clock failed: {0}")]
    Clock(#[source] io::Error),
    #[error("reload allocation failed")]
    Allocation,
}
fn count(
    result: io::Result<usize>,
    limit: usize,
    writing: bool,
) -> Result<Option<usize>, TransferError> {
    match result {
        Ok(0) => Err(TransferError::Io(io::Error::new(
            if writing {
                io::ErrorKind::WriteZero
            } else {
                io::ErrorKind::UnexpectedEof
            },
            "reload endpoint made no progress",
        ))),
        Ok(n) if n <= limit => Ok(Some(n)),
        Ok(_) => Err(TransferError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "reload IO count exceeds buffer",
        ))),
        Err(e)
            if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted =>
        {
            Ok(None)
        }
        Err(e) => Err(TransferError::Io(e)),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteStep {
    Pending,
    Complete,
}
/// Exact header followed by the owned raw candidate. No Clone/Debug/body access.
/// Complete means local writes accepted, not worker parsing/activation/ack.
pub struct RequestWriter<W: Write> {
    writer: W,
    request: Request,
    header: [u8; FRAME_BYTES],
    header_written: usize,
    body: Box<[u8]>,
    body_written: usize,
    closed: bool,
}
impl<W: Write> RequestWriter<W> {
    pub fn new(writer: W, request: Request, body: Box<[u8]>) -> Result<Self, TransferError> {
        let header = request.encode()?;
        if request.byte_length != body.len() as u64 {
            return Err(ReloadError::Length.into());
        }
        Ok(Self {
            writer,
            request,
            header,
            header_written: 0,
            body,
            body_written: 0,
            closed: false,
        })
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.body = Box::new([]);
    }
    fn poll_io(&mut self) -> Result<WriteStep, TransferError> {
        if self.closed {
            return Err(ReloadError::Closed.into());
        }
        if self.header_written < FRAME_BYTES {
            let slice = &self.header[self.header_written..];
            if let Some(n) = count(self.writer.write(slice), slice.len(), true)? {
                self.header_written += n;
            }
            return Ok(WriteStep::Pending); // never body IO in a header iteration
        }
        if self.body_written == self.body.len() {
            return Ok(WriteStep::Complete);
        }
        let end = (self.body_written + CHUNK_BYTES).min(self.body.len());
        let slice = &self.body[self.body_written..end];
        if let Some(n) = count(self.writer.write(slice), slice.len(), true)? {
            self.body_written += n;
        }
        Ok(if self.body_written == self.body.len() {
            WriteStep::Complete
        } else {
            WriteStep::Pending
        })
    }
}
/// Parent ack endpoint and its unique protocol client. Receiver is not progress.
pub struct Parent<R: Read> {
    reader: R,
    client: Client,
    bytes: [u8; FRAME_BYTES],
    read: usize,
    closed: bool,
}
impl<R: Read> Parent<R> {
    pub fn new(reader: R, client: Client) -> Self {
        Self {
            reader,
            client,
            bytes: [0; FRAME_BYTES],
            read: 0,
            closed: false,
        }
    }
    pub fn begin(&mut self, length: u64, now: MonoTime) -> Result<Request, ReloadError> {
        self.client.begin(length, now)
    }
    /// Derive byte length from the owned candidate and reserve its transaction
    /// together. Busy/bad candidate errors preserve the existing pending lane.
    pub fn start<W: Write>(
        &mut self,
        writer: W,
        body: Box<[u8]>,
        now: MonoTime,
    ) -> Result<RequestWriter<W>, TransferError> {
        let request = self.begin(body.len() as u64, now)?;
        match RequestWriter::new(writer, request, body) {
            Ok(writer) => Ok(writer),
            Err(error) => {
                self.close();
                Err(error)
            }
        }
    }
    pub fn active(&self) -> ConfigGeneration {
        self.client.active()
    }
    pub fn deadline(&self) -> Option<MonoTime> {
        self.client.deadline()
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.client.close();
    }
    fn check(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<MonoTime, TransferError> {
        if self.closed {
            return Err(ReloadError::Closed.into());
        }
        let now = clock().map_err(TransferError::Clock)?;
        self.client.poll(now)?;
        Ok(now)
    }
    /// One write, <=64KiB body, pre/post clock+original deadline checks. Cached
    /// local completion performs no write but still checks the in-flight lease.
    pub fn write<W: Write>(
        &mut self,
        writer: &mut RequestWriter<W>,
        mut clock: impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<WriteStep, TransferError> {
        let result = (|| {
            self.check(&mut clock)?;
            if self.client.pending() != Some(writer.request) {
                return Err(ReloadError::Identity.into());
            }
            let step = writer.poll_io()?;
            self.check(&mut clock)?;
            Ok(step)
        })();
        if result.is_err() {
            self.close();
            writer.close();
        }
        result
    }
    /// No ack IO without a pending request. Each read attempts only the rest of
    /// the fixed frame; post-IO clock failure/expiry prevents ack acceptance.
    pub fn poll(
        &mut self,
        mut clock: impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<Option<Outcome>, TransferError> {
        let result = (|| {
            self.check(&mut clock)?;
            if self.client.pending().is_none() {
                return Ok(None);
            }
            let slice = &mut self.bytes[self.read..];
            if let Some(n) = count(self.reader.read(slice), slice.len(), false)? {
                self.read += n;
            }
            let now = self.check(&mut clock)?;
            if self.read < FRAME_BYTES {
                return Ok(None);
            }
            let outcome = self.client.accept(Ack::decode(&self.bytes)?, now)?;
            self.read = 0;
            Ok(Some(outcome))
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveStep {
    Pending,
    Ready,
    AwaitingCommit,
    AwaitingAck,
}
/// One owned candidate. Debug intentionally omits body and run identity.
pub struct Received {
    pub request: Request,
    pub body: Vec<u8>,
}
impl std::fmt::Debug for Received {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReceivedReload")
            .field("request", &self.request)
            .field("byte_length", &self.body.len())
            .finish_non_exhaustive()
    }
}
pub struct Receiver<R: Read> {
    reader: R,
    worker: Worker,
    header: [u8; FRAME_BYTES],
    header_read: usize,
    request: Option<Request>,
    body: Vec<u8>,
    ready: bool,
    taken: bool,
    ack: Option<Ack>,
    closed: bool,
}
impl<R: Read> Receiver<R> {
    pub fn new(reader: R, worker: Worker) -> Self {
        Self {
            reader,
            worker,
            header: [0; FRAME_BYTES],
            header_read: 0,
            request: None,
            body: Vec::new(),
            ready: false,
            taken: false,
            ack: None,
            closed: false,
        }
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.worker.close();
        self.body = Vec::new();
    }
    pub fn active(&self) -> ConfigGeneration {
        self.worker.active()
    }
    /// At most one read. Header/run/id/base are checked before body allocation.
    /// No input callback use: reserving the bounded body is cold work, not a
    /// hard-real-time allocation guarantee. Caller must still check parent lease.
    pub fn poll(&mut self) -> Result<ReceiveStep, TransferError> {
        let result = self.poll_io();
        if result.is_err() {
            self.close();
        }
        result
    }
    fn poll_io(&mut self) -> Result<ReceiveStep, TransferError> {
        if self.closed {
            return Err(ReloadError::Closed.into());
        }
        if self.ack.is_some() {
            return Ok(ReceiveStep::AwaitingAck);
        }
        if self.taken {
            return Ok(ReceiveStep::AwaitingCommit);
        }
        if self.ready {
            return Ok(ReceiveStep::Ready);
        }
        if self.header_read < FRAME_BYTES {
            let slice = &mut self.header[self.header_read..];
            if let Some(n) = count(self.reader.read(slice), slice.len(), false)? {
                self.header_read += n;
            }
            if self.header_read == FRAME_BYTES {
                let request = Request::decode(&self.header)?;
                self.worker.begin(request)?;
                let size = usize::try_from(request.byte_length).map_err(|_| ReloadError::Length)?;
                self.body
                    .try_reserve_exact(size)
                    .map_err(|_| TransferError::Allocation)?;
                self.request = Some(request);
            }
            return Ok(ReceiveStep::Pending);
        }
        let request = self.request.ok_or(ReloadError::NoPending)?;
        let length = usize::try_from(request.byte_length).map_err(|_| ReloadError::Length)?;
        let old = self.body.len();
        let end = (old + CHUNK_BYTES).min(length);
        self.body.resize(end, 0);
        let slice = &mut self.body[old..];
        let read = count(self.reader.read(slice), slice.len(), false);
        match read {
            Ok(Some(n)) => self.body.truncate(old + n),
            Ok(None) => self.body.truncate(old),
            Err(e) => return Err(e),
        };
        self.ready = self.body.len() == length;
        Ok(if self.ready {
            ReceiveStep::Ready
        } else {
            ReceiveStep::Pending
        })
    }
    pub fn take(&mut self) -> Option<Received> {
        if self.closed || !self.ready || self.taken {
            return None;
        }
        let request = self.request?;
        self.ready = false;
        self.taken = true;
        Some(Received {
            request,
            body: std::mem::take(&mut self.body),
        })
    }
    /// Cold result delivery must match this exact taken transaction before
    /// mutating an actor. A closed/finished/different receiver cannot commit it.
    pub fn awaiting_commit(&self, request: Request) -> bool {
        !self.closed && self.taken && self.ack.is_none() && self.request == Some(request)
    }
    /// Invoke only with actual actor result/current generation. This does not
    /// send ack, and no new request is read until its exact delivery token.
    pub fn finish(
        &mut self,
        outcome: Outcome,
        active: ConfigGeneration,
    ) -> Result<Ack, TransferError> {
        let result = (|| {
            if self.closed {
                return Err(ReloadError::Closed.into());
            }
            if !self.taken || self.ack.is_some() {
                return Err(ReloadError::NoPending.into());
            }
            let ack = self.worker.finish(outcome, active)?;
            self.ack = Some(ack);
            Ok(ack)
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    pub fn ack_sent(&mut self, sent: AckSent) -> Result<(), TransferError> {
        if self.closed {
            return Err(ReloadError::Closed.into());
        }
        if self.ack != Some(sent.ack) {
            self.close();
            return Err(ReloadError::Identity.into());
        }
        self.ack = None;
        self.request = None;
        self.taken = false;
        self.header_read = 0;
        Ok(())
    }
}
/// Only AckWriter issues this local write-completion token. Not peer receipt,
/// actor proof or progress. No Clone/Copy; cannot be constructed externally.
pub struct AckSent {
    ack: Ack,
}
pub struct AckWriter<W: Write> {
    writer: W,
    ack: Ack,
    bytes: [u8; FRAME_BYTES],
    written: usize,
    closed: bool,
    issued: bool,
}
impl<W: Write> AckWriter<W> {
    pub fn new(writer: W, ack: Ack) -> Result<Self, TransferError> {
        Ok(Self {
            writer,
            ack,
            bytes: ack.encode()?,
            written: 0,
            closed: false,
            issued: false,
        })
    }
    pub fn close(&mut self) {
        self.closed = true;
    }
    /// At most one fixed-frame write. Completion token is issued exactly once.
    /// Worker supervision/time checks must bracket this separately from IO.
    pub fn poll(&mut self) -> Result<Option<AckSent>, TransferError> {
        let result = (|| {
            if self.closed {
                return Err(ReloadError::Closed.into());
            }
            if self.issued {
                return Ok(None);
            }
            let slice = &self.bytes[self.written..];
            if let Some(n) = count(self.writer.write(slice), slice.len(), true)? {
                self.written += n;
            }
            if self.written == FRAME_BYTES {
                self.issued = true;
                Ok(Some(AckSent { ack: self.ack }))
            } else {
                Ok(None)
            }
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};
    #[derive(Default)]
    struct State {
        bytes: VecDeque<u8>,
        reads: usize,
        writes: usize,
        read_caps: Vec<usize>,
        write_caps: Vec<usize>,
        max: usize,
        stall_read: bool,
        stall_write: bool,
        interrupt_write: bool,
        oversize_read: bool,
        oversize_write: bool,
        eof: bool,
        zero_write: bool,
    }
    #[derive(Clone)]
    struct Wire(Rc<RefCell<State>>);
    impl Wire {
        fn new(max: usize) -> Self {
            Self(Rc::new(RefCell::new(State {
                max,
                ..State::default()
            })))
        }
    }
    impl Write for Wire {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            let mut s = self.0.borrow_mut();
            s.writes += 1;
            s.write_caps.push(b.len());
            if s.interrupt_write {
                s.interrupt_write = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if s.stall_write {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            if s.zero_write {
                return Ok(0);
            }
            if s.oversize_write {
                return Ok(b.len() + 1);
            }
            let n = b.len().min(s.max);
            s.bytes.extend(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("transport must not flush/block")
        }
    }
    impl Read for Wire {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let mut s = self.0.borrow_mut();
            s.reads += 1;
            s.read_caps.push(b.len());
            if s.oversize_read {
                return Ok(b.len() + 1);
            }
            if s.eof {
                return Ok(0);
            }
            if s.stall_read || s.bytes.is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let n = b.len().min(s.max).min(s.bytes.len());
            for byte in &mut b[..n] {
                *byte = s.bytes.pop_front().unwrap();
            }
            Ok(n)
        }
    }
    fn parent(ack: Wire) -> Parent<Wire> {
        Parent::new(
            ack,
            Client::new(7, ConfigGeneration(1), MonoTime(0), 100).unwrap(),
        )
    }
    #[test]
    fn split_roundtrip_is_one_io_per_tick_and_waits_for_actual_ack_delivery() {
        let request_wire = Wire::new(4096);
        let ack_wire = Wire::new(7);
        let mut p = parent(ack_wire.clone());
        let body = vec![0x5a; CHUNK_BYTES + 17];
        let request = p.begin(body.len() as u64, MonoTime(1)).unwrap();
        let mut sender = RequestWriter::new(
            request_wire.clone(),
            request,
            body.clone().into_boxed_slice(),
        )
        .unwrap();
        let mut receiver = Receiver::new(
            request_wire.clone(),
            Worker::new(7, ConfigGeneration(1)).unwrap(),
        );
        request_wire.0.borrow_mut().interrupt_write = true;
        for _ in 0..100 {
            let before = request_wire.0.borrow().writes;
            p.write(&mut sender, || Ok(MonoTime(2))).unwrap();
            assert!(request_wire.0.borrow().writes <= before + 1);
            let before = request_wire.0.borrow().reads;
            let step = receiver.poll().unwrap();
            assert!(request_wire.0.borrow().reads <= before + 1);
            if step == ReceiveStep::Ready {
                break;
            }
        }
        let received = receiver.take().unwrap();
        assert_eq!(received.request, request);
        assert_eq!(received.body, body);
        assert!(receiver.take().is_none());
        let before = request_wire.0.borrow().reads;
        assert_eq!(receiver.poll().unwrap(), ReceiveStep::AwaitingCommit);
        assert_eq!(request_wire.0.borrow().reads, before);
        assert_eq!(p.active(), ConfigGeneration(1));
        let ack = receiver
            .finish(Outcome::Accepted, ConfigGeneration(2))
            .unwrap();
        assert_eq!(receiver.poll().unwrap(), ReceiveStep::AwaitingAck);
        assert_eq!(request_wire.0.borrow().reads, before);
        let mut writer = AckWriter::new(ack_wire.clone(), ack).unwrap();
        let mut sent = None;
        let mut outcome = None;
        for _ in 0..10 {
            sent = sent.or(writer.poll().unwrap());
            if let Some(value) = p.poll(|| Ok(MonoTime(3))).unwrap() {
                outcome = Some(value);
            }
            if sent.is_some() && outcome.is_some() {
                break;
            }
        }
        assert_eq!(outcome, Some(Outcome::Accepted));
        assert_eq!(p.active(), ConfigGeneration(2));
        receiver.ack_sent(sent.unwrap()).unwrap();
        assert!(writer.poll().unwrap().is_none());
        assert_eq!(p.begin(1, MonoTime(4)).unwrap().id, 2);
        assert!(
            request_wire
                .0
                .borrow()
                .read_caps
                .iter()
                .all(|n| *n <= CHUNK_BYTES)
        );
        assert!(
            request_wire
                .0
                .borrow()
                .write_caps
                .iter()
                .all(|n| *n <= CHUNK_BYTES)
        );
    }
    #[test]
    fn bad_header_identity_is_rejected_before_body_allocation_or_read() {
        let wire = Wire::new(usize::MAX);
        let request = Request {
            run_id: 8,
            id: 1,
            base: ConfigGeneration(1),
            target: ConfigGeneration(2),
            byte_length: super::super::MAX_UPDATE_BYTES,
        };
        wire.0.borrow_mut().bytes.extend(request.encode().unwrap());
        let mut receiver =
            Receiver::new(wire.clone(), Worker::new(7, ConfigGeneration(1)).unwrap());
        assert!(receiver.poll().is_err());
        assert_eq!(receiver.body.capacity(), 0);
        assert_eq!(wire.0.borrow().reads, 1);
        assert!(receiver.poll().is_err());
        assert_eq!(wire.0.borrow().reads, 1);
    }
    #[test]
    fn eof_after_partial_body_releases_candidate_and_never_retries_io() {
        let wire = Wire::new(usize::MAX);
        let request = Request {
            run_id: 7,
            id: 1,
            base: ConfigGeneration(1),
            target: ConfigGeneration(2),
            byte_length: 10,
        };
        wire.0.borrow_mut().bytes.extend(request.encode().unwrap());
        wire.0.borrow_mut().bytes.extend([1, 2, 3]);
        let mut receiver =
            Receiver::new(wire.clone(), Worker::new(7, ConfigGeneration(1)).unwrap());
        receiver.poll().unwrap();
        receiver.poll().unwrap();
        assert_eq!(receiver.body.len(), 3);
        wire.0.borrow_mut().eof = true;
        assert!(
            matches!(receiver.poll(),Err(TransferError::Io(e)) if e.kind()==io::ErrorKind::UnexpectedEof)
        );
        assert!(receiver.body.is_empty());
        let before = wire.0.borrow().reads;
        assert!(receiver.poll().is_err());
        assert_eq!(wire.0.borrow().reads, before);
        assert!(receiver.take().is_none());
    }
    #[test]
    fn post_write_expiry_fails_even_when_last_body_byte_was_accepted() {
        let wire = Wire::new(usize::MAX);
        let mut p = parent(Wire::new(1));
        let request = p.begin(1, MonoTime(0)).unwrap();
        let mut sender =
            RequestWriter::new(wire.clone(), request, vec![1].into_boxed_slice()).unwrap();
        p.write(&mut sender, || Ok(MonoTime(1))).unwrap();
        let mut times = [MonoTime(99), MonoTime(100)].into_iter();
        assert!(matches!(
            p.write(&mut sender, || Ok(times.next().unwrap())),
            Err(TransferError::Protocol(ReloadError::Expired))
        ));
        assert_eq!(wire.0.borrow().bytes.len(), FRAME_BYTES + 1);
        assert_eq!(p.active(), ConfigGeneration(1));
        assert!(sender.body.is_empty());
        let before = wire.0.borrow().writes;
        assert!(
            p.write(&mut sender, || panic!("closed lane must not sample again"))
                .is_err()
        );
        assert_eq!(wire.0.borrow().writes, before);
    }
    #[test]
    fn post_read_expiry_does_not_accept_a_full_committed_ack() {
        let wire = Wire::new(usize::MAX);
        let mut p = parent(wire.clone());
        let r = p.begin(1, MonoTime(0)).unwrap();
        let ack = Ack {
            run_id: r.run_id,
            id: r.id,
            base: r.base,
            target: r.target,
            active: r.target,
            outcome: Outcome::Accepted,
        };
        wire.0.borrow_mut().bytes.extend(ack.encode().unwrap());
        let mut times = [MonoTime(99), MonoTime(100)].into_iter();
        assert!(matches!(
            p.poll(|| Ok(times.next().unwrap())),
            Err(TransferError::Protocol(ReloadError::Expired))
        ));
        assert_eq!(p.active(), ConfigGeneration(1));
        assert_eq!(p.read, FRAME_BYTES);
    }
    #[test]
    fn wouldblock_does_not_reset_prefix_deadline_or_allocate_multiple_candidates() {
        let wire = Wire::new(3);
        let mut p = parent(Wire::new(3));
        let request = p.begin(4, MonoTime(0)).unwrap();
        let mut sender =
            RequestWriter::new(wire.clone(), request, vec![1; 4].into_boxed_slice()).unwrap();
        p.write(&mut sender, || Ok(MonoTime(1))).unwrap();
        wire.0.borrow_mut().stall_write = true;
        p.write(&mut sender, || Ok(MonoTime(2))).unwrap();
        assert_eq!(sender.header_written, 3);
        assert_eq!(p.deadline(), Some(MonoTime(100)));
        let mut receiver =
            Receiver::new(wire.clone(), Worker::new(7, ConfigGeneration(1)).unwrap());
        receiver.poll().unwrap();
        wire.0.borrow_mut().stall_read = true;
        receiver.poll().unwrap();
        assert_eq!(receiver.header_read, 3);
        assert_eq!(receiver.body.capacity(), 0);
    }
    #[test]
    fn oversized_count_zero_write_and_clock_error_close_without_more_io() {
        for oversized in [false, true] {
            let wire = Wire::new(10);
            wire.0.borrow_mut().oversize_write = oversized;
            wire.0.borrow_mut().zero_write = !oversized;
            let mut p = parent(Wire::new(10));
            let request = p.begin(1, MonoTime(0)).unwrap();
            let mut sender =
                RequestWriter::new(wire.clone(), request, vec![1].into_boxed_slice()).unwrap();
            assert!(p.write(&mut sender, || Ok(MonoTime(1))).is_err());
            assert!(p.write(&mut sender, || Ok(MonoTime(1))).is_err());
            assert_eq!(wire.0.borrow().writes, 1);
        }
        let wire = Wire::new(10);
        wire.0.borrow_mut().oversize_read = true;
        let mut receiver =
            Receiver::new(wire.clone(), Worker::new(7, ConfigGeneration(1)).unwrap());
        assert!(receiver.poll().is_err());
        assert!(receiver.poll().is_err());
        assert_eq!(wire.0.borrow().reads, 1);
        let wire = Wire::new(10);
        let mut p = parent(wire.clone());
        p.begin(1, MonoTime(0)).unwrap();
        assert!(matches!(
            p.poll(|| Err(io::ErrorKind::Other.into())),
            Err(TransferError::Clock(_))
        ));
        assert_eq!(wire.0.borrow().reads, 0);
    }
    #[test]
    fn wrong_ack_delivery_token_closes_receiver_without_rollback() {
        let wire = Wire::new(usize::MAX);
        let request = Request {
            run_id: 7,
            id: 1,
            base: ConfigGeneration(1),
            target: ConfigGeneration(2),
            byte_length: 1,
        };
        wire.0.borrow_mut().bytes.extend(request.encode().unwrap());
        wire.0.borrow_mut().bytes.push_back(1);
        let mut receiver = Receiver::new(wire, Worker::new(7, ConfigGeneration(1)).unwrap());
        receiver.poll().unwrap();
        receiver.poll().unwrap();
        receiver.take().unwrap();
        let mut ack = receiver
            .finish(Outcome::Accepted, ConfigGeneration(2))
            .unwrap();
        ack.id = 2;
        let mut writer = AckWriter::new(Vec::<u8>::new(), ack).unwrap();
        let sent = writer.poll().unwrap().unwrap();
        assert!(receiver.ack_sent(sent).is_err());
        assert_eq!(receiver.active(), ConfigGeneration(2));
        assert!(receiver.poll().is_err());
    }
    #[test]
    fn second_transaction_uses_clean_header_and_rejection_keeps_last_commit() {
        let request_wire = Wire::new(usize::MAX);
        let ack_wire = Wire::new(usize::MAX);
        let mut parent = parent(ack_wire.clone());
        let mut receiver = Receiver::new(
            request_wire.clone(),
            Worker::new(7, ConfigGeneration(1)).unwrap(),
        );
        for outcome in [Outcome::Accepted, Outcome::Rejected] {
            let mut sender = parent
                .start(
                    request_wire.clone(),
                    vec![42].into_boxed_slice(),
                    MonoTime(1),
                )
                .unwrap();
            parent.write(&mut sender, || Ok(MonoTime(1))).unwrap();
            receiver.poll().unwrap();
            parent.write(&mut sender, || Ok(MonoTime(1))).unwrap();
            receiver.poll().unwrap();
            let received = receiver.take().unwrap();
            assert_eq!(
                received.request.id,
                if outcome == Outcome::Accepted { 1 } else { 2 }
            );
            assert_eq!(received.body, vec![42]);
            let ack = receiver.finish(outcome, ConfigGeneration(2)).unwrap();
            let mut sender = AckWriter::new(ack_wire.clone(), ack).unwrap();
            receiver.ack_sent(sender.poll().unwrap().unwrap()).unwrap();
            assert_eq!(parent.poll(|| Ok(MonoTime(1))).unwrap(), Some(outcome));
            assert_eq!(parent.active(), ConfigGeneration(2));
            assert_eq!(receiver.active(), ConfigGeneration(2));
        }
        let before = ack_wire.0.borrow().reads;
        assert_eq!(parent.poll(|| Ok(MonoTime(1))).unwrap(), None);
        assert_eq!(ack_wire.0.borrow().reads, before);
    }
}
