#![allow(clippy::unwrap_used)]
use keyweave_output::*;
use keyweave_types::*;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn edge(name: &str, kind: KeyEdge, at: u64) -> OutputIntent {
    OutputIntent {
        at_ns: MonoTime(at),
        key: key(name),
        edge: kind,
    }
}
fn running() -> OutputQueue {
    let mut queue = OutputQueue::new();
    queue.resume().unwrap();
    queue
}
#[test]
fn future_up_and_later_events_wait_in_fifo_without_pretending_submission() {
    let mut queue = running();
    queue
        .enqueue(&[edge("a", KeyEdge::Down, 10), edge("a", KeyEdge::Up, 510)])
        .unwrap();
    queue.enqueue(&[edge("b", KeyEdge::Down, 20)]).unwrap();
    assert_eq!(
        queue.desired().iter().copied().collect::<Vec<_>>(),
        [key("b")]
    );
    assert!(queue.submitted().is_empty());
    assert!(queue.begin_due(MonoTime(9)).unwrap().is_none());
    let batch = queue.begin_due(MonoTime(10)).unwrap().unwrap();
    assert_eq!(batch.edges, [edge("a", KeyEdge::Down, 10)]);
    assert!(queue.submitted().is_empty());
    queue.acknowledge(batch.id, 1).unwrap();
    assert!(queue.submitted().contains(&key("a")));
    assert_eq!(queue.next_deadline(), Some(MonoTime(510)));
    assert!(queue.begin_due(MonoTime(509)).unwrap().is_none());
    let batch = queue.begin_due(MonoTime(510)).unwrap().unwrap();
    assert_eq!(
        batch.edges,
        [edge("a", KeyEdge::Up, 510), edge("b", KeyEdge::Down, 510)]
    );
    queue.acknowledge(batch.id, 2).unwrap();
    assert_eq!(queue.submitted(), queue.desired());
}
#[test]
fn every_partial_prefix_releases_only_keys_actually_accepted() {
    for accepted in 0..4 {
        let mut queue = running();
        queue
            .enqueue(&[
                edge("a", KeyEdge::Down, 0),
                edge("b", KeyEdge::Down, 0),
                edge("a", KeyEdge::Up, 0),
                edge("b", KeyEdge::Up, 0),
            ])
            .unwrap();
        let batch = queue.begin_due(MonoTime(0)).unwrap().unwrap();
        assert_eq!(
            queue.acknowledge(batch.id, accepted).unwrap(),
            Acknowledgment::RecoveryRequired
        );
        let expected: Vec<_> = match accepted {
            1 => vec![key("a")],
            2 => vec![key("a"), key("b")],
            3 => vec![key("b")],
            _ => vec![],
        };
        assert_eq!(
            queue.submitted().iter().copied().collect::<Vec<_>>(),
            expected
        );
        assert!(queue.desired().is_empty());
        if let Some(recovery) = queue.begin_due(MonoTime(0)).unwrap() {
            assert_eq!(
                recovery.edges.iter().map(|e| e.key).collect::<Vec<_>>(),
                expected
            );
            assert!(recovery.edges.iter().all(|e| e.edge == KeyEdge::Up));
            queue
                .acknowledge(recovery.id, recovery.edges.len())
                .unwrap();
        }
        assert_eq!(queue.mode(), Mode::Paused);
        assert!(queue.submitted().is_empty());
        assert_eq!(
            queue.enqueue(&[edge("c", KeyEdge::Down, 0)]),
            Err(OutputError::NotRunning)
        );
        queue.resume().unwrap();
    }
}
#[test]
fn short_recovery_retries_remaining_ups_and_does_not_automatically_resume() {
    let mut queue = running();
    queue
        .enqueue(&[edge("a", KeyEdge::Down, 0), edge("b", KeyEdge::Down, 0)])
        .unwrap();
    let batch = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    queue.acknowledge(batch.id, 2).unwrap();
    queue.begin_recovery(MonoTime(1)).unwrap();
    let recovery = queue.begin_due(MonoTime(1)).unwrap().unwrap();
    assert_eq!(
        queue.acknowledge(recovery.id, 1).unwrap(),
        Acknowledgment::RecoveryRequired
    );
    assert_eq!(queue.mode(), Mode::Recovering);
    assert_eq!(queue.resume(), Err(OutputError::NotNeutral));
    let retry = queue.begin_due(MonoTime(2)).unwrap().unwrap();
    assert_eq!(retry.edges, [edge("b", KeyEdge::Up, 1)]);
    queue.acknowledge(retry.id, 0).unwrap();
    let retry = queue.begin_due(MonoTime(3)).unwrap().unwrap();
    assert_eq!(retry.edges.len(), 1);
    queue.acknowledge(retry.id, 1).unwrap();
    assert_eq!(queue.mode(), Mode::Paused);
}
#[test]
fn recovery_discards_future_edges_and_releases_submitted_down_now() {
    let mut queue = running();
    queue
        .enqueue(&[
            edge("a", KeyEdge::Down, 0),
            edge("a", KeyEdge::Up, 1000),
            edge("b", KeyEdge::Down, 1000),
        ])
        .unwrap();
    let batch = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    queue.acknowledge(batch.id, 1).unwrap();
    queue.begin_recovery(MonoTime(5)).unwrap();
    let batch = queue.begin_due(MonoTime(5)).unwrap().unwrap();
    assert_eq!(batch.edges, [edge("a", KeyEdge::Up, 5)]);
    queue.acknowledge(batch.id, 1).unwrap();
    assert!(queue.begin_due(MonoTime(1000)).unwrap().is_none());
}
#[test]
fn acknowledgments_are_single_flight_checked_and_never_reused() {
    let mut queue = running();
    queue.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    let first = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    assert_eq!(queue.begin_due(MonoTime(0)), Err(OutputError::InFlight));
    assert_eq!(
        queue.begin_recovery(MonoTime(0)),
        Err(OutputError::InFlight)
    );
    assert_eq!(
        queue.acknowledge(first.id, 2),
        Err(OutputError::InvalidAcknowledgment)
    );
    assert_eq!(queue.queued_edges(), 1);
    assert!(queue.submitted().is_empty());
    queue.acknowledge(first.id, 1).unwrap();
    queue.begin_recovery(MonoTime(1)).unwrap();
    let second = queue.begin_due(MonoTime(1)).unwrap().unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(
        queue.acknowledge(first.id, 1),
        Err(OutputError::InvalidAcknowledgment)
    );
    assert!(queue.submitted().contains(&key("a")));
    queue.acknowledge(second.id, 1).unwrap();
}
#[test]
fn appending_during_flight_is_bounded_and_short_submission_discards_append() {
    let mut queue = running();
    queue.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    let batch = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    queue
        .enqueue(&[edge("a", KeyEdge::Up, 1), edge("b", KeyEdge::Down, 2)])
        .unwrap();
    queue.acknowledge(batch.id, 0).unwrap();
    assert_eq!(queue.mode(), Mode::Paused);
    assert_eq!(queue.queued_edges(), 0);
    assert!(queue.submitted().is_empty());
}
#[test]
fn rejected_transitions_and_batch_capacity_are_atomic() {
    let mut queue = running();
    for edges in [
        vec![edge("a", KeyEdge::Up, 0)],
        vec![edge("a", KeyEdge::Repeat, 0)],
        vec![edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Down, 1)],
    ] {
        assert_eq!(queue.enqueue(&edges), Err(OutputError::InvalidTransition));
        assert!(queue.desired().is_empty());
        assert_eq!(queue.queued_edges(), 0);
    }
    let tap = [edge("a", KeyEdge::Down, 100), edge("a", KeyEdge::Up, 101)];
    let edges: Vec<_> = tap
        .iter()
        .cloned()
        .cycle()
        .take(MAX_ENQUEUE_EDGES)
        .collect();
    for _ in 0..(MAX_QUEUED_EDGES / MAX_ENQUEUE_EDGES) {
        queue.enqueue(&edges).unwrap();
    }
    assert_eq!(queue.queued_edges(), MAX_QUEUED_EDGES);
    assert_eq!(queue.enqueue(&tap), Err(OutputError::Capacity));
    assert_eq!(queue.queued_edges(), MAX_QUEUED_EDGES);
    assert!(queue.desired().is_empty());
    assert_eq!(
        queue.enqueue(&vec![tap[0].clone(); MAX_ENQUEUE_EDGES + 1]),
        Err(OutputError::Capacity)
    );
}
#[test]
fn native_submission_batches_are_bounded_and_keep_equal_time_order() {
    let mut queue = running();
    let tap = [edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Up, 0)];
    queue
        .enqueue(&tap.iter().cloned().cycle().take(1024).collect::<Vec<_>>())
        .unwrap();
    for _ in 0..4 {
        let batch = queue.begin_due(MonoTime(0)).unwrap().unwrap();
        assert_eq!(batch.edges.len(), MAX_SUBMISSION_EDGES);
        assert!(batch.edges.chunks(2).all(|pair| pair == tap));
        queue.acknowledge(batch.id, batch.edges.len()).unwrap();
    }
    assert!(queue.begin_due(MonoTime(0)).unwrap().is_none());
    assert!(queue.submitted().is_empty());
}
#[test]
fn repeat_does_not_create_submitted_owners() {
    let mut queue = running();
    queue
        .enqueue(&[edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Repeat, 0)])
        .unwrap();
    let batch = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    queue.acknowledge(batch.id, 2).unwrap();
    assert_eq!(queue.submitted().len(), 1);
    queue.begin_recovery(MonoTime(0)).unwrap();
    assert_eq!(queue.queued_edges(), 1);
}
#[test]
fn monotonic_clock_and_initial_activation_are_explicit() {
    let mut queue = OutputQueue::new();
    assert_eq!(queue.mode(), Mode::Paused);
    assert_eq!(
        queue.enqueue(&[edge("a", KeyEdge::Down, 0)]),
        Err(OutputError::NotRunning)
    );
    assert!(queue.begin_due(MonoTime(10)).unwrap().is_none());
    assert_eq!(
        queue.begin_recovery(MonoTime(9)),
        Err(OutputError::TimeReversed)
    );
    assert_eq!(queue.begin_due(MonoTime(9)), Err(OutputError::TimeReversed));
    queue.resume().unwrap();
    queue.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    let batch = queue.begin_due(MonoTime(10)).unwrap().unwrap();
    assert_eq!(batch.edges[0].at_ns, MonoTime(10));
}
#[test]
fn projected_pressed_capacity_is_checked_even_when_batch_ends_neutral() {
    let mut queue = running();
    for start in (0..MAX_PRESSED_KEYS).step_by(MAX_ENQUEUE_EDGES) {
        let edges: Vec<_> = (start..start + MAX_ENQUEUE_EDGES)
            .map(|usage| OutputIntent {
                at_ns: MonoTime(0),
                key: KeyId::new(7, usage as u16),
                edge: KeyEdge::Down,
            })
            .collect();
        queue.enqueue(&edges).unwrap();
    }
    let extra = KeyId::new(7, MAX_PRESSED_KEYS as u16);
    assert_eq!(
        queue.enqueue(&[
            OutputIntent {
                at_ns: MonoTime(0),
                key: extra,
                edge: KeyEdge::Down
            },
            OutputIntent {
                at_ns: MonoTime(0),
                key: extra,
                edge: KeyEdge::Up
            }
        ]),
        Err(OutputError::Capacity)
    );
    assert_eq!(queue.desired().len(), MAX_PRESSED_KEYS);
}

