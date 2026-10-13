#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, simultaneous::*, timeline::*, *};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn rule(name: &str, from: &str, to: &str) -> Mapping {
    Mapping {
        id: name.into(),
        from: key(from),
        to: vec![key(to).into()],
        allow_extra_modifiers: true,
        extras: Default::default(),
    }
}
fn chord(name: &str, keys: &[&str], to: &str, threshold: u64) -> Mapping {
    let mut mapping = rule(name, keys[0], to);
    let mut spec = Simultaneous::new(keys.iter().map(|name| key(name)).collect());
    spec.threshold_ns = threshold;
    mapping.extras.simultaneous = Some(spec);
    mapping
}
fn cfg(rules: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "arbitration".into(),
        simple: vec![],
        complex: rules,
    })
}
fn event(
    cfg: &Arc<ConfigSnapshot>,
    id: u64,
    name: &str,
    edge: KeyEdge,
    at: u64,
) -> DueEvent<PlannedAction> {
    DueEvent {
        event: StageEvent {
            stage: Stage::Complex,
            at_ns: MonoTime(at),
            device: DeviceInstanceId {
                id: 1,
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
        delivered_at: MonoTime(at),
    }
}
fn arbiter(cfg: &Arc<ConfigSnapshot>) -> ChordArbiter {
    ChordArbiter::new(event(cfg, 1, "a", KeyEdge::Down, 100)).unwrap()
}
fn ordinary(result: Arbitration) -> (rule_selection::RuleSelection, Vec<DueEvent<PlannedAction>>) {
    match result.choice {
        ArbitrationChoice::Ordinary { selection, inputs } => (selection, inputs),
        _ => panic!("expected ordinary"),
    }
}
fn matched(
    result: Arbitration,
) -> (
    rule_selection::RuleSelection,
    ChordMatch,
    Vec<DueEvent<PlannedAction>>,
) {
    match result.choice {
        ArbitrationChoice::Matched {
            selection,
            group,
            tail,
        } => (selection, group, tail),
        _ => panic!("expected match"),
    }
}
#[test]
fn earlier_ordinary_rule_wins_without_waiting_or_querying_later_conditions() {
    let mut later = chord("later", &["a", "b"], "y", 50);
    later.extras.conditions = vec![Condition::Variable {
        name: "never".into(),
        value: VariableValue::Integer(1),
        unless: false,
    }];
    let cfg = cfg(vec![rule("ordinary", "a", "x"), later]);
    let mut arbiter = ChordArbiter::new(event(&cfg, 1, "a", KeyEdge::Down, 100)).unwrap();
    let (selection, inputs) = ordinary(
        arbiter
            .advance(MonoTime(100), Some(MonoTime(100)), &BTreeSet::new(), |_| {
                panic!("unreachable later condition")
            })
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "ordinary");
    assert_eq!(inputs.len(), 1);
}
#[test]
fn earlier_long_chord_waits_even_when_later_short_chord_is_already_complete() {
    let cfg = cfg(vec![
        chord("abc", &["a", "b", "c"], "x", 50),
        chord("ab", &["a", "b"], "y", 25),
        rule("ordinary", "a", "z"),
    ]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    assert!(matches!(
        arbiter
            .advance(MonoTime(110), Some(MonoTime(111)), &BTreeSet::new(), |_| {
                Truth::True
            })
            .unwrap()
            .choice,
        ArbitrationChoice::Pending {
            deadline: MonoTime(150)
        }
    ));
    let result = arbiter
        .advance(MonoTime(150), None, &BTreeSet::new(), |_| Truth::True)
        .unwrap();
    assert_eq!(result.at, MonoTime(150));
    let (selection, group, tail) = matched(result);
    assert_eq!(selection.mapping().id, "ab");
    assert_eq!(group.events().len(), 2);
    assert!(tail.is_empty());
    assert_eq!(group.events()[1].delivered_at, MonoTime(110));
}
#[test]
fn earlier_long_chord_matches_when_final_member_arrives_at_inclusive_boundary() {
    let cfg = cfg(vec![
        chord("abc", &["a", "b", "c"], "x", 50),
        chord("ab", &["a", "b"], "y", 25),
    ]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    arbiter
        .advance(MonoTime(110), Some(MonoTime(111)), &BTreeSet::new(), |_| {
            Truth::True
        })
        .unwrap();
    arbiter
        .observe(event(&cfg, 3, "c", KeyEdge::Down, 150))
        .unwrap();
    let (selection, group, _) = matched(
        arbiter
            .advance(MonoTime(150), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "abc");
    assert_eq!(group.members().len(), 3);
}
#[test]
fn interruption_failure_falls_back_to_normal_rule_without_reentering_failed_chord() {
    let cfg = cfg(vec![
        chord("ab", &["a", "b"], "y", 50),
        rule("ordinary", "a", "x"),
    ]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 3, "c", KeyEdge::Down, 110))
        .unwrap();
    let (selection, inputs) = ordinary(
        arbiter
            .advance(MonoTime(110), Some(MonoTime(111)), &BTreeSet::new(), |_| {
                Truth::True
            })
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "ordinary");
    assert_eq!(
        inputs
            .iter()
            .map(|event| event.event.payload.slot.batch.0)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(
        arbiter
            .advance(MonoTime(120), None, &BTreeSet::new(), |_| Truth::True)
            .err(),
        Some(ChordError::Finished)
    );
}
#[test]
fn early_up_failure_pass_returns_entire_original_down_up_sequence() {
    let cfg = cfg(vec![chord("ab", &["a", "b"], "y", 50)]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 1, "a", KeyEdge::Up, 110))
        .unwrap();
    let result = arbiter
        .advance(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True)
        .unwrap();
    let ArbitrationChoice::Pass { inputs } = result.choice else {
        panic!("expected pass")
    };
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[1].event.payload.edge, KeyEdge::Up);
    assert!(Arc::ptr_eq(&inputs[0].event.config, &cfg));
}
#[test]
fn replay_lower_match_returns_unobserved_tail_and_current_generation_clock_separately() {
    let cfg = cfg(vec![
        chord("abc", &["a", "b", "c"], "x", 50),
        chord("ab", &["a", "b"], "y", 25),
    ]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    arbiter
        .observe(event(&cfg, 1, "a", KeyEdge::Up, 120))
        .unwrap();
    let result = arbiter
        .advance(MonoTime(120), None, &BTreeSet::new(), |_| Truth::True)
        .unwrap();
    assert_eq!(result.at, MonoTime(120));
    let (selection, group, tail) = matched(result);
    assert_eq!(selection.mapping().id, "ab");
    assert_eq!(group.matched_at(), MonoTime(110));
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].delivered_at, MonoTime(120));
    assert_eq!(tail[0].event.payload.edge, KeyEdge::Up);
}
#[test]
fn unknown_conditions_skip_chord_and_modifiers_are_frozen_for_later_selection() {
    let mut first = chord("conditional", &["a", "b"], "x", 50);
    first.extras.conditions = vec![Condition::Variable {
        name: "flag".into(),
        value: VariableValue::Integer(1),
        unless: false,
    }];
    let mut normal = rule("shifted", "a", "y");
    normal
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::LeftShift);
    let cfg = cfg(vec![first, normal]);
    let modifiers = BTreeSet::from([key("left_shift")]);
    let mut arbiter = ChordArbiter::new(event(&cfg, 1, "a", KeyEdge::Down, 100)).unwrap();
    let (selection, _) = ordinary(
        arbiter
            .advance(MonoTime(100), None, &modifiers, |_| Truth::Unknown)
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "shifted");
    assert_eq!(selection.modifiers_at_down(), &modifiers);
    assert_eq!(selection.consumed_modifiers(), &modifiers);
}
#[test]
fn action_conditions_filter_once_at_final_choice_and_empty_match_still_consumes() {
    let mut first = chord("abc", &["a", "b", "c"], "x", 50);
    first.to[0].conditions = vec![Condition::Variable {
        name: "first".into(),
        value: VariableValue::Integer(1),
        unless: false,
    }];
    let mut second = chord("ab", &["a", "b"], "y", 25);
    second.to[0].conditions = vec![Condition::Variable {
        name: "second".into(),
        value: VariableValue::Integer(1),
        unless: false,
    }];
    let cfg = cfg(vec![first, second]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    arbiter
        .advance(MonoTime(110), Some(MonoTime(111)), &BTreeSet::new(), |_| {
            panic!("pending has no action filtering")
        })
        .unwrap();
    let mut calls = 0;
    let (selection, _, _) = matched(
        arbiter
            .advance(MonoTime(150), None, &BTreeSet::new(), |condition| {
                calls += 1;
                let Condition::Variable { name, .. } = condition else {
                    panic!("variable")
                };
                assert_eq!(name, "second");
                Truth::False
            })
            .unwrap(),
    );
    assert_eq!(calls, 1);
    assert_eq!(selection.actions().len(), 0);
}
#[test]
fn original_arc_wins_over_new_peer_snapshot_and_all_receipts_retain_their_own_arc() {
    let old = cfg(vec![chord("old", &["a", "b"], "x", 50)]);
    let mut new = ConfigSnapshot::empty(ConfigGeneration(2));
    new.complex = vec![chord("new", &["a", "b"], "y", 50)];
    let new = Arc::new(new);
    let mut arbiter = arbiter(&old);
    arbiter
        .observe(event(&new, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    let (selection, group, _) = matched(
        arbiter
            .advance(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "old");
    assert!(Arc::ptr_eq(selection.snapshot(), &old));
    assert!(Arc::ptr_eq(&group.events()[1].event.config, &new));
}
#[test]
fn boundary_input_is_returned_after_timeout_fallback_without_loss_or_reordering() {
    let cfg = cfg(vec![
        chord("ab", &["a", "b"], "y", 25),
        rule("ordinary", "a", "x"),
    ]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .advance(MonoTime(100), Some(MonoTime(101)), &BTreeSet::new(), |_| {
            Truth::True
        })
        .unwrap();
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 130))
        .unwrap();
    let (selection, inputs) = ordinary(
        arbiter
            .advance(MonoTime(130), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "ordinary");
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[1].event.at_ns, MonoTime(130));
}
#[test]
fn frontier_protects_pending_window_and_cold_rejections_preserve_queued_inputs() {
    let cfg = cfg(vec![chord("ab", &["a", "b"], "y", 50)]);
    let mut arbiter = arbiter(&cfg);
    assert!(matches!(
        arbiter
            .advance(MonoTime(151), Some(MonoTime(150)), &BTreeSet::new(), |_| {
                Truth::True
            })
            .unwrap()
            .choice,
        ArbitrationChoice::Pending { .. }
    ));
    let rejected = arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 150))
        .unwrap_err();
    assert_eq!(rejected.error, ChordError::Time);
    assert_eq!(arbiter.buffered(), 1);
    let mut receipt = event(&cfg, 2, "b", KeyEdge::Down, 150);
    receipt.delivered_at = MonoTime(151);
    arbiter.observe(receipt).unwrap();
    let (_, group, _) = matched(
        arbiter
            .advance(MonoTime(151), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap(),
    );
    assert_eq!(group.members().len(), 2);
}
#[test]
fn deferred_provenance_failure_retains_every_accepted_input_for_abort_owner() {
    let mut mapping = chord("ab", &["a", "b"], "x", 50);
    mapping.extras.simultaneous.as_mut().unwrap().key_up_order = KeyOrder::Strict;
    let cfg = cfg(vec![mapping]);
    let foreign = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    arbiter
        .advance(MonoTime(110), Some(MonoTime(111)), &BTreeSet::new(), |_| {
            Truth::True
        })
        .unwrap();
    arbiter
        .observe(event(&foreign, 1, "a", KeyEdge::Up, 120))
        .unwrap();
    assert_eq!(
        arbiter
            .advance(MonoTime(120), None, &BTreeSet::new(), |_| Truth::True)
            .err(),
        Some(ChordError::Provenance)
    );
    let inputs = arbiter.into_inputs();
    assert_eq!(inputs.len(), 3);
    assert!(Arc::ptr_eq(&inputs[2].event.config, &foreign));
}

#[test]
fn waiting_conditions_recheck_and_later_previously_unknown_candidate_can_become_eligible() {
    let condition = |name: &str| Condition::Variable {
        name: name.into(),
        value: VariableValue::Integer(1),
        unless: false,
    };
    let mut first = chord("abc", &["a", "b", "c"], "x", 50);
    first.extras.conditions = vec![condition("first")];
    let mut second = chord("ab", &["a", "b"], "y", 25);
    second.extras.conditions = vec![condition("second")];
    let cfg = cfg(vec![first, second, rule("ordinary", "a", "z")]);
    let mut arbiter = arbiter(&cfg);
    assert!(matches!(
        arbiter
            .advance(
                MonoTime(100),
                Some(MonoTime(101)),
                &BTreeSet::new(),
                |condition| {
                    let Condition::Variable { name, .. } = condition else {
                        panic!("variable")
                    };
                    assert_eq!(name, "first");
                    Truth::True
                }
            )
            .unwrap()
            .choice,
        ArbitrationChoice::Pending { .. }
    ));
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    let (selection, group, _) = matched(
        arbiter
            .advance(
                MonoTime(110),
                Some(MonoTime(111)),
                &BTreeSet::new(),
                |condition| {
                    let Condition::Variable { name, .. } = condition else {
                        panic!("variable")
                    };
                    if name == "first" {
                        Truth::Unknown
                    } else {
                        Truth::True
                    }
                },
            )
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "ab");
    assert_eq!(group.events().len(), 2);
}

#[test]
fn pending_modifier_eligibility_uses_fresh_view_and_final_selection_saves_that_view() {
    let mut first = chord("shifted", &["a", "b"], "x", 50);
    first
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::LeftShift);
    let cfg = cfg(vec![first, rule("ordinary", "a", "y")]);
    let mut arbiter = arbiter(&cfg);
    let shift = BTreeSet::from([key("left_shift")]);
    assert!(matches!(
        arbiter
            .advance(MonoTime(100), Some(MonoTime(101)), &shift, |_| Truth::True)
            .unwrap()
            .choice,
        ArbitrationChoice::Pending { .. }
    ));
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    let (selection, inputs) = ordinary(
        arbiter
            .advance(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "ordinary");
    assert!(selection.modifiers_at_down().is_empty());
    assert_eq!(inputs.len(), 2);
}

#[test]
fn arbiter_total_buffer_capacity_is_shared_across_candidate_and_queued_tail() {
    let mut first = chord("ab", &["a", "b"], "x", 50);
    first
        .extras
        .simultaneous
        .as_mut()
        .unwrap()
        .detect_key_down_uninterruptedly = true;
    let cfg = cfg(vec![first]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .advance(MonoTime(100), Some(MonoTime(101)), &BTreeSet::new(), |_| {
            Truth::True
        })
        .unwrap();
    for id in 2..=MAX_CHORD_EVENTS as u64 {
        arbiter
            .observe(event(&cfg, id, "x", KeyEdge::Down, 110))
            .unwrap();
    }
    let rejected = arbiter
        .observe(event(&cfg, 257, "b", KeyEdge::Down, 110))
        .unwrap_err();
    assert_eq!(rejected.error, ChordError::Capacity);
    assert_eq!(rejected.event.event.payload.slot.batch, BatchId(257));
    assert_eq!(arbiter.buffered(), MAX_CHORD_EVENTS);
    assert_eq!(arbiter.into_inputs().len(), MAX_CHORD_EVENTS);
}

#[test]
fn invalid_first_and_oversized_rule_catalog_return_original_receipt_without_condition_work() {
    let cfg = cfg(vec![]);
    let rejected = ChordArbiter::new(event(&cfg, 1, "a", KeyEdge::Up, 100))
        .err()
        .unwrap();
    assert_eq!(rejected.error, ChordError::Input);
    assert_eq!(rejected.event.event.payload.edge, KeyEdge::Up);
    let large = super_cfg_large();
    let rejected = ChordArbiter::new(event(&large, 1, "a", KeyEdge::Down, 100))
        .err()
        .unwrap();
    assert_eq!(rejected.error, ChordError::Definition);
    assert!(Arc::ptr_eq(&rejected.event.event.config, &large));
}
fn super_cfg_large() -> Arc<ConfigSnapshot> {
    cfg(vec![rule("ordinary", "a", "x"); 4097])
}

#[test]
fn one_work_budget_resumes_input_without_starvation_or_duplicate_receipts() {
    let cfg = cfg(vec![chord("ab", &["a", "b"], "x", 50)]);
    let mut arbiter = arbiter(&cfg);
    arbiter
        .observe(event(&cfg, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    let first = arbiter
        .advance_with_budget(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True, 1)
        .unwrap();
    assert!(matches!(
        first.choice,
        ArbitrationChoice::Yield { buffered: 2, .. }
    ));
    let (selection, group, tail) = matched(
        arbiter
            .advance_with_budget(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True, 1)
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "ab");
    assert_eq!(group.events().len(), 2);
    assert!(tail.is_empty());
}

#[test]
fn long_rule_search_yields_with_old_arc_and_event_preserved_then_finishes() {
    let mut rules = vec![rule("unrelated", "z", "x"); 130];
    rules.push(rule("chosen", "a", "y"));
    let cfg = cfg(rules);
    let mut arbiter = arbiter(&cfg);
    for _ in 0..2 {
        assert!(matches!(
            arbiter
                .advance(MonoTime(100), None, &BTreeSet::new(), |_| Truth::True)
                .unwrap()
                .choice,
            ArbitrationChoice::Yield { buffered: 1, .. }
        ));
    }
    let (selection, inputs) = ordinary(
        arbiter
            .advance(MonoTime(100), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap(),
    );
    assert_eq!(selection.mapping().id, "chosen");
    assert_eq!(inputs.len(), 1);
    assert!(Arc::ptr_eq(selection.snapshot(), &cfg));
}

#[test]
fn invalid_budget_is_cold_and_abort_retains_original_input() {
    let cfg = cfg(vec![chord("ab", &["a", "b"], "x", 50)]);
    let mut arbiter = arbiter(&cfg);
    for budget in [0, 65] {
        assert_eq!(
            arbiter
                .advance_with_budget(
                    MonoTime(100),
                    None,
                    &BTreeSet::new(),
                    |_| Truth::True,
                    budget
                )
                .err(),
            Some(ChordError::Definition)
        );
        assert_eq!(arbiter.buffered(), 1);
    }
    assert_eq!(arbiter.into_inputs().len(), 1);
}
