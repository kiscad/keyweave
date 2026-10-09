#![allow(clippy::unwrap_used)]
use super::*;
use keyweave_types::{KEYBOARD_PAGE, KeyEdge, KeyId};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
#[derive(Default)]
struct Trace {
    calls: Vec<Vec<KeyboardPacket>>,
    drops: usize,
}
struct Backend {
    replies: VecDeque<Result<Insertion, SubmitError>>,
    trace: Rc<RefCell<Trace>>,
}
impl SendBackend for Backend {
    fn submit(&mut self, packets: &[KeyboardPacket]) -> Result<Insertion, SubmitError> {
        self.trace.borrow_mut().calls.push(packets.to_vec());
        self.replies.pop_front().unwrap_or(Ok(Insertion {
            accepted: packets.len(),
            last_error: None,
        }))
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.trace.borrow_mut().drops += 1;
    }
}
fn edge(usage: u16, kind: KeyEdge, at: u64) -> OutputIntent {
    OutputIntent {
        at_ns: MonoTime(at),
        key: KeyId::new(KEYBOARD_PAGE, usage),
        edge: kind,
    }
}
fn pump(replies: Vec<Result<Insertion, SubmitError>>) -> (OutputPump<Backend>, Rc<RefCell<Trace>>) {
    let trace = Rc::new(RefCell::new(Trace::default()));
    let backend = Backend {
        replies: replies.into(),
        trace: trace.clone(),
    };
    (
        OutputPump::new(backend, InjectionTag::new(1).unwrap()),
        trace,
    )
}
fn short(count: usize) -> Result<Insertion, SubmitError> {
    Ok(Insertion {
        accepted: count,
        last_error: Some(0),
    })
}

#[test]
fn due_output_and_repeat_confirm_only_api_accepted_state() {
    let (mut pump, trace) = pump(vec![]);
    assert!(pump.enqueue(&[edge(4, KeyEdge::Down, 5)]).is_err());
    pump.resume().unwrap();
    pump.enqueue(&[
        edge(4, KeyEdge::Down, 5),
        edge(4, KeyEdge::Repeat, 7),
        edge(4, KeyEdge::Up, 9),
    ])
    .unwrap();
    assert_eq!(pump.poll(MonoTime(4)).unwrap(), PumpStep::Idle);
    pump.poll(MonoTime(5)).unwrap();
    assert_eq!(pump.ledger().submitted().len(), 1);
    pump.poll(MonoTime(7)).unwrap();
    assert_eq!(pump.ledger().submitted().len(), 1);
    pump.poll(MonoTime(9)).unwrap();
    assert!(pump.ledger().submitted().is_empty());
    let calls = &trace.borrow().calls;
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0], calls[1]);
    assert_eq!(calls[2][0].flags, 10);
}

#[test]
fn short_prefix_requires_stop_and_recovers_nonmodifiers_before_modifiers() {
    let (mut pump, trace) = pump(vec![short(2)]);
    pump.resume().unwrap();
    pump.enqueue(&[
        edge(0xe0, KeyEdge::Down, 0),
        edge(4, KeyEdge::Down, 0),
        edge(5, KeyEdge::Down, 0),
    ])
    .unwrap();
    assert_eq!(
        pump.poll(MonoTime(0)).unwrap(),
        PumpStep::Inserted(Insertion {
            accepted: 2,
            last_error: Some(0)
        })
    );
    assert_eq!(pump.phase(), Phase::StopRequired);
    assert_eq!(pump.ledger().submitted().len(), 2);
    assert!(pump.ledger().desired().is_empty());
    assert!(matches!(pump.poll(MonoTime(0)), Err(PumpError::NotReady)));
    assert!(pump.resume().is_err());
    pump.begin_recovery(MonoTime(1)).unwrap();
    pump.poll(MonoTime(1)).unwrap();
    assert_eq!(pump.phase(), Phase::Paused);
    assert!(pump.ledger().submitted().is_empty());
    let trace = trace.borrow();
    assert_eq!(trace.calls.len(), 2);
    assert_eq!(
        trace.calls[1]
            .iter()
            .map(|p| (p.scan, p.flags))
            .collect::<Vec<_>>(),
        vec![(0x1e, 10), (0x1d, 10)]
    );
}

#[test]
fn short_recovery_retries_only_remaining_up() {
    let (mut pump, trace) = pump(vec![short(2), short(1)]);
    pump.resume().unwrap();
    pump.enqueue(&[
        edge(0xe4, KeyEdge::Down, 0),
        edge(4, KeyEdge::Down, 0),
        edge(5, KeyEdge::Down, 0),
    ])
    .unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.begin_recovery(MonoTime(1)).unwrap();
    pump.poll(MonoTime(1)).unwrap();
    assert_eq!(pump.phase(), Phase::Recovery);
    assert_eq!(
        pump.ledger()
            .submitted()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![KeyId::new(KEYBOARD_PAGE, 0xe4)]
    );
    pump.poll(MonoTime(2)).unwrap();
    assert_eq!(pump.phase(), Phase::Paused);
    let trace = trace.borrow();
    assert_eq!(trace.calls[2].len(), 1);
    assert_eq!(
        (trace.calls[2][0].scan, trace.calls[2][0].flags),
        (0x1d, 11)
    );
    assert!(
        trace.calls[1]
            .iter()
            .chain(&trace.calls[2])
            .all(|packet| packet.flags & 2 != 0)
    );
}