#[test]
fn all_edges_without_native_frame_completion_discards_future_plans() {
    let mut queue = running();
    queue
        .enqueue(&[edge("a", KeyEdge::Down, 0), edge("a", KeyEdge::Up, 100)])
        .unwrap();
    let batch = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    assert_eq!(
        queue.acknowledge_incomplete(batch.id, 1).unwrap(),
        Acknowledgment::RecoveryRequired
    );
    assert_eq!(queue.next_deadline(), Some(MonoTime(0)));
    assert!(queue.desired().is_empty());
    let up = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    assert_eq!(up.edges, [edge("a", KeyEdge::Up, 0)]);
    assert_eq!(
        queue.acknowledge_incomplete(up.id, 1).unwrap(),
        Acknowledgment::RecoveryRequired
    );
    assert_eq!(queue.mode(), Mode::Paused);
}
#[test]
fn unknown_acceptance_freezes_ledger_until_actual_backend_destruction() {
    let mut queue = running();
    queue.enqueue(&[edge("a", KeyEdge::Down, 0)]).unwrap();
    let first = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    queue.acknowledge(first.id, 1).unwrap();
    queue.enqueue(&[edge("a", KeyEdge::Up, 1)]).unwrap();
    let uncertain = queue.begin_due(MonoTime(1)).unwrap().unwrap();
    queue.quarantine();
    assert!(queue.submitted().contains(&key("a")));
    assert_eq!(queue.queued_edges(), 1);
    assert_eq!(
        queue.acknowledge(uncertain.id, 0),
        Err(OutputError::Quarantined)
    );
    assert_eq!(
        queue.acknowledge_incomplete(uncertain.id, 1),
        Err(OutputError::Quarantined)
    );
    assert_eq!(queue.resume(), Err(OutputError::Quarantined));
    assert_eq!(
        queue.begin_recovery(MonoTime(1)),
        Err(OutputError::Quarantined)
    );
    assert_eq!(queue.begin_due(MonoTime(1)), Err(OutputError::Quarantined));
    queue.backend_destroyed().unwrap();
    assert!(queue.submitted().is_empty());
    assert_eq!(queue.mode(), Mode::Paused);
    queue.resume().unwrap();
    queue.enqueue(&[edge("b", KeyEdge::Down, 2)]).unwrap();
    let next = queue.begin_due(MonoTime(2)).unwrap().unwrap();
    assert_ne!(uncertain.id, next.id);
    assert_eq!(
        queue.acknowledge(uncertain.id, 1),
        Err(OutputError::InvalidAcknowledgment)
    );
    assert_eq!(queue.backend_destroyed(), Err(OutputError::NotQuarantined));
}

