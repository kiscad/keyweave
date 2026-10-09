#![allow(clippy::unwrap_used)]
use super::super::hook_thread::{test_quit, test_send, with_callback_borrow};
use super::super::input_probe::test_probe;
use super::*;
use crate::{
    keys::KeyboardPacket,
    progress::{Challenge, ProgressPhase, Receipt},
    pump::{Insertion, SubmitError},
};
use keyweave_types::ConfigGeneration;
use std::{cell::RefCell, collections::VecDeque, io, rc::Rc};
#[derive(Default)]
struct Trace {
    packets: Vec<Vec<KeyboardPacket>>,
    unknown: bool,
}
struct Backend(Rc<RefCell<Trace>>);
impl SendBackend for Backend {
    fn submit(&mut self, packets: &[KeyboardPacket]) -> Result<Insertion, SubmitError> {
        if packets[0].flags & 2 != 0 && super::super::outside_callback() {
            assert_eq!(test_send(0x30, false), 9);
        }
        let mut t = self.0.borrow_mut();
        t.packets.push(packets.to_vec());
        if t.unknown {
            Err(SubmitError::Unknown)
        } else {
            Ok(Insertion {
                accepted: packets.len(),
                last_error: None,
            })
        }
    }
}
#[derive(Default)]
struct Input {
    bytes: VecDeque<u8>,
    calls: usize,
    chunk: usize,
    eof: bool,
    error: bool,
}
struct Reader(Rc<RefCell<Input>>);
impl Read for Reader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut x = self.0.borrow_mut();
        x.calls += 1;
        if x.error {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if x.bytes.is_empty() {
            return if x.eof {
                Ok(0)
            } else {
                Err(io::ErrorKind::WouldBlock.into())
            };
        }
        let n = out.len().min(x.chunk).min(x.bytes.len());
        for b in &mut out[..n] {
            *b = x.bytes.pop_front().unwrap();
        }
        Ok(n)
    }
}
#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    calls: usize,
    chunk: usize,
    error: bool,
}
struct Writer(Rc<RefCell<Output>>);
impl Write for Writer {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let mut x = self.0.borrow_mut();
        x.calls += 1;
        if x.error {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if x.chunk == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = data.len().min(x.chunk);
        x.bytes.extend_from_slice(&data[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
type Loop = Driver<Backend, Reader, Writer>;
fn config() -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "worker test".into(),
        simple: vec![],
        complex: vec![],
    })
}
type Fixture = (
    Loop,
    Rc<RefCell<Input>>,
    Rc<RefCell<Output>>,
    Rc<RefCell<Trace>>,
);
fn fixture() -> Fixture {
    let trace = Rc::new(RefCell::new(Trace::default()));
    let session = CaptureSession::install(
        Backend(trace.clone()),
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
        config(),
    )
    .unwrap();
    let input = Rc::new(RefCell::new(Input {
        chunk: 24,
        ..Input::default()
    }));
    input
        .borrow_mut()
        .bytes
        .extend(Challenge { run_id: 7, id: 1 }.encode());
    let output = Rc::new(RefCell::new(Output {
        chunk: 40,
        ..Output::default()
    }));
    let client = ProgressClient::new(Reader(input.clone()), Writer(output.clone()), 7).unwrap();
    (
        Driver {
            session,
            client,
            report: None,
        },
        input,
        output,
        trace,
    )
}
#[test]
fn receive_precedes_actual_poll_and_only_the_completed_poll_publishes() {
    let (mut worker, input, output, _) = fixture();
    assert_eq!(worker.session.progress_epoch(), 0);
    assert_eq!(
        worker.step().unwrap(),
        WorkerStep::Session(SessionStep::Activated)
    );
    let first = Receipt::decode(&output.borrow().bytes).unwrap();
    assert_eq!(
        (first.baseline, first.epoch, first.phase),
        (0, 1, ProgressPhase::Active)
    );
    assert_eq!(input.borrow().calls, 1);
    assert_eq!(output.borrow().calls, 1);
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(test_send(0x1e, true), 1);
    assert_eq!(worker.session.progress_epoch(), 1);
    input
        .borrow_mut()
        .bytes
        .extend(Challenge { run_id: 7, id: 2 }.encode());
    worker.step().unwrap();
    let next = Receipt::decode(&output.borrow().bytes[40..]).unwrap();
    assert_eq!((next.baseline, next.epoch), (1, 2));
    worker.request_stop().unwrap();
}
#[test]
fn passive_poll_keeps_ticket_but_does_not_publish_before_activation() {
    let (mut worker, input, output, _) = fixture();
    test_probe(i16::MIN, true);
    assert_eq!(
        worker.step().unwrap(),
        WorkerStep::Session(SessionStep::Waiting)
    );
    assert_eq!(worker.session.progress_epoch(), 1);
    assert!(output.borrow().bytes.is_empty());
    assert_eq!(input.borrow().calls, 1);
    test_probe(0, true);
    assert_eq!(
        worker.step().unwrap(),
        WorkerStep::Session(SessionStep::Activated)
    );
    let frame = Receipt::decode(&output.borrow().bytes).unwrap();
    assert_eq!((frame.baseline, frame.epoch), (0, 2));
    worker.request_stop().unwrap();
}
#[test]
fn partial_reply_remains_original_snapshot_and_next_ticket_uses_current_poll() {
    let (mut worker, input, output, _) = fixture();
    output.borrow_mut().chunk = 5;
    input
        .borrow_mut()
        .bytes
        .extend(Challenge { run_id: 7, id: 2 }.encode());
    for _ in 0..8 {
        worker.step().unwrap();
    }
    assert_eq!(worker.session.progress_epoch(), 8);
    assert_eq!(input.borrow().calls, 2); // one queued command, not accepted yet
    assert_eq!(output.borrow().calls, 8);
    let frame = Receipt::decode(&output.borrow().bytes).unwrap();
    assert_eq!((frame.baseline, frame.epoch), (0, 1));
    output.borrow_mut().chunk = 40;
    worker.step().unwrap();
    let frame = Receipt::decode(&output.borrow().bytes[40..]).unwrap();
    assert_eq!((frame.baseline, frame.epoch), (8, 9));
    worker.request_stop().unwrap();
}
#[test]
fn receive_failure_closes_hook_before_recovery_and_never_polls_or_writes_again() {
    let (mut worker, input, output, trace) = fixture();
    worker.step().unwrap();
    assert_eq!(test_send(0x1e, false), 1);
    input.borrow_mut().error = true;
    assert_eq!(worker.step().unwrap(), WorkerStep::Stopped);
    assert_eq!(worker.session.progress_epoch(), 1);
    assert_eq!(trace.borrow().packets.len(), 2);
    let report = worker.report.as_ref().unwrap();
    assert!(matches!(report.primary, Some(WorkerFault::Progress(_))));
    assert!(worker.session.shutdown_report().unwrap().unhook.is_ok());
    let reads = input.borrow().calls;
    let writes = output.borrow().calls;
    worker.step().unwrap();
    worker.request_stop().unwrap();
    assert_eq!(input.borrow().calls, reads);
    assert_eq!(output.borrow().calls, writes);
    assert_eq!(worker.session.progress_epoch(), 1);
}
#[test]
fn publish_failure_stops_after_completed_poll_with_no_followup_io() {
    let (mut worker, input, output, _) = fixture();
    output.borrow_mut().error = true;
    assert_eq!(worker.step().unwrap(), WorkerStep::Stopped);
    assert_eq!(worker.session.progress_epoch(), 1);
    let report = worker.report.as_ref().unwrap();
    assert_eq!(report.reason, WorkerStopReason::Fault);
    assert_eq!(report.reply, WorkerReply::Failed);
    assert!(matches!(report.primary, Some(WorkerFault::Progress(_))));
    assert_eq!(worker.session.phase(), ActorPhase::Stopped);
    worker.step().unwrap();
    assert_eq!(input.borrow().calls, 1);
    assert_eq!(output.borrow().calls, 1);
}
#[test]
fn session_primary_and_progress_secondary_are_retained_with_unresolved_ledger() {
    let (mut worker, input, output, trace) = fixture();
    worker.step().unwrap();
    trace.borrow_mut().unknown = true;
    assert_eq!(test_send(0x1e, false), 9);
    input
        .borrow_mut()
        .bytes
        .extend(Challenge { run_id: 7, id: 2 }.encode());
    output.borrow_mut().error = true;
    assert_eq!(worker.step().unwrap(), WorkerStep::Stopped);
    let report = worker.report.as_ref().unwrap();
    assert!(matches!(report.primary, Some(WorkerFault::Session(_))));
    assert!(report.progress.is_some());
    assert_eq!(worker.session.phase(), ActorPhase::Unresolved);
    assert!(worker.session.shutdown_report().unwrap().recovery.is_err());
    assert_eq!(trace.borrow().packets.len(), 1);
    assert_eq!(worker.session.progress_epoch(), 2);
    worker.step().unwrap();
    assert_eq!(trace.borrow().packets.len(), 1);
}
#[test]
fn quit_can_leave_frozen_active_frame_pending_without_claiming_terminal_ack() {
    let (mut worker, input, output, _) = fixture();
    output.borrow_mut().chunk = 5;
    worker.step().unwrap();
    test_quit();
    assert_eq!(worker.step().unwrap(), WorkerStep::Stopped);
    assert_eq!(worker.session.progress_epoch(), 2);
    let report = worker.report.as_ref().unwrap();
    assert_eq!(report.reason, WorkerStopReason::Quit);
    assert_eq!(report.reply, WorkerReply::FramePending);
    assert_eq!(output.borrow().bytes.len(), 10);
    assert_eq!(output.borrow().bytes[5], 1); // frozen Active
    assert_eq!(input.borrow().calls, 2); // stop-priority receive before this poll
    assert_eq!(output.borrow().calls, 2);
    worker.step().unwrap();
    assert_eq!(output.borrow().calls, 2);
}
#[test]
fn callbacks_and_manual_stop_never_mint_epoch_or_replace_first_report() {
    let (mut worker, input, output, _) = fixture();
    with_callback_borrow(|| {
        assert_eq!(worker.step().unwrap_err(), WorkerAccessError::InCallback);
        assert_eq!(
            worker.request_stop().unwrap_err(),
            WorkerAccessError::InCallback
        );
    });
    assert_eq!(input.borrow().calls, 0);
    assert_eq!(output.borrow().calls, 0);
    assert!(worker.session.shutdown_report().is_none());
    worker.request_stop().unwrap();
    assert_eq!(worker.session.progress_epoch(), 0);
    assert_eq!(
        worker.report.as_ref().unwrap().reason,
        WorkerStopReason::Requested
    );
    with_callback_borrow(|| {
        assert_eq!(worker.step().unwrap_err(), WorkerAccessError::InCallback);
    });
    worker.step().unwrap();
    assert_eq!(
        worker.report.as_ref().unwrap().reason,
        WorkerStopReason::Requested
    );
}
#[test]
fn guarded_factory_installs_passively_and_cached_run_retains_both_reports() {
    let startup = WorkerStartup::claim(76, 7).unwrap();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut worker = Worker::install(
        startup,
        Backend(trace.clone()),
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
        config(),
    )
    .unwrap();
    assert_eq!(worker.phase(), ActorPhase::Standby);
    assert_eq!(worker.progress_epoch(), 0);
    assert!(worker.report().is_none());
    with_callback_borrow(|| {
        assert_eq!(worker.step().unwrap_err(), WorkerAccessError::InCallback);
    });
    worker.request_stop().unwrap();
    assert_eq!(worker.run().unwrap().reason, WorkerStopReason::Requested);
    assert!(worker.shutdown_report().unwrap().unhook.is_ok());
    assert!(worker.shutdown_report().unwrap().recovery.is_ok());
    assert!(trace.borrow().packets.is_empty());
    assert_eq!(worker.phase(), ActorPhase::Stopped);
}
#[test]
fn install_failure_preserves_probe_error_and_cannot_reclaim_startup() {
    let startup = WorkerStartup::claim(76, 7).unwrap();
    test_probe(0, false);
    let trace = Rc::new(RefCell::new(Trace::default()));
    assert!(matches!(
        Worker::install(
            startup,
            Backend(trace.clone()),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::CanonicalScan,
            config()
        ),
        Err(WorkerInstallError::Session(SessionError::Probe(_)))
    ));
    assert!(trace.borrow().packets.is_empty());
    assert!(matches!(
        WorkerStartup::claim(76, 7),
        Err(super::super::worker_startup::WorkerStartupError::AlreadyClaimed)
    ));
}

#[test]
fn cooperative_stop_in_standby_prevents_any_actual_poll_or_output() {
    let (mut worker, input, output, trace) = fixture();
    input.borrow_mut().bytes = keyweave_progress::StopRequest { run_id: 7, id: 1 }
        .encode()
        .into();
    assert_eq!(worker.step().unwrap(), WorkerStep::Stopped);
    assert_eq!(worker.session.progress_epoch(), 0);
    assert_eq!(worker.session.phase(), ActorPhase::Stopped);
    assert_eq!(
        worker.report.as_ref().unwrap().reason,
        WorkerStopReason::Requested
    );
    assert!(trace.borrow().packets.is_empty());
    assert_eq!(output.borrow().calls, 0);
    let reads = input.borrow().calls;
    worker.step().unwrap();
    assert_eq!(input.borrow().calls, reads);
}
#[test]
fn cooperative_stop_preempts_partial_reply_and_closes_capture_before_known_up() {
    let (mut worker, input, output, trace) = fixture();
    output.borrow_mut().chunk = 5;
    worker.step().unwrap();
    assert_eq!(test_send(0x1e, false), 1);
    input
        .borrow_mut()
        .bytes
        .extend(keyweave_progress::StopRequest { run_id: 7, id: 2 }.encode());
    assert_eq!(worker.step().unwrap(), WorkerStep::Stopped);
    assert_eq!(worker.session.progress_epoch(), 1);
    let report = worker.report.as_ref().unwrap();
    assert_eq!(report.reason, WorkerStopReason::Requested);
    assert_eq!(report.reply, WorkerReply::FramePending);
    assert!(report.primary.is_none() && report.progress.is_none());
    assert_eq!(output.borrow().bytes.len(), 5);
    assert_eq!(trace.borrow().packets.len(), 2);
    assert_eq!(trace.borrow().packets[1][0].flags & 2, 2);
    assert!(worker.session.shutdown_report().unwrap().recovery.is_ok());
    worker.step().unwrap();
    assert_eq!(output.borrow().calls, 1);
}
#[test]
fn wrong_stop_identity_is_fault_and_does_not_poll_or_publish() {
    let (mut worker, input, output, _) = fixture();
    input.borrow_mut().bytes = keyweave_progress::StopRequest { run_id: 8, id: 1 }
        .encode()
        .into();
    assert_eq!(worker.step().unwrap(), WorkerStep::Stopped);
    assert_eq!(
        worker.report.as_ref().unwrap().reason,
        WorkerStopReason::Fault
    );
    assert_eq!(worker.session.progress_epoch(), 0);
    assert_eq!(output.borrow().calls, 0);
}
