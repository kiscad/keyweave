#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::GeneratedError, modifier_scope::ScopedActionPlan,
    output_stage::*, rule_selection::*, terminal_actor::ActorError, timeline::*, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn variable() -> Action {
    Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        hold_down_ns: 50,
        ..key("b").into()
    }
}
fn config() -> Arc<ConfigSnapshot> {
    let mut extras = MappingExtras::default();
    extras.from_modifiers.mandatory.insert(Modifier::Shift);
    extras.if_alone = vec![variable()];
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "output".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key("right_control").into()],
            allow_extra_modifiers: true,
            extras,
        }],
    })
}
fn actor(config: Arc<ConfigSnapshot>) -> OutputStageActor {
    let mut actor = OutputStageActor::new(config).unwrap();
    for id in [1, 2] {
        actor
            .observe_device(device(id), false, MonoTime(0))
            .unwrap();
    }
    actor
}
fn plain(
    actor: &OutputStageActor,
    id: u64,
    start: u64,
    device_id: u64,
    actions: &[Action],
    tap: bool,
) -> ActionPlan {
    plan_actions(
        actions,
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(start),
            device: device(device_id),
            source: SourceToken(id + 100),
            config: actor.snapshot().clone(),
        },
        tap,
    )
    .unwrap()
}
fn drain(actor: &mut OutputStageActor, at: u64) -> (Vec<(String, KeyEdge)>, Vec<BatchId>) {
    let mut outputs = vec![];
    let mut completed = vec![];
    while let Some(step) = actor.poll(MonoTime(at)).unwrap() {
        outputs.extend(step.outputs.into_iter().map(|o| (o.key.name(), o.edge)));
        completed.extend(step.completed_batch);
    }
    (outputs, completed)
}
fn shift(actor: &mut OutputStageActor) {
    actor
        .enqueue(plain(actor, 1, 0, 2, &[key("left_shift").into()], false))
        .unwrap();
    drain(actor, 0);
}
fn selected(actor: &OutputStageActor, at: u64) -> RuleSelection {
    actor
        .select(
            key("a"),
            &ContextSnapshot::default(),
            device(1),
            MonoTime(at),
        )
        .unwrap()
        .unwrap()
}
fn main(rule: &RuleSelection, id: u64, start: u64) -> ScopedActionPlan {
    ScopedActionPlan::main(
        rule,
        BatchId(id),
        MonoTime(start),
        device(1),
        SourceToken(100 + id),
    )
    .unwrap()
}
#[test]
fn raw_deferred_work_executes_saved_bindings_and_automatically_retires_both_sides() {
    let mut actor = actor(config());
    let actions = [
        Action {
            hold_down_ns: 50,
            ..key("b").into()
        },
        key("c").into(),
    ];
    actor
        .enqueue(plain(&actor, 1, 0, 1, &actions, false))
        .unwrap();
    assert_eq!(actor.pending_batches(), 1);
    assert_eq!(actor.reserved_slots(), 2);
    assert_eq!(drain(&mut actor, 0).0, [("b".into(), KeyEdge::Down)]);
    assert!(drain(&mut actor, 49).0.is_empty());
    assert_eq!(
        drain(&mut actor, 50).0,
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
    actor.input_up(BatchId(1), MonoTime(60)).unwrap();
    let (outputs, complete) = drain(&mut actor, 60);
    assert_eq!(outputs, [("c".into(), KeyEdge::Up)]);
    assert_eq!(complete, [BatchId(1)]);
    assert_eq!(actor.reserved_slots(), 0);
    assert_eq!(actor.pending_batches(), 0);
    assert!(!actor.input_up(BatchId(1), MonoTime(60)).unwrap());
}
#[test]
fn selection_uses_generated_consumed_view_and_actual_scope_waits_for_original_up() {
    let mut actor = actor(config());
    shift(&mut actor);
    let rule = selected(&actor, 0);
    actor.enqueue_scoped(main(&rule, 2, 100)).unwrap();
    assert_eq!(
        actor.generated_modifiers(),
        BTreeSet::from([key("right_control")])
    );
    assert!(
        actor
            .select(
                key("a"),
                &ContextSnapshot::default(),
                device(1),
                MonoTime(0)
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(actor.pressed_outputs(), [key("left_shift")]);
    assert_eq!(
        drain(&mut actor, 100).0,
        [
            ("left_shift".into(), KeyEdge::Up),
            ("right_control".into(), KeyEdge::Down)
        ]
    );
    actor.input_up(BatchId(2), MonoTime(110)).unwrap();
    assert_eq!(
        actor.generated_modifiers(),
        BTreeSet::from([key("left_shift")])
    );
    assert_eq!(actor.pressed_outputs(), [key("right_control")]);
    let (outputs, complete) = drain(&mut actor, 110);
    assert_eq!(
        outputs,
        [
            ("right_control".into(), KeyEdge::Up),
            ("left_shift".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(complete, [BatchId(2)]);
    assert_eq!(actor.pending_batches(), 1);
}
#[test]
fn scope_marker_does_not_consume_following_batch_action_at_the_same_time() {
    let mut config = (*config()).clone();
    config.complex[0].to[0].repeat = false;
    config.complex[0].to[0].hold_down_ns = 50;
    let mut actor = actor(Arc::new(config));
    shift(&mut actor);
    actor
        .enqueue_scoped(main(&selected(&actor, 0), 2, 100))
        .unwrap();
    drain(&mut actor, 100);
    actor.input_up(BatchId(2), MonoTime(110)).unwrap(); // generated recipe has no key Up
    actor
        .enqueue(plain(&actor, 3, 110, 1, &[key("c").into()], false))
        .unwrap();
    let first = actor.poll(MonoTime(150)).unwrap().unwrap();
    assert!(first.completed_batch.is_none());
    let marker = actor.poll(MonoTime(150)).unwrap().unwrap();
    assert_eq!(marker.completed_batch, Some(BatchId(2)));
    assert_eq!(actor.pending_batches(), 2);
    let next = actor.poll(MonoTime(150)).unwrap().unwrap();
    assert!(
        next.outputs
            .iter()
            .any(|o| o.key == key("c") && o.edge == KeyEdge::Down)
    );
    assert!(!actor.is_failed());
    actor.input_up(BatchId(3), MonoTime(160)).unwrap();
    drain(&mut actor, 160);
    assert_eq!(actor.pending_batches(), 1);
}
#[test]
fn snapshot_variable_generation_and_actual_delivery_remain_distinct_until_retirement() {
    let mut actor = actor(config());
    shift(&mut actor);
    let branch = selected(&actor, 0)
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    actor
        .enqueue_scoped(
            ScopedActionPlan::branch(
                &branch,
                BatchId(2),
                MonoTime(100),
                device(1),
                SourceToken(102),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(actor.generated_variables()["v"], VariableValue::Integer(2));
    assert!(actor.executed_variables().is_empty());
    assert_eq!(actor.reserved_slots(), 3);
    drain(&mut actor, 100);
    assert_eq!(actor.executed_variables()["v"], VariableValue::Integer(1));
    let (_, complete) = drain(&mut actor, 150);
    assert_eq!(complete, [BatchId(2)]);
    assert_eq!(actor.executed_variables()["v"], VariableValue::Integer(2));
    assert_eq!(actor.reserved_slots(), 1);
}
#[test]
fn pending_cancel_keeps_physical_modifier_and_reclaims_both_reservation_sets() {
    let mut actor = actor(config());
    shift(&mut actor);
    actor
        .enqueue_scoped(main(&selected(&actor, 0), 2, 100))
        .unwrap();
    assert!(
        actor
            .cancel_batch(BatchId(2), MonoTime(10))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        actor.generated_modifiers(),
        BTreeSet::from([key("left_shift")])
    );
    assert_eq!(actor.pressed_outputs(), [key("left_shift")]);
    assert_eq!(actor.reserved_slots(), 1);
    assert!(drain(&mut actor, 100).0.is_empty());
}
#[test]
fn device_removal_cleans_only_its_actual_scoped_bindings_and_preserves_other_owner() {
    let mut actor = actor(config());
    shift(&mut actor);
    actor
        .enqueue_scoped(main(&selected(&actor, 0), 2, 100))
        .unwrap();
    drain(&mut actor, 100);
    let outputs = actor
        .observe_device(device(1), true, MonoTime(110))
        .unwrap();
    assert!(
        outputs
            .iter()
            .any(|o| o.key == key("right_control") && o.edge == KeyEdge::Up)
    );
    assert_eq!(
        actor.generated_modifiers(),
        BTreeSet::from([key("left_shift")])
    );
    assert_eq!(actor.pressed_outputs(), [key("left_shift")]);
    assert_eq!(actor.pending_batches(), 1);
    assert_eq!(actor.reserved_slots(), 1);
    assert!(!actor.input_up(BatchId(2), MonoTime(110)).unwrap());
    assert!(matches!(
        actor.observe_device(device(1), false, MonoTime(110)),
        Err(OutputStageError::Generated(GeneratedError::Timeline(
            TimelineError::RetiredDevice
        )))
    ));
    let newer = DeviceInstanceId {
        id: 1,
        generation: 2,
    };
    actor.observe_device(newer, false, MonoTime(110)).unwrap();
}
#[test]
fn reload_preserves_saved_old_variable_up_and_invalid_reload_changes_neither_side() {
    let mut actor = actor(config());
    actor
        .enqueue(plain(&actor, 1, 100, 1, &[variable()], false))
        .unwrap();
    drain(&mut actor, 100);
    let old = actor.snapshot().clone();
    let mut invalid = (*old).clone();
    invalid.generation = ConfigGeneration(2);
    invalid.complex[0].extras.if_alone[0].target = ActionTarget::SetVariable(VariableAction {
        name: "".into(),
        operation: VariableOperation::Unset,
    });
    assert!(actor.activate(Arc::new(invalid)).is_err());
    assert!(Arc::ptr_eq(actor.snapshot(), &old));
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor.input_up(BatchId(1), MonoTime(110)).unwrap();
    assert_eq!(actor.generated_variables()["v"], VariableValue::Integer(2));
    assert_eq!(actor.executed_variables()["v"], VariableValue::Integer(1));
    drain(&mut actor, 110);
    assert_eq!(actor.executed_variables()["v"], VariableValue::Integer(2));
    assert_eq!(actor.pending_batches(), 0);
}
#[test]
fn full_queue_failed_variable_admission_has_no_generated_assignment_and_same_id_retries() {
    let mut actor = actor(config());
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 1..=8 {
        actor
            .enqueue(plain(&actor, id, 0, 1, &actions, true))
            .unwrap();
    }
    let candidate = plain(&actor, 9, 100, 1, &[variable()], true);
    assert!(matches!(
        actor.enqueue(candidate.clone()),
        Err(OutputStageError::Generated(GeneratedError::Timeline(
            TimelineError::Capacity
        )))
    ));
    assert!(actor.generated_variables().is_empty());
    assert_eq!(actor.pending_batches(), 8);
    assert!(!actor.is_failed());
    actor.cancel_batch(BatchId(1), MonoTime(0)).unwrap();
    actor.enqueue(candidate).unwrap();
    assert_eq!(actor.generated_variables()["v"], VariableValue::Integer(2));
    assert_eq!(actor.pending_batches(), 8);
    drain(&mut actor, 5000);
    assert_eq!(actor.pending_batches(), 0);
    assert_eq!(actor.reserved_slots(), 0);
}
#[test]
fn marker_capacity_rejection_preserves_logical_scope_and_original_up_can_retry() {
    let mut configuration = (*config()).clone();
    configuration.complex[0].to[0].repeat = false;
    let mut actor = actor(Arc::new(configuration));
    shift(&mut actor);
    actor
        .enqueue_scoped(main(&selected(&actor, 0), 2, 100))
        .unwrap();
    drain(&mut actor, 100);
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 3..=10 {
        actor
            .enqueue(plain(&actor, id, 200, 1, &actions, true))
            .unwrap();
    }
    assert_eq!(actor.queued_events(), 8192);
    // Generated Up is empty and prepares; Terminal's extra ScopeUp exceeds capacity.
    assert!(matches!(
        actor.input_up(BatchId(2), MonoTime(110)),
        Err(OutputStageError::Terminal(ActorError::Timeline(
            TimelineError::Capacity
        )))
    ));
    assert_eq!(actor.generated_modifiers(), BTreeSet::new());
    assert!(!actor.is_failed());
    actor.cancel_batch(BatchId(3), MonoTime(105)).unwrap();
    actor.input_up(BatchId(2), MonoTime(110)).unwrap();
    assert_eq!(
        actor.generated_modifiers(),
        BTreeSet::from([key("left_shift")])
    );
    drain(&mut actor, 5000);
    assert_eq!(actor.pending_batches(), 1);
    assert_eq!(actor.reserved_slots(), 1);
}
#[test]
fn empty_and_completed_batches_do_not_leave_credentials_and_ids_survive_reset() {
    let mut actor = actor(config());
    actor.enqueue(plain(&actor, 1, 0, 1, &[], false)).unwrap();
    assert_eq!(actor.pending_batches(), 0);
    assert!(!actor.input_up(BatchId(1), MonoTime(0)).unwrap());
    actor
        .enqueue(plain(&actor, 2, 0, 1, &[key("b").into()], false))
        .unwrap();
    drain(&mut actor, 0);
    let outputs = actor.reset(MonoTime(10)).unwrap();
    assert_eq!(
        outputs
            .iter()
            .map(|o| (o.key.name(), o.edge))
            .collect::<Vec<_>>(),
        [("b".into(), KeyEdge::Up)]
    );
    assert_eq!(actor.pending_batches(), 0);
    assert_eq!(actor.reserved_slots(), 0);
    assert!(matches!(
        actor.enqueue(plain(&actor, 2, 10, 1, &[key("c").into()], false)),
        Err(OutputStageError::Terminal(ActorError::InvalidBatch))
    ));
    actor
        .enqueue(plain(&actor, 3, 10, 1, &[key("c").into()], true))
        .unwrap();
    drain(&mut actor, 10);
    assert_eq!(actor.pending_batches(), 0);
}
#[test]
fn due_work_gate_and_time_reversal_never_close_generated_scope() {
    let mut actor = actor(config());
    shift(&mut actor);
    actor
        .enqueue_scoped(main(&selected(&actor, 0), 2, 100))
        .unwrap();
    assert!(matches!(
        actor.input_up(BatchId(2), MonoTime(100)),
        Err(OutputStageError::Terminal(ActorError::Engine(
            EngineError::DeadlinePending
        )))
    ));
    assert_eq!(
        actor.generated_modifiers(),
        BTreeSet::from([key("right_control")])
    );
    drain(&mut actor, 100);
    assert_eq!(
        actor.input_up(BatchId(2), MonoTime(99)),
        Err(OutputStageError::TimeReversed)
    );
    actor.input_up(BatchId(2), MonoTime(100)).unwrap();
    drain(&mut actor, 100);
    assert_eq!(actor.pending_batches(), 1);
}
#[test]
fn overlapping_same_key_owners_retire_independently_without_unowned_up() {
    let mut actor = actor(config());
    for id in [1, 2] {
        actor
            .enqueue(plain(&actor, id, 0, id, &[key("b").into()], false))
            .unwrap();
    }
    assert_eq!(drain(&mut actor, 0).0, [("b".into(), KeyEdge::Down)]);
    assert!(
        actor
            .cancel_batch(BatchId(1), MonoTime(10))
            .unwrap()
            .is_empty()
    );
    assert_eq!(actor.pending_batches(), 1);
    assert_eq!(actor.reserved_slots(), 1);
    actor.input_up(BatchId(2), MonoTime(11)).unwrap();
    assert_eq!(drain(&mut actor, 11).0, [("b".into(), KeyEdge::Up)]);
    assert_eq!(actor.pending_batches(), 0);
}

#[test]
fn repeat_query_waits_for_last_main_down_and_never_repeats_a_timed_prefix() {
    let mut actor = actor(config());
    let actions = [
        Action {
            hold_down_ns: 50,
            ..key("b").into()
        },
        key("c").into(),
    ];
    actor
        .enqueue(plain(&actor, 1, 0, 1, &actions, false))
        .unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    drain(&mut actor, 0);
    assert_eq!(actor.pressed_outputs(), [key("b")]);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    drain(&mut actor, 50);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("c")));
    let before = (
        actor.pending_batches(),
        actor.reserved_slots(),
        actor.live_slots(),
        actor.queued_events(),
        actor.pressed_outputs(),
    );
    for _ in 0..10_000 {
        assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("c")));
    }
    assert_eq!(
        before,
        (
            actor.pending_batches(),
            actor.reserved_slots(),
            actor.live_slots(),
            actor.queued_events(),
            actor.pressed_outputs()
        )
    );
    actor.input_up(BatchId(1), MonoTime(60)).unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
}
#[test]
fn queued_original_up_stops_repeat_even_while_actual_key_waits_behind_future_work() {
    let mut actor = actor(config());
    actor
        .enqueue(plain(&actor, 1, 0, 1, &[key("b").into()], false))
        .unwrap();
    drain(&mut actor, 0);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("b")));
    let tap = [Action {
        hold_down_ns: 50,
        ..key("c").into()
    }];
    actor
        .enqueue(plain(&actor, 2, 1000, 1, &tap, true))
        .unwrap();
    actor.input_up(BatchId(1), MonoTime(10)).unwrap();
    assert_eq!(actor.pressed_outputs(), [key("b")]);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    drain(&mut actor, 1000);
    assert!(actor.pressed_outputs().contains(&key("c")));
    assert_eq!(actor.physical_repeat_target(BatchId(2)), None); // scheduled tap only
    drain(&mut actor, 1050);
    assert_eq!(actor.pending_batches(), 0);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
}
#[test]
fn repeat_queries_are_independent_for_same_key_owners_and_released_source_is_ineligible() {
    let mut actor = actor(config());
    for id in [1, 2] {
        actor
            .enqueue(plain(&actor, id, 0, id, &[key("b").into()], false))
            .unwrap();
    }
    drain(&mut actor, 0);
    for id in [1, 2] {
        assert_eq!(actor.physical_repeat_target(BatchId(id)), Some(key("b")));
    }
    actor.input_up(BatchId(1), MonoTime(10)).unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    assert_eq!(actor.physical_repeat_target(BatchId(2)), Some(key("b")));
    drain(&mut actor, 10);
    assert_eq!(actor.pressed_outputs(), [key("b")]);
    actor.cancel_batch(BatchId(2), MonoTime(11)).unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(2)), None);
}
#[test]
fn retained_repeat_target_uses_old_binding_after_reload_and_new_batch_uses_new_snapshot() {
    let mut actor = actor(config());
    actor
        .enqueue(plain(&actor, 1, 0, 1, &[key("b").into()], false))
        .unwrap();
    drain(&mut actor, 0);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("b")));
    actor
        .enqueue(plain(&actor, 2, 10, 1, &[key("z").into()], false))
        .unwrap();
    drain(&mut actor, 10);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("b")));
    assert_eq!(actor.physical_repeat_target(BatchId(2)), Some(key("z")));
    assert_eq!(actor.physical_repeat_target(BatchId(99)), None);
}
#[test]
fn modifiers_variables_consumer_buttons_and_repeat_false_are_never_physical_repeat_targets() {
    for action in [
        key("left_shift").into(),
        variable(),
        KeyId::new(12, 0xe9).into(),
        KeyId::new(9, 1).into(),
        Action {
            repeat: false,
            hold_down_ns: 50,
            ..key("b").into()
        },
    ] {
        let mut actor = actor(config());
        actor
            .enqueue(plain(&actor, 1, 0, 1, &[action], false))
            .unwrap();
        drain(&mut actor, 0);
        assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    }
}
#[test]
fn additional_modifiers_never_replace_the_main_repeat_slot_or_recreate_a_shortcut() {
    let mut actor = actor(config());
    let action = Action {
        modifiers: vec![key("left_control")],
        ..key("b").into()
    };
    actor
        .enqueue(plain(&actor, 1, 0, 1, &[action], false))
        .unwrap();
    // Modifier Down precedes main Down; the main slot is still Reserved.
    actor.poll(MonoTime(0)).unwrap().unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    actor.poll(MonoTime(0)).unwrap().unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("b")));
    drain(&mut actor, 0);
    let outputs = actor.pressed_outputs();
    let queued = actor.queued_events();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("b")));
    assert_eq!(actor.pressed_outputs(), outputs);
    assert_eq!(actor.queued_events(), queued);
}
#[test]
fn device_generation_replacement_cancel_and_reset_remove_only_original_repeat_authority() {
    let mut actor = actor(config());
    actor
        .enqueue(plain(&actor, 1, 0, 1, &[key("b").into()], false))
        .unwrap();
    drain(&mut actor, 0);
    let newer = DeviceInstanceId {
        id: 1,
        generation: 2,
    };
    actor.observe_device(newer, false, MonoTime(10)).unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    let plan = plan_actions(
        &[key("c").into()],
        PlanOrigin {
            batch: BatchId(2),
            stage: Stage::Output,
            start: MonoTime(10),
            device: newer,
            source: SourceToken(102),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    actor.enqueue(plan).unwrap();
    drain(&mut actor, 10);
    assert_eq!(actor.physical_repeat_target(BatchId(2)), Some(key("c")));
    actor.reset(MonoTime(20)).unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(2)), None);
}
#[test]
fn selected_consumed_scope_repeat_reads_actual_main_binding_without_restoring_modifiers() {
    let mut configuration = (*config()).clone();
    configuration.complex[0].to = vec![Action {
        modifiers: vec![key("left_control")],
        ..key("b").into()
    }];
    let mut actor = actor(Arc::new(configuration));
    shift(&mut actor);
    actor
        .enqueue_scoped(main(&selected(&actor, 0), 2, 100))
        .unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(2)), None);
    drain(&mut actor, 100);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None); // physical modifier
    let emitted = actor.pressed_outputs();
    for _ in 0..100 {
        assert_eq!(actor.physical_repeat_target(BatchId(2)), Some(key("b")));
    }
    assert_eq!(actor.pressed_outputs(), emitted);
    actor.input_up(BatchId(2), MonoTime(110)).unwrap();
    assert_eq!(actor.physical_repeat_target(BatchId(2)), None);
}
