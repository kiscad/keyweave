#![allow(clippy::unwrap_used)]
use keyweave_linux::{
    events::{Event, FrameWrite, NativeKey},
    output::{DeviceSpec, RepeatPolicy, WriteError},
    pump::*,
    relative::*,
};
use keyweave_types::*;
use std::{cell::RefCell, collections::VecDeque, io, rc::Rc};
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Keys(Vec<NativeKey>, bool),
    Relative(Vec<Event>),
    Flush,
    Destroy,
}
type Reply = io::Result<Result<FrameWrite, WriteError>>;
struct Backend {
    spec: DeviceSpec,
    axes: Option<RelativeAxes>,
    calls: Rc<RefCell<Vec<Call>>>,
    relative_replies: VecDeque<Reply>,
    reports: VecDeque<bool>,
}
impl FrameBackend for Backend {
    fn spec(&self) -> &DeviceSpec {
        &self.spec
    }
    fn relative_axes(&self) -> Option<&RelativeAxes> {
        self.axes.as_ref()
    }
    fn submit(&mut self, edges: &[NativeKey], recovery: bool) -> Reply {
        self.calls
            .borrow_mut()
            .push(Call::Keys(edges.to_vec(), recovery));
        Ok(Ok(FrameWrite::Complete {
            accepted: edges.len(),
        }))
    }
    fn submit_relative(&mut self, frame: &RelativeFrame) -> Reply {
        self.calls
            .borrow_mut()
            .push(Call::Relative(frame.records().to_vec()));
        self.relative_replies
            .pop_front()
            .unwrap_or(Ok(Ok(FrameWrite::Complete {
                accepted: frame.records().len(),
            })))
    }
    fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        self.calls.borrow_mut().push(Call::Flush);
        Ok(Ok(self.reports.pop_front().unwrap_or(true)))
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.calls.borrow_mut().push(Call::Destroy);
    }
}
fn make(
    axes: Option<RelativeAxes>,
    replies: Vec<Reply>,
    reports: Vec<bool>,
) -> (OutputPump<Backend>, Rc<RefCell<Vec<Call>>>) {
    let calls = Rc::new(RefCell::new(vec![]));
    let mut pump = OutputPump::new(Backend {
        spec: DeviceSpec::new("mixed mock", &[30, 48], RepeatPolicy::Kernel, false).unwrap(),
        axes,
        calls: calls.clone(),
        relative_replies: replies.into(),
        reports: reports.into(),
    });
    pump.resume().unwrap();
    (pump, calls)
}
fn axes() -> RelativeAxes {
    RelativeAxes::new(&[REL_X, REL_Y]).unwrap()
}
fn frame() -> RelativeFrame {
    RelativeFrame::prepare(
        &axes(),
        &[
            Event::output(EV_REL, REL_X, 1),
            Event::output(EV_REL, REL_Y, -2),
        ],
    )
    .unwrap()
}
fn edge(name: &str, edge: KeyEdge, at: u64) -> OutputIntent {
    OutputIntent {
        key: name.parse().unwrap(),
        edge,
        at_ns: MonoTime(at),
    }
}
fn keycalls(calls: &Rc<RefCell<Vec<Call>>>) -> Vec<(Vec<NativeKey>, bool)> {
    calls
        .borrow()
        .iter()
        .filter_map(|call| {
            if let Call::Keys(keys, recovery) = call {
                Some((keys.clone(), *recovery))
            } else {
                None
            }
        })
        .collect()
}
#[test]
fn mixed_fifo_submits_keys_relative_report_then_later_keys_at_same_time() {
    let (mut pump, calls) = make(Some(axes()), vec![], vec![]);
    pump.enqueue_mixed(&[edge("a", KeyEdge::Down, 0)], &[(MonoTime(0), frame())])
        .unwrap();
    pump.enqueue(&[edge("a", KeyEdge::Up, 0)]).unwrap();
    assert_eq!(
        pump.poll(MonoTime(0)).unwrap(),
        PumpStep::Submitted { accepted: 1 }
    );
    assert!(matches!(calls.borrow()[0], Call::Keys(_, false)));
    assert_eq!(pump.queued_relative_events(), 2);
    assert_eq!(
        pump.poll(MonoTime(0)).unwrap(),
        PumpStep::Submitted { accepted: 2 }
    );
    assert!(matches!(calls.borrow()[1], Call::Relative(_)));
    pump.poll(MonoTime(0)).unwrap();
    assert_eq!(
        keycalls(&calls),
        [
            (vec![NativeKey { code: 30, value: 1 }], false),
            (vec![NativeKey { code: 30, value: 0 }], false)
        ]
    );
    assert!(pump.ledger().submitted().is_empty());
    assert_eq!(pump.next_deadline(), None);
}
#[test]
fn large_key_span_splits_at_native_limit_and_never_crosses_relative_barrier() {
    let (mut pump, calls) = make(Some(axes()), vec![], vec![]);
    let keys: Vec<_> = (0..300)
        .map(|index| {
            edge(
                "a",
                if index % 2 == 0 {
                    KeyEdge::Down
                } else {
                    KeyEdge::Up
                },
                0,
            )
        })
        .collect();
    pump.enqueue_mixed(&keys, &[(MonoTime(0), frame())])
        .unwrap();
    pump.enqueue(&[edge("b", KeyEdge::Down, 0)]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert_eq!(
        keycalls(&calls)
            .iter()
            .map(|(keys, _)| keys.len())
            .collect::<Vec<_>>(),
        [256, 44]
    );
    pump.poll(MonoTime(0)).unwrap();
    assert!(matches!(calls.borrow()[2], Call::Relative(_)));
    pump.poll(MonoTime(0)).unwrap();
    assert_eq!(
        keycalls(&calls).last().unwrap().0,
        vec![NativeKey { code: 48, value: 1 }]
    );
}
#[test]
fn mixed_time_normalization_preserves_future_relative_before_earlier_key_timestamp() {
    let (mut pump, calls) = make(Some(axes()), vec![], vec![]);
    pump.enqueue_mixed(&[edge("a", KeyEdge::Down, 0)], &[(MonoTime(100), frame())])
        .unwrap();
    pump.enqueue(&[edge("a", KeyEdge::Up, 50)]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert_eq!(pump.next_deadline(), Some(MonoTime(100)));
    assert_eq!(pump.poll(MonoTime(99)).unwrap(), PumpStep::Idle);
    assert_eq!(calls.borrow().len(), 1);
    pump.poll(MonoTime(100)).unwrap();
    pump.poll(MonoTime(100)).unwrap();
    assert!(matches!(calls.borrow()[1], Call::Relative(_)));
    assert_eq!(pump.next_deadline(), None);
}
#[test]
fn relative_short_write_flushes_report_before_recovery_and_never_replays_motion() {
    for accepted in [0, 1, 2] {
        let (mut pump, calls) = make(
            Some(axes()),
            vec![Ok(Ok(FrameWrite::Incomplete {
                accepted,
                report_pending: true,
            }))],
            vec![false, true],
        );
        pump.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
        pump.poll(MonoTime(0)).unwrap();
        pump.enqueue_mixed(&[], &[(MonoTime(10), frame())]).unwrap();
        pump.enqueue(&[edge("b", KeyEdge::Down, 20)]).unwrap();
        assert_eq!(
            pump.poll(MonoTime(10)).unwrap(),
            PumpStep::ReportPending { accepted }
        );
        assert_eq!(pump.phase(), Phase::ReportPending);
        assert_eq!(pump.queued_relative_events(), 0);
        assert_eq!(pump.poll(MonoTime(10)).unwrap(), PumpStep::Idle);
        assert_eq!(pump.poll(MonoTime(10)).unwrap(), PumpStep::ReportFlushed);
        pump.poll(MonoTime(10)).unwrap();
        assert_eq!(
            keycalls(&calls),
            [
                (vec![NativeKey { code: 30, value: 1 }], false),
                (vec![NativeKey { code: 30, value: 0 }], true)
            ]
        );
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|c| matches!(c, Call::Relative(_)))
                .count(),
            1
        );
        assert!(pump.ledger().submitted().is_empty());
    }
}
#[test]
fn relative_report_pending_is_not_complete_even_with_empty_key_ledger() {
    let (mut pump, calls) = make(
        Some(axes()),
        vec![Ok(Ok(FrameWrite::Incomplete {
            accepted: 2,
            report_pending: true,
        }))],
        vec![false, true],
    );
    pump.enqueue_mixed(&[], &[(MonoTime(0), frame())]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert!(pump.ledger().submitted().is_empty());
    assert_eq!(pump.phase(), Phase::ReportPending);
    assert_eq!(pump.next_deadline(), Some(MonoTime(0)));
    pump.poll(MonoTime(0)).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert_eq!(pump.phase(), Phase::Recovery);
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|c| matches!(c, Call::Flush))
            .count(),
        2
    );
}
#[test]
fn unknown_relative_acceptance_quarantines_until_unique_backend_destruction() {
    let (mut pump, calls) = make(
        Some(axes()),
        vec![Ok(Err(WriteError::UnknownAcceptance(
            io::ErrorKind::WouldBlock.into(),
        )))],
        vec![],
    );
    pump.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.enqueue_mixed(&[], &[(MonoTime(1), frame())]).unwrap();
    assert!(pump.poll(MonoTime(1)).is_err());
    assert_eq!(pump.phase(), Phase::Quarantined);
    assert!(!pump.ledger().submitted().is_empty());
    assert!(pump.begin_recovery(MonoTime(1)).is_err());
    pump.destroy_backend().unwrap();
    assert_eq!(calls.borrow().last(), Some(&Call::Destroy));
    assert!(pump.ledger().submitted().is_empty());
    assert_eq!(pump.phase(), Phase::Detached);
}
#[test]
fn invalid_relative_receipts_never_invent_complete_or_zero_acceptance() {
    for reply in [
        FrameWrite::Complete { accepted: 1 },
        FrameWrite::Complete { accepted: 3 },
        FrameWrite::Incomplete {
            accepted: 1,
            report_pending: false,
        },
        FrameWrite::Incomplete {
            accepted: 3,
            report_pending: true,
        },
    ] {
        let (mut pump, _) = make(Some(axes()), vec![Ok(Ok(reply))], vec![]);
        pump.enqueue_mixed(&[], &[(MonoTime(0), frame())]).unwrap();
        assert!(matches!(
            pump.poll(MonoTime(0)),
            Err(PumpError::InvalidReceipt)
        ));
        assert_eq!(pump.phase(), Phase::Quarantined);
    }
}
#[test]
fn known_prewrite_relative_rejection_recovers_only_prior_accepted_keys() {
    let (mut pump, calls) = make(
        Some(axes()),
        vec![Err(io::ErrorKind::PermissionDenied.into())],
        vec![],
    );
    pump.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.enqueue_mixed(&[], &[(MonoTime(1), frame())]).unwrap();
    assert!(matches!(
        pump.poll(MonoTime(1)),
        Err(PumpError::Rejected(_))
    ));
    assert_eq!(pump.phase(), Phase::Recovery);
    pump.poll(MonoTime(1)).unwrap();
    assert_eq!(
        keycalls(&calls).last().unwrap(),
        &(vec![NativeKey { code: 30, value: 0 }], true)
    );
}
#[test]
fn mixed_cold_capability_and_capacity_failure_leave_desired_and_queue_unchanged() {
    for supported in [None, Some(RelativeAxes::new(&[REL_X]).unwrap())] {
        let (mut pump, calls) = make(supported, vec![], vec![]);
        assert!(
            pump.enqueue_mixed(&[edge("a", KeyEdge::Down, 0)], &[(MonoTime(0), frame())])
                .is_err()
        );
        assert!(pump.ledger().desired().is_empty());
        assert_eq!(pump.ledger().queued_edges(), 0);
        assert_eq!(pump.queued_relative_events(), 0);
        assert!(calls.borrow().is_empty());
    }
    let (mut pump, _) = make(Some(axes()), vec![], vec![]);
    let big = RelativeFrame::prepare(&axes(), &vec![Event::output(EV_REL, REL_X, 1); 256]).unwrap();
    let frames = vec![(MonoTime(0), big); 32];
    pump.enqueue_mixed(&[], &frames).unwrap();
    assert_eq!(pump.queued_relative_events(), 8192);
    assert!(pump.enqueue(&[edge("a", KeyEdge::Down, 0)]).is_err());
    assert!(pump.ledger().desired().is_empty());
    assert_eq!(pump.queued_relative_events(), 8192);
}
#[test]
fn user_stop_discards_future_motion_and_restores_only_known_key_downs() {
    let (mut pump, calls) = make(Some(axes()), vec![], vec![]);
    pump.enqueue_mixed(&[edge("a", KeyEdge::Down, 0)], &[(MonoTime(100), frame())])
        .unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.begin_recovery(MonoTime(10)).unwrap();
    assert_eq!(pump.queued_relative_events(), 0);
    pump.poll(MonoTime(10)).unwrap();
    assert!(
        calls
            .borrow()
            .iter()
            .all(|c| !matches!(c, Call::Relative(_)))
    );
    assert!(pump.ledger().submitted().is_empty());
    assert_eq!(pump.next_deadline(), None);
}
#[test]
fn pending_relative_work_is_not_a_neutral_output_gate() {
    let (mut pump, _) = make(Some(axes()), vec![], vec![]);
    pump.enqueue_mixed(&[], &[(MonoTime(10), frame())]).unwrap();
    assert!(pump.ledger().submitted().is_empty());
    assert!(matches!(
        pump.resume(),
        Err(PumpError::Queue(keyweave_output::OutputError::NotNeutral))
    ));
    pump.poll(MonoTime(10)).unwrap();
    assert!(pump.resume().is_ok());
}