#[test]
fn recovery_releases_consumer_key_before_modifier_and_short_return_preserves_scope() {
    let mut queue = running();
    queue
        .enqueue(&[
            edge("left_control", KeyEdge::Down, 0),
            edge("consumer:mute", KeyEdge::Down, 0),
            edge("consumer:mute", KeyEdge::Up, 100),
            edge("left_control", KeyEdge::Up, 100),
        ])
        .unwrap();
    let down = queue.begin_due(MonoTime(0)).unwrap().unwrap();
    queue.acknowledge(down.id, 2).unwrap();
    queue.begin_recovery(MonoTime(1)).unwrap();
    let cleanup = queue.begin_due(MonoTime(1)).unwrap().unwrap();
    assert_eq!(
        cleanup.edges,
        [
            edge("consumer:mute", KeyEdge::Up, 1),
            edge("left_control", KeyEdge::Up, 1)
        ]
    );
    assert_eq!(
        queue.acknowledge(cleanup.id, 1).unwrap(),
        Acknowledgment::RecoveryRequired
    );
    assert!(queue.submitted().contains(&key("left_control")));
    assert!(!queue.submitted().contains(&key("consumer:mute")));
    let rest = queue.begin_due(MonoTime(2)).unwrap().unwrap();
    assert_eq!(rest.edges, [edge("left_control", KeyEdge::Up, 1)]);
    queue.acknowledge(rest.id, 1).unwrap();
    assert_eq!(queue.mode(), Mode::Paused);
    assert!(queue.submitted().is_empty());
}
