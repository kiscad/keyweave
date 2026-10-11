//! Deterministic control evidence, owned cold threads, no native input.
#![allow(clippy::unwrap_used)]
use keyweave_engine::Engine;
use keyweave_progress::reload::{Ack, FRAME_BYTES, Outcome, Request};
use keyweave_types::{ConfigGeneration, MonoTime};
use keyweave_worker::{
    preparation::PreparedConfig,
    reload_control::{Controller, Evidence, Step},
    reload_preparation::Target,
};
use std::{
    cell::{Cell, RefCell},
    io::{self, Read, Write},
    rc::Rc,
    time::{Duration, Instant},
};
const BODY: &[u8] = br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
fn frame(id: u64, base: u64, body: &[u8]) -> Vec<u8> {
    let mut bytes = Request {
        run_id: 42,
        id,
        base: ConfigGeneration(base),
        target: ConfigGeneration(base + 1),
        byte_length: body.len() as u64,
    }
    .encode()
    .unwrap()
    .to_vec();
    bytes.extend_from_slice(body);
    bytes
}
struct Reader {
    bytes: Vec<u8>,
    pos: usize,
    limit: usize,
    calls: Rc<Cell<usize>>,
}
impl Read for Reader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.calls.set(self.calls.get() + 1);
        let n = out.len().min(self.limit).min(self.bytes.len() - self.pos);
        if n == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        out[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
struct Writer {
    bytes: Rc<RefCell<Vec<u8>>>,
    limit: usize,
    fail: bool,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let n = bytes.len().min(self.limit);
        self.bytes.borrow_mut().extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("no flush")
    }
}
fn evidence(at: u64) -> io::Result<Evidence> {
    Ok(Evidence {
        at: MonoTime(at),
        parent_until: MonoTime(1000),
        session_until: MonoTime(1000),
        stop: false,
    })
}
fn actor() -> Engine {
    Engine::new(
        PreparedConfig::from_bytes(BODY.to_vec())
            .unwrap()
            .snapshot()
            .clone(),
    )
    .unwrap()
}
type Fixture = (
    Controller<Reader, Writer>,
    Rc<Cell<usize>>,
    Rc<RefCell<Vec<u8>>>,
);
fn fixture(bytes: Vec<u8>, read_limit: usize, write_limit: usize, fail: bool) -> Fixture {
    let calls = Rc::new(Cell::new(0));
    let written = Rc::new(RefCell::new(Vec::new()));
    let c = Controller::new(
        Reader {
            bytes,
            pos: 0,
            limit: read_limit,
            calls: calls.clone(),
        },
        Writer {
            bytes: written.clone(),
            limit: write_limit,
            fail,
        },
        42,
        ConfigGeneration(1),
        100,
        Target::Windows,
    )
    .unwrap();
    (c, calls, written)
}
fn to_ready<R: Read, W: Write>(c: &mut Controller<R, W>, actor: &mut Engine) {
    let until = Instant::now() + Duration::from_secs(5);
    while c.step() != Step::Ready {
        c.tick(actor, || evidence(10)).unwrap();
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
}
#[test]
fn two_transactions_reuse_owned_ack_endpoint_only_after_full_token() {
    let mut bytes = frame(1, 1, BODY);
    bytes.extend(frame(2, 2, b"invalid private config"));
    let (mut c, calls, written) = fixture(bytes, 7, 7, false);
    let mut a = actor();
    for expected in [Outcome::Accepted, Outcome::Rejected] {
        to_ready(&mut c, &mut a);
        let reads = calls.get();
        assert_eq!(
            c.tick(&mut a, || evidence(10)).unwrap(),
            Step::Acknowledging
        );
        assert_eq!(calls.get(), reads);
        while c.step() == Step::Acknowledging {
            c.tick(&mut a, || evidence(10)).unwrap();
            assert_eq!(calls.get(), reads);
        }
        assert_eq!(c.step(), Step::Receiving);
        assert_eq!(c.transaction_deadline(), None);
        let bytes = written.borrow();
        let ack = Ack::decode(&bytes[bytes.len() - FRAME_BYTES..]).unwrap();
        assert_eq!(ack.outcome, expected);
        assert_eq!(ack.active, ConfigGeneration(2));
    }
    assert_eq!(written.borrow().len(), 2 * FRAME_BYTES);
    assert_eq!(a.config_generation(), ConfigGeneration(2));
}
#[test]
fn idle_wouldblock_has_no_transaction_budget_but_independent_lease_still_expires() {
    let (mut c, calls, _) = fixture(vec![], 1, 48, false);
    let mut a = actor();
    for at in [10, 200, 999] {
        c.tick(&mut a, || evidence(at)).unwrap();
        assert_eq!(c.transaction_deadline(), None);
    }
    let n = calls.get();
    assert!(c.tick(&mut a, || evidence(1000)).is_err());
    assert_eq!(calls.get(), n);
    assert_eq!(c.step(), Step::Closed);
}
#[test]
fn first_partial_header_anchors_strict_deadline_without_renewal() {
    let (mut c, calls, _) = fixture(frame(1, 1, BODY), 1, 48, false);
    let mut a = actor();
    c.tick(&mut a, || evidence(10)).unwrap();
    assert_eq!(c.transaction_deadline(), Some(MonoTime(110)));
    c.tick(&mut a, || evidence(109)).unwrap();
    assert_eq!(c.transaction_deadline(), Some(MonoTime(110)));
    let n = calls.get();
    assert!(c.tick(&mut a, || evidence(110)).is_err());
    assert_eq!(calls.get(), n);
    assert_eq!(a.config_generation(), ConfigGeneration(1));
}
#[test]
fn post_header_expiry_closes_before_body_or_thread_start() {
    let (mut c, calls, _) = fixture(frame(1, 1, BODY), 48, 48, false);
    let mut a = actor();
    let mut n = 0;
    assert!(
        c.tick(&mut a, || {
            n += 1;
            evidence(if n == 1 { 10 } else { 110 })
        })
        .is_err()
    );
    assert_eq!(calls.get(), 1);
    assert!(c.reap_closed().unwrap());
    assert_eq!(a.config_generation(), ConfigGeneration(1));
}
#[test]
fn pre_stop_and_backwards_clock_close_without_next_io() {
    for stopped in [true, false] {
        let (mut c, calls, _) = fixture(frame(1, 1, BODY), 1, 48, false);
        let mut a = actor();
        c.tick(&mut a, || evidence(10)).unwrap();
        let n = calls.get();
        assert!(
            c.tick(&mut a, || {
                let mut e = evidence(if stopped { 10 } else { 9 }).unwrap();
                e.stop = stopped;
                Ok(e)
            })
            .is_err()
        );
        assert_eq!(calls.get(), n);
        assert!(
            c.tick(&mut a, || panic!("closed lane must not query evidence"))
                .is_err()
        );
    }
}
#[test]
fn post_commit_expiry_keeps_actual_generation_and_sends_no_ack() {
    let (mut c, _, written) = fixture(frame(1, 1, BODY), 48, 48, false);
    let mut a = actor();
    to_ready(&mut c, &mut a);
    let mut n = 0;
    assert!(
        c.tick(&mut a, || {
            n += 1;
            evidence(if n == 1 { 10 } else { 110 })
        })
        .is_err()
    );
    assert_eq!(a.config_generation(), ConfigGeneration(2));
    assert!(written.borrow().is_empty());
    assert_eq!(c.step(), Step::Closed);
}
#[test]
fn post_ack_expiry_and_broken_pipe_never_rollback_actual_commit() {
    for fail in [false, true] {
        let (mut c, _, written) = fixture(frame(1, 1, BODY), 48, 48, fail);
        let mut a = actor();
        to_ready(&mut c, &mut a);
        c.tick(&mut a, || evidence(10)).unwrap();
        let mut n = 0;
        assert!(
            c.tick(&mut a, || {
                n += 1;
                evidence(if n == 1 { 10 } else { 110 })
            })
            .is_err()
        );
        assert_eq!(a.config_generation(), ConfigGeneration(2));
        assert_eq!(written.borrow().len(), if fail { 0 } else { 48 });
        assert_eq!(c.step(), Step::Closed);
    }
}
#[test]
fn stop_preparing_reaps_same_job_and_cannot_activate_late_result() {
    let (mut c, _, written) = fixture(frame(1, 1, BODY), 48, 48, false);
    let mut a = actor();
    while c.step() != Step::Preparing {
        c.tick(&mut a, || evidence(10)).unwrap();
    }
    c.close();
    let until = Instant::now() + Duration::from_secs(5);
    while !c.reap_closed().unwrap() {
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    assert_eq!(a.config_generation(), ConfigGeneration(1));
    assert!(written.borrow().is_empty());
    assert_eq!(c.step(), Step::Closed);
}
#[cfg(target_os = "linux")]
#[test]
fn owned_linux_pipes_drive_cold_preparation_actual_engine_and_ack() {
    let (parent, worker) = keyweave_linux::reload_channel::channel().unwrap();
    let (mut ack_reader, mut request_writer) = parent.into_parts();
    let (reader, writer) = worker.into_parts();
    request_writer.write_all(&frame(1, 1, BODY)).unwrap();
    let mut c = Controller::new(
        reader,
        writer,
        42,
        ConfigGeneration(1),
        100,
        Target::Linux(keyweave_worker::linux_preparation::default_output_spec().unwrap()),
    )
    .unwrap();
    let mut a = actor();
    to_ready(&mut c, &mut a);
    c.tick(&mut a, || evidence(10)).unwrap();
    assert_eq!(c.tick(&mut a, || evidence(10)).unwrap(), Step::Receiving);
    let mut wire = [0; 48];
    ack_reader.read_exact(&mut wire).unwrap();
    let ack = Ack::decode(&wire).unwrap();
    assert_eq!(ack.outcome, Outcome::Accepted);
    assert_eq!(a.config_generation(), ack.active);
    assert_eq!(ack.active, ConfigGeneration(2));
    c.close();
    assert!(c.reap_closed().unwrap());
}
