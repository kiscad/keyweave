#![allow(clippy::unwrap_used)]
use super::*;
use keyweave_progress::session_control::{Command, Outcome, Phase, Snapshot};
use keyweave_windows::session_control::{MESSAGE_CAPACITY, ReadResult};
use std::{cell::RefCell, io, rc::Rc};
struct State {
    calls: Vec<&'static str>,
    pending: Option<&'static str>,
    fail: Option<&'static str>,
    reply: Vec<u8>,
    writes: Vec<Vec<u8>>,
}
#[derive(Clone)]
struct Transport(Rc<RefCell<State>>);
fn reply() -> Reply {
    Reply {
        id: 3,
        outcome: Outcome::Status,
        snapshot: Snapshot {
            session: 17,
            phase: Phase::Starting,
            progress_epoch: 0,
            generation: None,
            exit: None,
            recovery_confirmed: false,
            fault: false,
        },
    }
}
fn request() -> Request {
    Request {
        id: 3,
        command: Command::Status,
    }
}
impl Transport {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(State {
            calls: vec![],
            pending: None,
            fail: None,
            reply: reply().encode().unwrap().to_vec(),
            writes: vec![],
        })))
    }
    fn call<T>(
        &mut self,
        name: &'static str,
        f: impl FnOnce(&mut State) -> T,
    ) -> Poll<io::Result<T>> {
        let mut state = self.0.borrow_mut();
        state.calls.push(name);
        if state.pending == Some(name) {
            return Poll::Pending;
        }
        if state.fail == Some(name) {
            return Poll::Ready(Err(io::Error::other(format!("test {name} failure"))));
        }
        Poll::Ready(Ok(f(&mut state)))
    }
}
impl ClientTransport for Transport {
    fn connect(&mut self) -> Poll<io::Result<()>> {
        self.call("connect", |_| ())
    }
    fn authorize(&mut self) -> Poll<io::Result<bool>> {
        self.call("authorize", |_| true)
    }
    fn write(&mut self, b: &[u8; 32]) -> Poll<io::Result<()>> {
        self.call("write", |s| s.writes.push(b.to_vec()))
    }
    fn read(&mut self, b: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>> {
        self.call("read", |s| {
            let length = s.reply.len().min(b.len());
            b[..length].copy_from_slice(&s.reply[..length]);
            ReadResult::Message {
                length,
                more: s.reply.len() > b.len(),
            }
        })
    }
    fn close(&mut self) -> Poll<io::Result<()>> {
        self.call("close", |_| ())
    }
}
#[test]
fn gui_driver_uses_actual_client_codec_one_method_per_frame_and_returns_only_after_close() {
    let transport = Transport::new();
    let state = transport.0.clone();
    let start = Instant::now();
    let mut driver = MessageRequest::new_at(transport, request(), start).unwrap();
    for tick in 0..5 {
        let before = state.borrow().calls.len();
        let value = driver.poll_at(start + Duration::from_millis(tick)).unwrap();
        assert_eq!(state.borrow().calls.len() - before, 1);
        if tick == 4 {
            assert_eq!(value, Some(reply()));
        } else {
            assert!(value.is_none());
        }
    }
    assert_eq!(
        state.borrow().calls,
        vec!["connect", "authorize", "write", "read", "close"]
    );
    assert_eq!(
        state.borrow().writes,
        vec![request().encode().unwrap().to_vec()]
    );
    assert!(
        driver
            .poll_at(start + Duration::from_secs(2))
            .unwrap()
            .is_none()
    );
    assert_eq!(state.borrow().calls.len(), 5);
}
#[test]
fn pending_does_not_extend_transaction_or_cleanup_and_expiry_makes_no_extra_io() {
    let transport = Transport::new();
    let state = transport.0.clone();
    state.borrow_mut().pending = Some("read");
    let start = Instant::now();
    let mut driver = MessageRequest::new_at(transport, request(), start).unwrap();
    for tick in [0, 1, 2, 3, 999] {
        assert!(
            driver
                .poll_at(start + Duration::from_millis(tick))
                .unwrap()
                .is_none()
        );
    }
    let before = state.borrow().calls.len();
    driver.poll_at(start + Duration::from_millis(1000)).unwrap();
    assert_eq!(state.borrow().calls.len(), before);
    state.borrow_mut().pending = Some("close");
    driver.poll_at(start + Duration::from_millis(1099)).unwrap();
    driver.poll_at(start + Duration::from_millis(1100)).unwrap();
    let before = state.borrow().calls.len();
    assert!(
        driver
            .poll_at(start + Duration::from_millis(1101))
            .unwrap_err()
            .contains("cleanup unconfirmed")
    );
    assert_eq!(state.borrow().calls.len(), before);
}
#[test]
fn primary_and_cleanup_failure_remain_visible_without_success_after_a_valid_reply() {
    for failed_method in ["write", "close"] {
        let transport = Transport::new();
        let state = transport.0.clone();
        state.borrow_mut().fail = Some(failed_method);
        let start = Instant::now();
        let mut driver = MessageRequest::new_at(transport, request(), start).unwrap();
        let failed_tick = if failed_method == "write" { 2 } else { 4 };
        for tick in 0..=failed_tick {
            assert!(
                driver
                    .poll_at(start + Duration::from_millis(tick))
                    .unwrap()
                    .is_none()
            );
        }
        state.borrow_mut().fail = Some("close");
        driver
            .poll_at(start + Duration::from_millis(failed_tick + 1))
            .unwrap();
        state.borrow_mut().fail = None;
        let error = driver
            .poll_at(start + Duration::from_millis(failed_tick + 2))
            .unwrap_err();
        assert!(error.contains(failed_method) && error.contains("close"));
    }
}
#[test]
fn mismatched_or_excess_reply_never_becomes_an_observation_even_after_normal_close() {
    for bytes in [vec![0; 64], vec![0; 200], {
        let mut value = reply();
        value.id = 4;
        value.encode().unwrap().to_vec()
    }] {
        let transport = Transport::new();
        transport.0.borrow_mut().reply = bytes;
        let start = Instant::now();
        let mut driver = MessageRequest::new_at(transport, request(), start).unwrap();
        for tick in 0..4 {
            assert!(
                driver
                    .poll_at(start + Duration::from_millis(tick))
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            driver
                .poll_at(start + Duration::from_millis(4))
                .unwrap_err()
                .contains("Malformed")
        );
    }
}
