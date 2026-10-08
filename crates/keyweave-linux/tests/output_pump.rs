#![allow(clippy::unwrap_used)]
use keyweave_linux::{
    events::{FrameWrite, NativeKey},
    output::{DeviceSpec, RepeatPolicy, WriteError},
    pump::*,
};
use keyweave_output::Mode;
use keyweave_types::*;
use std::{cell::RefCell, collections::VecDeque, io, rc::Rc};
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Submit(Vec<NativeKey>, bool),
    Flush,
    Destroy,
}
struct Backend {
    spec: DeviceSpec,
    receipts: VecDeque<io::Result<Result<FrameWrite, WriteError>>>,
    reports: VecDeque<Result<bool, WriteError>>,
    calls: Rc<RefCell<Vec<Call>>>,
}
impl FrameBackend for Backend {
    fn spec(&self) -> &DeviceSpec {
        &self.spec
    }
    fn submit(
        &mut self,
        edges: &[NativeKey],
        recovery: bool,
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        self.calls
            .borrow_mut()
            .push(Call::Submit(edges.to_vec(), recovery));
        self.receipts.pop_front().unwrap()
    }
    fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        self.calls.borrow_mut().push(Call::Flush);
        Ok(self.reports.pop_front().unwrap())
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.calls.borrow_mut().push(Call::Destroy);
    }
}
fn make(
    receipts: Vec<io::Result<Result<FrameWrite, WriteError>>>,
    reports: Vec<Result<bool, WriteError>>,
) -> (OutputPump<Backend>, Rc<RefCell<Vec<Call>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let backend = Backend {
        spec: DeviceSpec::new("mock", &[30, 48], RepeatPolicy::Kernel, false).unwrap(),
        receipts: receipts.into(),
        reports: reports.into(),
        calls: calls.clone(),
    };
    let mut pump = OutputPump::new(backend);
    pump.resume().unwrap();
    (pump, calls)
}
fn edge(name: &str, kind: KeyEdge, at: u64) -> OutputIntent {
    OutputIntent {
        key: name.parse().unwrap(),
        edge: kind,
        at_ns: MonoTime(at),
    }
}
fn complete(count: usize) -> io::Result<Result<FrameWrite, WriteError>> {
    Ok(Ok(FrameWrite::Complete { accepted: count }))
}
fn incomplete(count: usize) -> io::Result<Result<FrameWrite, WriteError>> {
    Ok(Ok(FrameWrite::Incomplete {
        accepted: count,
        report_pending: true,
    }))
}
#[test]
fn full_key_prefix_needs_report_before_releasing_saved_down() {
    let (mut pump, calls) = make(vec![incomplete(1), complete(1)], vec![Ok(false), Ok(true)]);
    pump.enqueue(&[edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Up, 100)])
        .unwrap();
    assert_eq!(
        pump.poll(MonoTime(0)).unwrap(),
        PumpStep::ReportPending { accepted: 1 }
    );
    assert_eq!(pump.phase(), Phase::ReportPending);
    assert_eq!(pump.ledger().submitted().len(), 1);
    assert!(pump.resume().is_err());
    assert!(pump.enqueue(&[edge("b", KeyEdge::Down, 1)]).is_err());
    assert_eq!(pump.poll(MonoTime(1)).unwrap(), PumpStep::Idle);
    assert_eq!(pump.poll(MonoTime(2)).unwrap(), PumpStep::ReportFlushed);
    assert_eq!(
        pump.poll(MonoTime(3)).unwrap(),
        PumpStep::Submitted { accepted: 1 }
    );
    assert_eq!(
        *calls.borrow(),
        vec![
            Call::Submit(vec![NativeKey { code: 30, value: 1 }], false),
            Call::Flush,
            Call::Flush,
            Call::Submit(vec![NativeKey { code: 30, value: 0 }], true)
        ]
    );
    assert_eq!(pump.ledger().mode(), Mode::Paused);
    assert!(pump.ledger().submitted().is_empty());
    assert!(pump.resume().is_err());
    pump.destroy_backend().unwrap();
    assert_eq!(calls.borrow().last(), Some(&Call::Destroy));
    assert_eq!(pump.phase(), Phase::Detached);
}
#[test]
fn neutral_key_prefix_still_cannot_skip_missing_report() {
    let (mut pump, calls) = make(vec![incomplete(2)], vec![Ok(true)]);
    pump.enqueue(&[
        edge("a", KeyEdge::Down, 0),
        edge("a", KeyEdge::Up, 0),
        edge("b", KeyEdge::Down, 10),
    ])
    .unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert!(pump.ledger().submitted().is_empty());
    assert_eq!(pump.ledger().queued_edges(), 0);
    assert!(pump.resume().is_err());
    assert_eq!(pump.poll(MonoTime(1)).unwrap(), PumpStep::ReportFlushed);
    assert_eq!(pump.poll(MonoTime(10)).unwrap(), PumpStep::Idle);
    assert_eq!(calls.borrow().len(), 2);
}
#[test]
fn unknown_acceptance_is_not_acknowledged_as_zero() {
    let (mut pump, calls) = make(
        vec![
            complete(1),
            Ok(Err(WriteError::UnknownAcceptance(
                io::Error::from_raw_os_error(14),
            ))),
        ],
        vec![],
    );
    pump.enqueue(&[edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Up, 1)])
        .unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert!(pump.poll(MonoTime(1)).is_err());
    assert_eq!(pump.phase(), Phase::Quarantined);
    assert_eq!(pump.ledger().mode(), Mode::Quarantined);
    assert_eq!(pump.ledger().submitted().len(), 1);
    assert_eq!(pump.ledger().queued_edges(), 1);
    assert!(pump.poll(MonoTime(2)).is_err());
    assert_eq!(calls.borrow().len(), 2);
    pump.destroy_backend().unwrap();
    assert!(pump.ledger().submitted().is_empty());
    assert_eq!(calls.borrow().last(), Some(&Call::Destroy));
}
#[test]
fn report_error_with_empty_ledger_still_quarantines() {
    let (mut pump, _) = make(
        vec![incomplete(0)],
        vec![Err(WriteError::UnknownAcceptance(io::Error::other(
            "fault",
        )))],
    );
    pump.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert!(pump.poll(MonoTime(1)).is_err());
    assert_eq!(pump.phase(), Phase::Quarantined);
    assert!(pump.resume().is_err());
}
#[test]
fn admission_failure_is_atomic_and_does_not_touch_backend() {
    let (mut pump, calls) = make(vec![], vec![]);
    for edges in [
        vec![edge("c", KeyEdge::Down, 0)],
        vec![edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Repeat, 0)],
        vec![OutputIntent {
            key: KeyId::new(7, 0x32),
            edge: KeyEdge::Down,
            at_ns: MonoTime(0),
        }],
    ] {
        assert!(pump.enqueue(&edges).is_err());
        assert!(pump.ledger().desired().is_empty());
        assert_eq!(pump.ledger().queued_edges(), 0);
    }
    assert!(calls.borrow().is_empty());
}
#[test]
fn invalid_receipt_does_not_invent_known_prefix() {
    let (mut pump, _) = make(vec![complete(2)], vec![]);
    pump.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    assert!(matches!(
        pump.poll(MonoTime(0)),
        Err(PumpError::InvalidReceipt)
    ));
    assert_eq!(pump.phase(), Phase::Quarantined);
    assert!(pump.ledger().submitted().is_empty());
    assert_eq!(pump.ledger().queued_edges(), 1);
}
#[test]
fn prewrite_rejection_preserves_prior_down_for_cleanup() {
    let (mut pump, _) = make(
        vec![
            complete(1),
            Err(io::Error::new(io::ErrorKind::InvalidInput, "preflight")),
            complete(1),
        ],
        vec![],
    );
    pump.enqueue(&[edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Up, 1)])
        .unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert!(pump.poll(MonoTime(1)).is_err());
    assert_eq!(pump.phase(), Phase::Recovery);
    assert_eq!(pump.ledger().submitted().len(), 1);
    pump.poll(MonoTime(2)).unwrap();
    assert_eq!(pump.ledger().mode(), Mode::Paused);
}
#[test]
fn orderly_recovery_discards_future_and_releases_existing_down() {
    let (mut pump, calls) = make(vec![complete(1), complete(1)], vec![]);
    pump.enqueue(&[edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Up, 100)])
        .unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.begin_recovery(MonoTime(5)).unwrap();
    pump.poll(MonoTime(5)).unwrap();
    assert_eq!(
        calls.borrow()[1],
        Call::Submit(vec![NativeKey { code: 30, value: 0 }], true)
    );
    assert!(pump.resume().is_err());
}
#[test]
fn report_flush_checks_monotonic_time_before_io() {
    let (mut pump, calls) = make(vec![incomplete(1)], vec![Ok(true)]);
    pump.enqueue(&[edge("a", KeyEdge::Down, 10)]).unwrap();
    pump.poll(MonoTime(10)).unwrap();
    assert!(pump.poll(MonoTime(9)).is_err());
    assert_eq!(calls.borrow().len(), 1);
    pump.poll(MonoTime(11)).unwrap();
    assert!(pump.begin_recovery(MonoTime(10)).is_err());
}

