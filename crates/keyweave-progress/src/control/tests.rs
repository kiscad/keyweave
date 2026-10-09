#![allow(clippy::unwrap_used)]
use crate::*;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};
struct Reader {
    bytes: VecDeque<u8>,
    chunk: usize,
    calls: Rc<Cell<usize>>,
}
impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.calls.set(self.calls.get() + 1);
        if self.bytes.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = bytes.len().min(self.bytes.len()).min(self.chunk);
        for out in &mut bytes[..n] {
            *out = self.bytes.pop_front().unwrap();
        }
        Ok(n)
    }
}
struct Writer {
    bytes: Rc<RefCell<Vec<u8>>>,
    calls: Rc<Cell<usize>>,
    chunk: usize,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls.set(self.calls.get() + 1);
        let n = bytes.len().min(self.chunk);
        self.bytes.borrow_mut().extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn client(bytes: Vec<u8>, chunk: usize) -> ProgressClient<Reader, Writer> {
    ProgressClient::new(
        Reader {
            bytes: bytes.into(),
            chunk,
            calls: Rc::new(Cell::new(0)),
        },
        Writer {
            bytes: Rc::new(RefCell::new(vec![])),
            chunk: 5,
            calls: Rc::new(Cell::new(0)),
        },
        7,
    )
    .unwrap()
}
#[test]
fn stop_wire_is_distinct_strict_and_legacy_challenge_rejects_it() {
    let stop = StopRequest {
        run_id: u64::MAX,
        id: 1,
    };
    assert_eq!(StopRequest::decode(&stop.encode()), Ok(stop));
    assert_eq!(&stop.encode()[..8], b"KWPS\x01\x00\x00\x00");
    assert_eq!(Challenge::decode(&stop.encode()), Err(ProgressError::Frame));
    for n in [0, 23, 25] {
        assert!(StopRequest::decode(&vec![0; n]).is_err());
    }
    for at in [0, 4, 5, 6, 7] {
        let mut b = stop.encode();
        b[at] = 255;
        assert!(StopRequest::decode(&b).is_err());
    }
    let mut b = stop.encode();
    b[8..16].fill(0);
    assert_eq!(StopRequest::decode(&b), Err(ProgressError::Identity));
    let mut b = stop.encode();
    b[16..24].fill(0);
    assert_eq!(StopRequest::decode(&b), Err(ProgressError::Identity));
    let mut legacy = client(StopRequest { run_id: 7, id: 1 }.encode().to_vec(), 24);
    assert!(matches!(
        legacy.receive(0),
        Err(ClientError::Protocol(ProgressError::Frame))
    ));
}
#[test]
fn partial_initial_stop_has_no_receipt_and_terminal_io_is_forbidden() {
    let mut c = client(StopRequest { run_id: 7, id: 1 }.encode().to_vec(), 7);
    for epoch in [1, 2, 3] {
        assert_eq!(c.receive_command(epoch).unwrap(), RequestEvent::Pending);
    }
    assert_eq!(c.receive_command(4).unwrap(), RequestEvent::Stop);
    assert!(!c.can_receive());
    let reads = c.reader.calls.get();
    assert_eq!(c.receive_command(500).unwrap(), RequestEvent::Pending);
    assert!(!c.receive(600).unwrap());
    assert!(matches!(
        c.publish(700, ProgressPhase::Stopped),
        Err(ClientError::Stopped)
    ));
    assert!(matches!(c.flush(), Err(ClientError::Stopped)));
    assert_eq!(c.reader.calls.get(), reads);
    assert_eq!(c.writer.calls.get(), 0);
}
#[test]
fn stop_preempts_standby_ticket_or_partial_active_reply_without_flushing_it() {
    for partial in [false, true] {
        let bytes = [
            Challenge { run_id: 7, id: 1 }.encode(),
            StopRequest { run_id: 7, id: 2 }.encode(),
        ]
        .concat();
        let mut c = client(bytes, 24);
        assert_eq!(c.receive_command(10).unwrap(), RequestEvent::Challenge);
        if partial {
            assert!(!c.publish(11, ProgressPhase::Active).unwrap());
        }
        let sent = c.writer.bytes.borrow().clone();
        assert_eq!(c.receive_command(12).unwrap(), RequestEvent::Stop);
        assert_eq!(c.response_pending(), partial);
        assert_eq!(c.ticket.unwrap().1, 10);
        assert!(matches!(c.flush(), Err(ClientError::Stopped)));
        assert_eq!(*c.writer.bytes.borrow(), sent);
    }
}
#[test]
fn parked_challenge_baseline_is_acceptance_time_and_following_stop_is_not_progress() {
    let bytes = [
        Challenge { run_id: 7, id: 1 }.encode(),
        Challenge { run_id: 7, id: 2 }.encode(),
        StopRequest { run_id: 7, id: 3 }.encode(),
    ]
    .concat();
    let mut c = client(bytes, 24);
    assert_eq!(c.receive_command(0).unwrap(), RequestEvent::Challenge);
    assert!(!c.publish(1, ProgressPhase::Active).unwrap());
    assert_eq!(c.receive_command(2).unwrap(), RequestEvent::Pending);
    for _ in 0..6 {
        assert!(!c.flush().unwrap());
    }
    assert!(c.flush().unwrap());
    assert_eq!(c.receive_command(8).unwrap(), RequestEvent::Challenge);
    assert_eq!(c.ticket.unwrap().1, 8);
    assert_eq!(c.receive_command(9).unwrap(), RequestEvent::Stop);
    assert_eq!(c.writer.bytes.borrow().len(), 40);
    assert_eq!(Receipt::decode(&c.writer.bytes.borrow()).unwrap().epoch, 1);
}
#[test]
fn wrong_identity_and_malformed_stop_cannot_preempt_a_valid_ticket() {
    for stop in [
        StopRequest { run_id: 8, id: 2 },
        StopRequest { run_id: 7, id: 1 },
        StopRequest { run_id: 7, id: 3 },
    ] {
        let mut c = client(
            [Challenge { run_id: 7, id: 1 }.encode(), stop.encode()].concat(),
            24,
        );
        c.receive_command(0).unwrap();
        assert!(matches!(
            c.receive_command(1),
            Err(ClientError::Protocol(ProgressError::Identity))
        ));
        assert!(!c.stop_received);
    }
    let mut bad = StopRequest { run_id: 7, id: 2 }.encode();
    bad[5] = 1;
    let mut c = client([Challenge { run_id: 7, id: 1 }.encode(), bad].concat(), 24);
    c.receive_command(0).unwrap();
    assert!(matches!(
        c.receive_command(1),
        Err(ClientError::Protocol(ProgressError::Frame))
    ));
}
