//! One parent reload transaction. No files, parser, process or progress APIs.
use keyweave_progress::reload::{
    Client, MAX_UPDATE_BYTES, Outcome, ReloadError,
    transport::{Parent, RequestWriter, TransferError, WriteStep},
};
use keyweave_types::{ConfigGeneration, MonoTime};
use std::io::{self, Read, Write};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadState {
    Idle,
    Queued,
    Sending,
    AwaitingAck,
    Closed,
}
pub(super) fn system_error(error: TransferError) -> io::Error {
    match error {
        TransferError::Io(error) | TransferError::Clock(error) => error,
        TransferError::Protocol(error) => io::Error::new(
            if error == ReloadError::Expired {
                io::ErrorKind::TimedOut
            } else {
                io::ErrorKind::InvalidData
            },
            error,
        ),
        TransferError::Allocation => {
            io::Error::new(io::ErrorKind::OutOfMemory, "reload allocation failed")
        }
    }
}
pub(super) type NativeReload = ReloadLane<std::fs::File, std::fs::File>;
pub(super) struct ReloadLane<R: Read, W: Write> {
    parent: Option<Parent<R>>,
    writer: Option<W>,
    request: Option<RequestWriter<W>>,
    queued: Option<Box<[u8]>>,
    completion: Option<Outcome>,
    active: ConfigGeneration,
    state: ReloadState,
    run: u64,
}
impl<R: Read, W: Write> ReloadLane<R, W> {
    pub(super) fn new(reader: R, writer: W, run: u64) -> Result<Self, TransferError> {
        Ok(Self {
            parent: Some(Parent::new(
                reader,
                Client::new(run, ConfigGeneration(1), MonoTime(0), 5_000_000_000)?,
            )),
            writer: Some(writer),
            request: None,
            queued: None,
            completion: None,
            active: ConfigGeneration(1),
            state: ReloadState::Idle,
            run,
        })
    }
    pub(super) fn submit(&mut self, body: Box<[u8]>) -> Result<(), ReloadError> {
        if self.state == ReloadState::Closed {
            return Err(ReloadError::Closed);
        }
        if self.state != ReloadState::Idle || self.completion.is_some() {
            return Err(ReloadError::Busy);
        }
        if body.is_empty() || body.len() as u64 > MAX_UPDATE_BYTES {
            return Err(ReloadError::Length);
        }
        self.queued = Some(body);
        self.state = ReloadState::Queued;
        Ok(())
    }
    pub(super) fn run_id(&self) -> u64 {
        self.run
    }
    pub(super) fn can_submit(&self) -> bool {
        self.state == ReloadState::Idle && self.completion.is_none()
    }
    pub(super) fn state(&self) -> ReloadState {
        self.state
    }
    pub(super) fn generation(&self) -> ConfigGeneration {
        self.active
    }
    pub(super) fn take_completion(&mut self) -> Option<Outcome> {
        self.completion.take()
    }
    pub(super) fn close(&mut self) {
        self.state = ReloadState::Closed;
        self.parent = None;
        self.writer = None;
        self.request = None;
        self.queued = None;
    }
    pub(super) fn poll(
        &mut self,
        mut clock: impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<(), TransferError> {
        let result = self.poll_inner(&mut clock);
        if result.is_err() {
            self.close()
        }
        result
    }
    fn poll_inner(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<(), TransferError> {
        if matches!(self.state, ReloadState::Idle | ReloadState::Closed) {
            return Ok(());
        }
        let parent = self.parent.as_mut().ok_or(ReloadError::Closed)?;
        match self.state {
            ReloadState::Queued => {
                let before = clock().map_err(TransferError::Clock)?;
                self.request = Some(parent.start(
                    self.writer.take().ok_or(ReloadError::Closed)?,
                    self.queued.take().ok_or(ReloadError::NoPending)?,
                    before,
                )?);
                parent.verify_clock(clock)?;
                self.state = ReloadState::Sending;
            }
            ReloadState::Sending => {
                if parent.write(self.request.as_mut().ok_or(ReloadError::NoPending)?, clock)?
                    == WriteStep::Complete
                {
                    self.writer = self
                        .request
                        .take()
                        .and_then(RequestWriter::into_completed_writer);
                    if self.writer.is_none() {
                        return Err(ReloadError::Closed.into());
                    }
                    self.state = ReloadState::AwaitingAck;
                }
            }
            ReloadState::AwaitingAck => {
                if let Some(outcome) = parent.poll(clock)? {
                    self.active = parent.active();
                    self.completion = Some(outcome);
                    self.state = ReloadState::Idle;
                }
            }
            ReloadState::Idle | ReloadState::Closed => {}
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use keyweave_progress::reload::{Ack, FRAME_BYTES, Request};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    struct Reader {
        bytes: io::Cursor<Vec<u8>>,
        calls: Rc<Cell<usize>>,
        blocked: bool,
    }
    impl Read for Reader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            if self.blocked {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            self.bytes.read(out)
        }
    }
    struct Writer {
        bytes: Rc<RefCell<Vec<u8>>>,
        chunk: usize,
    }
    impl Write for Writer {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            let n = b.len().min(self.chunk);
            self.bytes.borrow_mut().extend_from_slice(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("no flush")
        }
    }
    fn ack(id: u64, base: u64, outcome: Outcome) -> Vec<u8> {
        Ack {
            run_id: 42,
            id,
            base: ConfigGeneration(base),
            target: ConfigGeneration(base + 1),
            active: ConfigGeneration(if outcome == Outcome::Accepted {
                base + 1
            } else {
                base
            }),
            outcome,
        }
        .encode()
        .unwrap()
        .to_vec()
    }
    type Fixture = (
        ReloadLane<Reader, Writer>,
        Rc<Cell<usize>>,
        Rc<RefCell<Vec<u8>>>,
    );
    fn fixture(bytes: Vec<u8>, blocked: bool) -> Fixture {
        let calls = Rc::new(Cell::new(0));
        let written = Rc::new(RefCell::new(vec![]));
        (
            ReloadLane::new(
                Reader {
                    bytes: io::Cursor::new(bytes),
                    calls: calls.clone(),
                    blocked,
                },
                Writer {
                    bytes: written.clone(),
                    chunk: 7,
                },
                42,
            )
            .unwrap(),
            calls,
            written,
        )
    }
    fn send(lane: &mut ReloadLane<Reader, Writer>) {
        let mut steps = 0;
        while lane.state() != ReloadState::AwaitingAck {
            lane.poll(|| Ok(MonoTime(10))).unwrap();
            steps += 1;
            assert!(steps < 32)
        }
    }
    #[test]
    fn same_endpoint_reuse_and_completion_gate_preserve_accepted_then_rejected_generation() {
        let mut replies = ack(1, 1, Outcome::Accepted);
        replies.extend(ack(2, 2, Outcome::Rejected));
        let (mut lane, calls, written) = fixture(replies, false);
        for (body, expected) in [(b"one", Outcome::Accepted), (b"two", Outcome::Rejected)] {
            lane.submit(Box::new(*body)).unwrap();
            send(&mut lane);
            let before = calls.get();
            lane.poll(|| Ok(MonoTime(10))).unwrap();
            assert_eq!(calls.get(), before + 1);
            assert_eq!(lane.generation(), ConfigGeneration(2));
            assert_eq!(lane.state(), ReloadState::Idle);
            assert_eq!(
                lane.submit(Box::new(*b"new")).unwrap_err(),
                ReloadError::Busy
            );
            assert_eq!(lane.take_completion(), Some(expected));
            assert_eq!(lane.take_completion(), None);
        }
        let bytes = written.borrow();
        assert_eq!(bytes.len(), 2 * (FRAME_BYTES + 3));
        let first = Request::decode(&bytes[..48]).unwrap();
        let second = Request::decode(&bytes[51..99]).unwrap();
        assert_eq!((first.id, first.base.0), (1, 1));
        assert_eq!((second.id, second.base.0), (2, 2));
    }
    #[test]
    fn idle_queue_busy_and_stop_have_no_io_or_time_renewal() {
        let (mut lane, calls, written) = fixture(vec![], true);
        lane.poll(|| panic!("idle no clock")).unwrap();
        assert_eq!(lane.submit(Box::new([])).unwrap_err(), ReloadError::Length);
        lane.submit(Box::new(*b"one")).unwrap();
        assert_eq!(
            lane.submit(Box::new(*b"two")).unwrap_err(),
            ReloadError::Busy
        );
        lane.close();
        lane.poll(|| panic!("closed no clock")).unwrap();
        assert_eq!(calls.get(), 0);
        assert!(written.borrow().is_empty());
        assert_eq!(
            lane.submit(Box::new(*b"new")).unwrap_err(),
            ReloadError::Closed
        );
    }
    #[test]
    fn blocked_ack_strict_original_deadline_closes_without_advancing() {
        let (mut lane, calls, _) = fixture(vec![], true);
        lane.submit(Box::new(*b"one")).unwrap();
        send(&mut lane);
        lane.poll(|| Ok(MonoTime(5_000_000_009))).unwrap();
        let reads = calls.get();
        assert!(lane.poll(|| Ok(MonoTime(5_000_000_010))).is_err());
        assert_eq!(calls.get(), reads);
        assert_eq!(lane.generation(), ConfigGeneration(1));
        assert_eq!(lane.state(), ReloadState::Closed);
    }
    #[test]
    fn complete_late_ack_is_not_observed_as_accepted() {
        let (mut lane, _, _) = fixture(ack(1, 1, Outcome::Accepted), false);
        lane.submit(Box::new(*b"one")).unwrap();
        send(&mut lane);
        let mut calls = 0;
        assert!(
            lane.poll(|| {
                calls += 1;
                Ok(MonoTime(if calls == 1 { 10 } else { 5_000_000_010 }))
            })
            .is_err()
        );
        assert_eq!(lane.take_completion(), None);
        assert_eq!(lane.generation(), ConfigGeneration(1));
    }
    #[test]
    fn post_queue_clock_failure_drops_body_and_endpoint_without_write() {
        let (mut lane, reads, written) = fixture(vec![], true);
        lane.submit(Box::new(*b"one")).unwrap();
        let mut calls = 0;
        assert!(
            lane.poll(|| {
                calls += 1;
                if calls == 1 {
                    Ok(MonoTime(10))
                } else {
                    Err(io::ErrorKind::BrokenPipe.into())
                }
            })
            .is_err()
        );
        assert_eq!(reads.get(), 0);
        assert!(written.borrow().is_empty());
        assert_eq!(lane.state(), ReloadState::Closed);
    }
}