#[test]
fn engine_hot_update_keeps_old_release_through_native_pump() {
    use keyweave_engine::{ConfigSnapshot, Engine, Mapping};
    use std::sync::Arc;
    fn config(generation: u64, to: &str) -> Arc<ConfigSnapshot> {
        Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(generation),
            name: "pump integration".into(),
            complex: vec![],
            simple: vec![Mapping {
                id: "map".into(),
                from: "a".parse().unwrap(),
                to: vec![to.parse::<KeyId>().unwrap().into()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            }],
        })
    }
    let mut engine = Engine::new(config(1, "b")).unwrap();
    let (mut pump, calls) = make(vec![complete(1), complete(1)], vec![]);
    let down = engine
        .handle(InputEnvelope {
            seq: 1,
            at_ns: MonoTime(0),
            device: DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: "a".parse().unwrap(),
                edge: KeyEdge::Down,
            },
        })
        .unwrap();
    pump.enqueue(&down.outputs).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    engine.activate(config(2, "a")).unwrap();
    let up = engine
        .handle(InputEnvelope {
            seq: 2,
            at_ns: MonoTime(1),
            device: DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: "a".parse().unwrap(),
                edge: KeyEdge::Up,
            },
        })
        .unwrap();
    pump.enqueue(&up.outputs).unwrap();
    pump.poll(MonoTime(1)).unwrap();
    assert_eq!(
        *calls.borrow(),
        vec![
            Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
            Call::Submit(vec![NativeKey { code: 48, value: 0 }], false)
        ]
    );
    assert!(engine.state().pressed_outputs.is_empty());
    assert!(pump.ledger().submitted().is_empty());
}