#[test]
fn zero_return_cannot_resume_until_stop_gate() {
    let (mut pump, trace) = pump(vec![short(0)]);
    pump.resume().unwrap();
    pump.enqueue(&[edge(4, KeyEdge::Down, 0)]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert_eq!(pump.phase(), Phase::StopRequired);
    assert!(pump.ledger().submitted().is_empty());
    assert!(pump.resume().is_err());
    pump.begin_recovery(MonoTime(0)).unwrap();
    assert_eq!(pump.phase(), Phase::Paused);
    pump.resume().unwrap();
    assert_eq!(trace.borrow().calls.len(), 1);
}

#[test]
fn preinvoke_rejection_preserves_previous_submitted_for_recovery() {
    let (mut pump, trace) = pump(vec![short(1), Err(SubmitError::Rejected)]);
    pump.resume().unwrap();
    pump.enqueue(&[edge(4, KeyEdge::Down, 0)]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.enqueue(&[edge(5, KeyEdge::Down, 1)]).unwrap();
    assert!(matches!(
        pump.poll(MonoTime(1)),
        Err(PumpError::Submit(SubmitError::Rejected))
    ));
    assert_eq!(pump.phase(), Phase::StopRequired);
    assert_eq!(pump.ledger().submitted().len(), 1);
    pump.begin_recovery(MonoTime(1)).unwrap();
    pump.poll(MonoTime(1)).unwrap();
    assert_eq!(trace.borrow().calls[2].len(), 1);
    assert_eq!(trace.borrow().calls[2][0].scan, 0x1e);
}

#[test]
fn unknown_or_invalid_receipt_freezes_and_close_does_not_clear_global_state() {
    for result in [Err(SubmitError::Unknown), short(2)] {
        let (mut pump, trace) = pump(vec![short(1), result]);
        pump.resume().unwrap();
        pump.enqueue(&[edge(4, KeyEdge::Down, 0)]).unwrap();
        pump.poll(MonoTime(0)).unwrap();
        pump.enqueue(&[edge(5, KeyEdge::Down, 1)]).unwrap();
        assert!(pump.poll(MonoTime(1)).is_err());
        assert_eq!(pump.phase(), Phase::Quarantined);
        assert_eq!(pump.ledger().submitted().len(), 1);
        assert!(pump.begin_recovery(MonoTime(1)).is_err());
        assert!(pump.poll(MonoTime(2)).is_err());
        pump.close();
        assert_eq!(pump.phase(), Phase::Closed);
        assert_eq!(pump.ledger().mode(), Mode::Quarantined);
        assert_eq!(pump.ledger().submitted().len(), 1);
        assert_eq!(pump.ledger().desired().len(), 2);
        assert_eq!(trace.borrow().calls.len(), 2);
        assert_eq!(trace.borrow().drops, 1);
        pump.close();
        assert_eq!(trace.borrow().drops, 1);
    }
}

#[test]
fn native_validation_is_atomic_even_after_multiple_chunks() {
    let (mut pump, trace) = pump(vec![]);
    pump.resume().unwrap();
    let mut edges = vec![edge(4, KeyEdge::Down, 0)];
    edges.extend(std::iter::repeat_n(
        edge(4, KeyEdge::Repeat, 0),
        keys::MAX_BATCH,
    ));
    edges.push(edge(0x32, KeyEdge::Down, 0));
    assert!(matches!(
        pump.enqueue(&edges),
        Err(PumpError::Key(KeyError::Ambiguous))
    ));
    assert!(pump.ledger().desired().is_empty());
    assert_eq!(pump.ledger().queued_edges(), 0);
    assert!(trace.borrow().calls.is_empty());
    assert!(matches!(
        pump.enqueue(&vec![
            edge(4, KeyEdge::Down, 0);
            keyweave_output::MAX_ENQUEUE_EDGES + 1
        ]),
        Err(PumpError::Queue(OutputError::Capacity))
    ));
}

#[test]
fn time_reversal_and_drop_never_emit_hidden_cleanup() {
    let (mut pump, trace) = pump(vec![]);
    pump.resume().unwrap();
    pump.enqueue(&[edge(4, KeyEdge::Down, 5)]).unwrap();
    pump.poll(MonoTime(5)).unwrap();
    assert!(matches!(
        pump.begin_recovery(MonoTime(4)),
        Err(PumpError::Queue(OutputError::TimeReversed))
    ));
    assert_eq!(pump.phase(), Phase::Normal);
    assert!(matches!(
        pump.poll(MonoTime(4)),
        Err(PumpError::Queue(OutputError::TimeReversed))
    ));
    drop(pump);
    assert_eq!(trace.borrow().drops, 1);
    assert_eq!(trace.borrow().calls.len(), 1);
}
