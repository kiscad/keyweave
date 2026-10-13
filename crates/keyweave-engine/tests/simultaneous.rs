#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, simultaneous::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn cfg(generation: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot::empty(ConfigGeneration(generation)))
}
fn spec() -> Simultaneous {
    let mut spec = Simultaneous::new(vec![key("a"), key("b")]);
    spec.threshold_ns = 50;
    spec
}
fn event(
    cfg: &Arc<ConfigSnapshot>,
    id: u64,
    dev: u64,
    name: &str,
    edge: KeyEdge,
    at: u64,
    delivered: u64,
) -> DueEvent<PlannedAction> {
    DueEvent {
        event: StageEvent {
            stage: Stage::Complex,
            at_ns: MonoTime(at),
            device: DeviceInstanceId {
                id: dev,
                generation: 1,
            },
            source: SourceToken(id),
            config: cfg.clone(),
            payload: PlannedAction {
                slot: ActionSlot {
                    batch: BatchId(id),
                    index: 0,
                },
                target: ActionTarget::Key(key(name)),
                edge,
                lazy: false,
                halt: false,
            },
        },
        delivered_at: MonoTime(delivered),
    }
}
fn matched(decision: ChordDecision) -> ChordMatch {
    match decision {
        ChordDecision::Matched(group) => group,
        other => panic!("expected match: {other:?}"),
    }
}
fn replay(decision: ChordDecision) -> Vec<DueEvent<PlannedAction>> {
    match decision {
        ChordDecision::Replay(events) => events,
        other => panic!("expected replay: {other:?}"),
    }
}
#[test]
fn inclusive_boundary_and_frontier_keep_queued_old_timestamp_input_matchable() {
    let cfg = cfg(1);
    let mut window =
        ChordWindow::new(spec(), event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 300)).unwrap();
    assert!(matches!(
        window.advance(MonoTime(400), Some(MonoTime(150))).unwrap(),
        ChordDecision::Pending { .. }
    ));
    let group = matched(
        window
            .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 150, 400))
            .unwrap(),
    );
    assert_eq!(group.matched_at(), MonoTime(400));
    assert_eq!(group.members()[0].down().event.at_ns, MonoTime(100));
    assert_eq!(group.members()[1].down().event.at_ns, MonoTime(150));
}
#[test]
fn incomplete_timeout_replays_exact_owned_receipts_and_late_incoming_is_retained() {
    let cfg = cfg(1);
    let mut window =
        ChordWindow::new(spec(), event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    let rejected = window
        .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 151, 151))
        .unwrap_err();
    assert_eq!(rejected.error, ChordError::Boundary);
    assert_eq!(rejected.event.event.payload.slot.batch, BatchId(2));
    let events = replay(window.advance(MonoTime(151), Some(MonoTime(151))).unwrap());
    assert_eq!(events.len(), 1);
    assert!(Arc::ptr_eq(&events[0].event.config, &cfg));
    assert_eq!(events[0].event.at_ns, MonoTime(100));
    assert_eq!(events[0].delivered_at, MonoTime(100));
}
#[test]
fn down_order_all_modes_match_or_replay_full_prefix() {
    for order in [
        KeyOrder::Insensitive,
        KeyOrder::Strict,
        KeyOrder::StrictInverse,
    ] {
        for reverse in [false, true] {
            let cfg = cfg(1);
            let mut spec = spec();
            spec.key_down_order = order;
            let (first, second) = if reverse { ("b", "a") } else { ("a", "b") };
            let mut window =
                ChordWindow::new(spec, event(&cfg, 1, 1, first, KeyEdge::Down, 100, 100)).unwrap();
            let decision = window
                .observe(event(&cfg, 2, 1, second, KeyEdge::Down, 110, 110))
                .unwrap();
            let expected = order == KeyOrder::Insensitive
                || (order == KeyOrder::Strict && !reverse)
                || (order == KeyOrder::StrictInverse && reverse);
            if expected {
                assert_eq!(matched(decision).members().len(), 2);
            } else {
                assert_eq!(replay(decision).len(), 2);
            }
        }
    }
}
#[test]
fn interruption_flag_true_allows_other_down_and_preserves_outside_receipt() {
    for uninterrupted in [false, true] {
        let cfg = cfg(1);
        let mut spec = spec();
        spec.detect_key_down_uninterruptedly = uninterrupted;
        let mut window =
            ChordWindow::new(spec, event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
        let decision = window
            .observe(event(&cfg, 3, 1, "x", KeyEdge::Down, 105, 105))
            .unwrap();
        if uninterrupted {
            assert!(matches!(decision, ChordDecision::Pending { .. }));
            let group = matched(
                window
                    .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 110, 110))
                    .unwrap(),
            );
            assert_eq!(group.events().len(), 3);
            assert_eq!(group.events()[1].event.payload.slot.batch, BatchId(3));
        } else {
            assert_eq!(replay(decision).len(), 2);
        }
    }
}
#[test]
fn first_duplicate_key_device_is_member_and_foreign_up_cannot_cancel_window() {
    let cfg = cfg(1);
    let mut window =
        ChordWindow::new(spec(), event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    window
        .observe(event(&cfg, 3, 2, "a", KeyEdge::Down, 105, 105))
        .unwrap();
    window
        .observe(event(&cfg, 3, 2, "a", KeyEdge::Up, 106, 106))
        .unwrap();
    let group = matched(
        window
            .observe(event(&cfg, 2, 2, "b", KeyEdge::Down, 110, 110))
            .unwrap(),
    );
    assert_eq!(group.members()[0].down().event.device.id, 1);
    assert_eq!(group.events().len(), 4);
    let (mut active, events) = group.into_active();
    assert_eq!(events.len(), 4);
    let rejected = active
        .up(&event(&cfg, 3, 2, "a", KeyEdge::Up, 120, 120))
        .unwrap_err();
    assert_eq!(rejected, ChordError::Outside);
    assert!(
        active
            .up(&event(&cfg, 1, 1, "a", KeyEdge::Up, 121, 121))
            .unwrap()
            .release_output
    );
}
#[test]
fn early_member_up_replays_down_and_original_up_in_order() {
    let cfg = cfg(1);
    let mut window =
        ChordWindow::new(spec(), event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    let events = replay(
        window
            .observe(event(&cfg, 1, 1, "a", KeyEdge::Up, 110, 110))
            .unwrap(),
    );
    assert_eq!(
        events
            .iter()
            .map(|e| e.event.payload.edge)
            .collect::<Vec<_>>(),
        [KeyEdge::Down, KeyEdge::Up]
    );
}
#[test]
fn strict_up_prefix_waits_n_minus_one_or_mature_window_and_rejects_wrong_order() {
    for inverse in [false, true] {
        for correct in [false, true] {
            let cfg = cfg(1);
            let mut spec = spec();
            spec.key_up_order = if inverse {
                KeyOrder::StrictInverse
            } else {
                KeyOrder::Strict
            };
            let mut window =
                ChordWindow::new(spec, event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
            assert!(matches!(
                window
                    .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 101, 101))
                    .unwrap(),
                ChordDecision::Pending { .. }
            ));
            let b = inverse == correct;
            let decision = window
                .observe(event(
                    &cfg,
                    if b { 2 } else { 1 },
                    1,
                    if b { "b" } else { "a" },
                    KeyEdge::Up,
                    110,
                    110,
                ))
                .unwrap();
            if correct {
                let group = matched(decision);
                assert_eq!(group.events().len(), 3);
                let (mut active, _) = group.into_active();
                assert!(active.initial_facts().release_output);
                assert!(!active.initial_facts().release_output);
            } else {
                assert_eq!(replay(decision).len(), 3);
            }
        }
    }
}
#[test]
fn strict_up_after_window_maturity_does_not_retroactively_validate_future_order() {
    let cfg = cfg(1);
    let mut spec = spec();
    spec.key_up_order = KeyOrder::Strict;
    let mut window =
        ChordWindow::new(spec, event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    window
        .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 101, 101))
        .unwrap();
    let (mut active, _) = matched(window.advance(MonoTime(150), None).unwrap()).into_active();
    assert!(!active.initial_facts().release_output);
    assert!(
        active
            .up(&event(&cfg, 2, 1, "b", KeyEdge::Up, 160, 160))
            .unwrap()
            .release_output
    );
    assert!(
        active
            .up(&event(&cfg, 1, 1, "a", KeyEdge::Up, 170, 170))
            .unwrap()
            .after_group_up
    );
}
#[test]
fn any_all_release_and_group_after_up_are_independent_one_shot_edges() {
    for when in [KeyUpWhen::Any, KeyUpWhen::All] {
        let cfg = cfg(1);
        let mut spec = spec();
        spec.key_up_when = when;
        let mut window =
            ChordWindow::new(spec, event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
        let (mut active, _) = matched(
            window
                .observe(event(&cfg, 2, 2, "b", KeyEdge::Down, 110, 110))
                .unwrap(),
        )
        .into_active();
        assert!(!active.initial_facts().release_output);
        let first = active
            .up(&event(&cfg, 1, 1, "a", KeyEdge::Up, 120, 120))
            .unwrap();
        assert_eq!(first.release_output, when == KeyUpWhen::Any);
        assert!(!first.after_group_up);
        let second = active
            .up(&event(&cfg, 2, 2, "b", KeyEdge::Up, 130, 130))
            .unwrap();
        assert_eq!(second.release_output, when == KeyUpWhen::All);
        assert!(second.after_group_up && second.finished);
        assert!(!active.initial_facts().after_group_up);
        assert_eq!(
            active
                .up(&event(&cfg, 2, 2, "b", KeyEdge::Up, 131, 131))
                .unwrap_err(),
            ChordError::Duplicate
        );
    }
}
#[test]
fn capacity_reject_returns_incoming_and_preserves_all_original_buffered_events() {
    let cfg = cfg(1);
    let mut spec = spec();
    spec.detect_key_down_uninterruptedly = true;
    let mut window =
        ChordWindow::new(spec, event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    for id in 2..=MAX_CHORD_EVENTS as u64 {
        window
            .observe(event(&cfg, id, 1, "x", KeyEdge::Down, 100, 100))
            .unwrap();
    }
    let rejected = window
        .observe(event(&cfg, 257, 1, "b", KeyEdge::Down, 100, 100))
        .unwrap_err();
    assert_eq!(rejected.error, ChordError::Capacity);
    assert_eq!(rejected.event.event.payload.slot.batch, BatchId(257));
    assert_eq!(
        replay(window.advance(MonoTime(150), None).unwrap()).len(),
        MAX_CHORD_EVENTS
    );
}
#[test]
fn invalid_provenance_and_time_rejections_are_cold_and_return_original_event() {
    let old = cfg(1);
    let foreign = cfg(2);
    let mut spec = spec();
    spec.key_up_order = KeyOrder::Strict;
    let mut window =
        ChordWindow::new(spec, event(&old, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    window
        .observe(event(&old, 2, 1, "b", KeyEdge::Down, 110, 110))
        .unwrap();
    let rejected = window
        .observe(event(&foreign, 1, 1, "a", KeyEdge::Up, 120, 120))
        .unwrap_err();
    assert_eq!(rejected.error, ChordError::Provenance);
    assert!(Arc::ptr_eq(&rejected.event.event.config, &foreign));
    assert_eq!(window.buffered(), 2);
    assert_eq!(
        window
            .observe(event(&old, 3, 1, "x", KeyEdge::Down, 109, 111))
            .unwrap_err()
            .error,
        ChordError::Time
    );
    assert_eq!(window.buffered(), 2);
    assert_eq!(
        matched(
            window
                .observe(event(&old, 1, 1, "a", KeyEdge::Up, 120, 120))
                .unwrap()
        )
        .events()
        .len(),
        3
    );
}
#[test]
fn definition_and_overflow_rejection_retain_first_and_single_key_is_immediate() {
    let cfg = cfg(1);
    for keys in [
        vec![],
        vec![key("a"); 2],
        vec![key("a"); MAX_CHORD_KEYS + 1],
    ] {
        let rejected = ChordWindow::new(
            Simultaneous::new(keys),
            event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100),
        )
        .err()
        .unwrap();
        assert_eq!(rejected.error, ChordError::Definition);
        assert_eq!(rejected.event.event.payload.slot.batch, BatchId(1));
    }
    let rejected = ChordWindow::new(
        spec(),
        event(&cfg, 1, 1, "a", KeyEdge::Down, u64::MAX - 20, u64::MAX - 20),
    )
    .err()
    .unwrap();
    assert_eq!(rejected.error, ChordError::Overflow);
    let mut window = ChordWindow::new(
        Simultaneous::new(vec![key("a")]),
        event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100),
    )
    .unwrap();
    assert_eq!(
        matched(window.advance(MonoTime(100), Some(MonoTime(100))).unwrap())
            .members()
            .len(),
        1
    );
}
#[test]
fn runtime_and_single_key_selection_gate_never_execute_first_chord_key_as_mapping() {
    let mut config = ConfigSnapshot::empty(ConfigGeneration(1));
    config.complex.push(Mapping {
        id: "chord".into(),
        from: key("a"),
        to: vec![key("x").into()],
        allow_extra_modifiers: true,
        extras: MappingExtras {
            simultaneous: Some(spec()),
            ..Default::default()
        },
    });
    let cfg = Arc::new(config);
    assert!(matches!(
        Engine::new(cfg.clone()),
        Err(EngineError::UnsupportedSimultaneous)
    ));
    assert!(matches!(
        physical_controller::PhysicalStageController::new(cfg.clone()),
        Err(physical_controller::PhysicalControllerError::Simple(
            simple_actor::SimpleActorError::Generated(generated_stage::GeneratedError::Engine(
                EngineError::UnsupportedSimultaneous
            ))
        ))
    ));
    assert!(
        rule_selection::select_rule(
            cfg,
            rule_selection::MappingStage::Complex,
            key("a"),
            Default::default(),
            |_| Truth::True
        )
        .is_none()
    );
}

#[test]
fn three_key_strict_up_waits_two_original_releases_and_all_mode_waits_last() {
    let cfg = cfg(1);
    let mut spec = Simultaneous::new(vec![key("a"), key("b"), key("c")]);
    spec.threshold_ns = 50;
    spec.key_up_order = KeyOrder::Strict;
    spec.key_up_when = KeyUpWhen::All;
    let mut window =
        ChordWindow::new(spec, event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    window
        .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 101, 101))
        .unwrap();
    window
        .observe(event(&cfg, 3, 2, "c", KeyEdge::Down, 102, 102))
        .unwrap();
    assert!(matches!(
        window
            .observe(event(&cfg, 1, 1, "a", KeyEdge::Up, 110, 110))
            .unwrap(),
        ChordDecision::Pending { .. }
    ));
    let (mut active, _) = matched(
        window
            .observe(event(&cfg, 2, 1, "b", KeyEdge::Up, 120, 120))
            .unwrap(),
    )
    .into_active();
    assert!(!active.initial_facts().release_output);
    let facts = active
        .up(&event(&cfg, 3, 2, "c", KeyEdge::Up, 130, 130))
        .unwrap();
    assert!(facts.release_output && facts.finished && facts.after_group_up);
}

#[test]
fn active_early_up_and_changed_arc_reject_without_releasing_group() {
    let cfg = cfg(1);
    let mut window =
        ChordWindow::new(spec(), event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    let (mut active, _) = matched(
        window
            .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 110, 110))
            .unwrap(),
    )
    .into_active();
    let foreign = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    assert_eq!(
        active
            .up(&event(&foreign, 1, 1, "a", KeyEdge::Up, 120, 120))
            .unwrap_err(),
        ChordError::Provenance
    );
    assert_eq!(
        active
            .up(&event(&cfg, 1, 1, "a", KeyEdge::Up, 99, 120))
            .unwrap_err(),
        ChordError::Time
    );
    assert!(!active.initial_facts().release_output);
    assert!(
        active
            .up(&event(&cfg, 1, 1, "a", KeyEdge::Up, 120, 120))
            .unwrap()
            .release_output
    );
}

#[test]
fn maximum_definition_size_and_zero_threshold_keep_inclusive_members() {
    let cfg = cfg(1);
    let mut spec = Simultaneous::new(
        (4..4 + MAX_CHORD_KEYS as u16)
            .map(|usage| KeyId::new(KEYBOARD_PAGE, usage))
            .collect(),
    );
    spec.threshold_ns = 0;
    let keys = spec.keys.clone();
    let mut window =
        ChordWindow::new(spec, event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    assert!(matches!(
        window.advance(MonoTime(100), Some(MonoTime(100))).unwrap(),
        ChordDecision::Pending { .. }
    ));
    for (index, key) in keys.into_iter().enumerate().skip(1) {
        let mut receipt = event(&cfg, index as u64 + 1, 2, "b", KeyEdge::Down, 100, 100);
        receipt.event.payload.target = ActionTarget::Key(key);
        let result = window.observe(receipt).unwrap();
        if index + 1 == MAX_CHORD_KEYS {
            assert_eq!(matched(result).members().len(), MAX_CHORD_KEYS);
        } else {
            assert!(matches!(result, ChordDecision::Pending { .. }));
        }
    }
}

#[test]
fn accepted_active_up_keeps_original_receipt_owned_by_caller_for_binding_admission() {
    let cfg = cfg(1);
    let mut window =
        ChordWindow::new(spec(), event(&cfg, 1, 1, "a", KeyEdge::Down, 100, 100)).unwrap();
    let (mut active, _) = matched(
        window
            .observe(event(&cfg, 2, 1, "b", KeyEdge::Down, 110, 110))
            .unwrap(),
    )
    .into_active();
    let receipt = event(&cfg, 1, 1, "a", KeyEdge::Up, 120, 125);
    assert!(active.up(&receipt).unwrap().release_output);
    assert_eq!(receipt.event.payload.slot.batch, BatchId(1));
    assert_eq!(receipt.event.at_ns, MonoTime(120));
    assert_eq!(receipt.delivered_at, MonoTime(125));
    assert!(Arc::ptr_eq(&receipt.event.config, &cfg));
}
