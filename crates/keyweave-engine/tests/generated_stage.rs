#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*,
    generated_stage::*,
    timeline::{Stage, StageEvent},
    *,
};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn variable(value: i64, up: Option<i64>) -> Action {
    Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(value)),
                key_up_value: up.map(VariableValue::Integer),
            },
        }),
        ..key("b").into()
    }
}
fn mapping(from: &str, target: Action) -> Mapping {
    Mapping {
        id: from.into(),
        from: key(from),
        to: vec![target],
        allow_extra_modifiers: true,
        extras: Default::default(),
    }
}
fn config() -> Arc<ConfigSnapshot> {
    let mut simple = mapping("a", key("b").into());
    simple
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    let mut complex = mapping("b", key("c").into());
    complex
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    let mut var_rule = mapping("d", key("e").into());
    var_rule.extras.conditions.push(Condition::Variable {
        name: "v".into(),
        value: VariableValue::Integer(2),
        unless: false,
    });
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "generated".into(),
        simple: vec![simple],
        complex: vec![complex, var_rule, mapping("x", variable(1, Some(2)))],
    })
}
fn queue(stage: Stage) -> GeneratedStageQueue {
    let mut queue = GeneratedStageQueue::new(stage, config()).unwrap();
    queue.observe_device(device(1), false).unwrap();
    queue.observe_device(device(2), false).unwrap();
    queue
}
fn plan(
    queue: &GeneratedStageQueue,
    stage: Stage,
    id: u64,
    start: u64,
    actions: &[Action],
    tap: bool,
) -> ActionPlan {
    plan_actions(
        actions,
        PlanOrigin {
            batch: BatchId(id),
            stage,
            start: MonoTime(start),
            device: device(1),
            source: SourceToken(100),
            config: queue.snapshot().clone(),
        },
        tap,
    )
    .unwrap()
}
fn select(
    queue: &GeneratedStageQueue,
    name: &str,
) -> Option<keyweave_engine::rule_selection::RuleSelection> {
    queue
        .select(
            key(name),
            &ContextSnapshot::default(),
            device(1),
            MonoTime(0),
        )
        .unwrap()
}
#[test]
fn matching_uses_destination_generated_modifiers_before_due_delivery_and_stages_are_independent() {
    let mut complex = queue(Stage::Complex);
    let output = GeneratedStageQueue::new(Stage::Output, complex.snapshot().clone()).unwrap();
    let modifier = Action {
        lazy: true,
        ..key("left_shift").into()
    };
    let modifier = plan(&complex, Stage::Complex, 1, 100, &[modifier], false);
    complex.enqueue(modifier.events().to_vec()).unwrap();
    assert!(complex.poll(MonoTime(0)).unwrap().is_none());
    assert_eq!(
        complex.modifiers(),
        [key("left_shift")].into_iter().collect()
    );
    let selected = select(&complex, "a").unwrap();
    assert_eq!(selected.mapping().to[0].key(), Some(key("b")));
    assert!(select(&output, "b").is_none());
    assert!(output.modifiers().is_empty());
    assert_eq!(
        complex
            .poll(MonoTime(100))
            .unwrap()
            .unwrap()
            .event
            .payload
            .edge,
        KeyEdge::Down
    );
    assert_eq!(
        complex.modifiers(),
        [key("left_shift")].into_iter().collect()
    );
}
#[test]
fn timed_tap_and_variable_up_change_logical_state_at_generation_without_delivery_replay() {
    let mut queue = queue(Stage::Output);
    let tap = Action {
        hold_down_ns: 50,
        ..key("left_shift").into()
    };
    let actions = [tap, variable(1, Some(2))];
    let plan = plan(&queue, Stage::Output, 1, 100, &actions, true);
    queue.enqueue(plan.events().to_vec()).unwrap();
    assert!(queue.modifiers().is_empty());
    assert_eq!(queue.logical_slots(), 0);
    assert_eq!(queue.variable_values()["v"], VariableValue::Integer(2));
    assert!(select(&queue, "d").is_some());
    queue.poll(MonoTime(100)).unwrap().unwrap();
    assert!(queue.modifiers().is_empty());
    assert_eq!(queue.variable_values()["v"], VariableValue::Integer(2));
    while queue.poll(MonoTime(150)).unwrap().is_some() {}
    assert_eq!(queue.variable_values()["v"], VariableValue::Integer(2));
}
#[test]
fn old_saved_up_survives_activation_but_mismatched_provenance_and_stage_are_rejected_atomically() {
    let mut queue = queue(Stage::Output);
    let plan = plan(
        &queue,
        Stage::Output,
        1,
        0,
        &[key("left_shift").into()],
        false,
    );
    queue.enqueue(plan.events().to_vec()).unwrap();
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let original_up = plan.input_up_events(MonoTime(10));
    for field in 0..4 {
        let mut wrong = original_up.clone();
        match field {
            0 => wrong[0].source = SourceToken(101),
            1 => wrong[0].device = device(2),
            2 => wrong[0].config = queue.snapshot().clone(),
            _ => wrong[0].stage = Stage::Complex,
        }
        assert!(queue.enqueue(wrong).is_err());
        assert_eq!(queue.logical_slots(), 1);
        assert_eq!(queue.queued_events(), 1);
    }
    queue.enqueue(original_up).unwrap();
    assert!(queue.modifiers().is_empty());
    assert_eq!(queue.logical_slots(), 0);
}
#[test]
fn timeline_overflow_aborts_all_prepared_modifier_and_variable_effects_and_returns_original_events()
{
    let mut queue = queue(Stage::Output);
    queue.poll(MonoTime(u64::MAX - 10)).unwrap();
    let actions = [
        variable(1, None),
        Action {
            hold_down_ns: 50,
            ..key("left_shift").into()
        },
    ];
    let plan = plan(&queue, Stage::Output, 1, 0, &actions, true);
    let rejected = queue.enqueue(plan.events().to_vec()).unwrap_err();
    assert_eq!(
        rejected.error,
        GeneratedError::Timeline(keyweave_engine::timeline::TimelineError::TimeOverflow)
    );
    assert_eq!(rejected.events.len(), plan.events().len());
    for (actual, expected) in rejected.events.iter().zip(plan.events()) {
        assert_eq!(actual.payload, expected.payload);
        assert_eq!(actual.stage, expected.stage);
        assert_eq!(actual.at_ns, expected.at_ns);
        assert_eq!(actual.device, expected.device);
        assert_eq!(actual.source, expected.source);
        assert!(Arc::ptr_eq(&actual.config, &expected.config));
    }
    assert!(queue.variable_values().is_empty());
    assert!(queue.modifiers().is_empty());
    assert_eq!(queue.logical_slots(), 0);
    assert_eq!(queue.queued_events(), 0);
    let retry = plan_actions(
        &[key("b").into()],
        PlanOrigin {
            batch: BatchId(1),
            stage: Stage::Output,
            start: MonoTime(0),
            device: device(1),
            source: SourceToken(100),
            config: queue.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    queue.enqueue(retry.events().to_vec()).unwrap();
}
#[test]
fn lifecycle_failure_rolls_back_earlier_assignments_in_same_append() {
    let mut queue = queue(Stage::Output);
    let plan = plan(
        &queue,
        Stage::Output,
        1,
        0,
        &[variable(1, None), key("b").into()],
        false,
    );
    let mut events = plan.events().to_vec();
    let mut duplicate = events[0].clone();
    duplicate.payload.edge = KeyEdge::Down;
    events.push(duplicate);
    assert_eq!(
        queue.enqueue(events).unwrap_err().error,
        GeneratedError::Lifecycle
    );
    assert!(queue.variable_values().is_empty());
    assert_eq!(queue.logical_slots(), 0);
    assert_eq!(queue.queued_events(), 0);
}
#[test]
fn device_cleanup_uses_saved_variable_up_and_retains_other_devices_and_reset_reclaims_old_names() {
    let mut queue = queue(Stage::Output);
    let plan = plan(
        &queue,
        Stage::Output,
        1,
        100,
        &[variable(1, Some(2))],
        false,
    );
    queue.enqueue(plan.events().to_vec()).unwrap();
    let mut other = plan_actions(
        &[key("right_shift").into()],
        PlanOrigin {
            batch: BatchId(2),
            stage: Stage::Output,
            start: MonoTime(100),
            device: device(2),
            source: SourceToken(100),
            config: queue.snapshot().clone(),
        },
        false,
    )
    .unwrap()
    .events()
    .to_vec();
    queue.enqueue(std::mem::take(&mut other)).unwrap();
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let (slots, removed) = queue.observe_device(device(1), true).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(removed.len(), 1);
    assert_eq!(queue.variable_values()["v"], VariableValue::Integer(2));
    assert_eq!(
        queue.modifiers(),
        [key("right_shift")].into_iter().collect()
    );
    assert!(queue.enqueue(plan.events().to_vec()).is_err());
    assert_eq!(queue.reset(MonoTime(0)).unwrap().len(), 1);
    assert_eq!(queue.logical_slots(), 0);
    assert!(queue.variable_values().is_empty());
}
#[test]
fn live_generated_slot_budget_checks_peak_and_rejection_does_not_commit_variable_effects() {
    let mut queue = queue(Stage::Output);
    for base in (0..MAX_OUTPUT_OWNERS).step_by(512) {
        let events: Vec<_> = (base..base + 512)
            .map(|index| StageEvent {
                stage: Stage::Output,
                at_ns: MonoTime(0),
                device: device(1),
                source: SourceToken(100),
                config: queue.snapshot().clone(),
                payload: PlannedAction {
                    slot: ActionSlot {
                        batch: BatchId(index as u64),
                        index: 0,
                    },
                    target: ActionTarget::Key(key("left_shift")),
                    edge: KeyEdge::Down,
                    lazy: true,
                    halt: false,
                },
            })
            .collect();
        queue.enqueue(events).unwrap();
        while queue.poll(MonoTime(0)).unwrap().is_some() {}
    }
    assert_eq!(queue.logical_slots(), MAX_OUTPUT_OWNERS);
    let extra = plan(
        &queue,
        Stage::Output,
        MAX_OUTPUT_OWNERS as u64 + 1,
        0,
        &[variable(1, None)],
        true,
    );
    assert_eq!(
        queue.enqueue(extra.events().to_vec()).unwrap_err().error,
        GeneratedError::Engine(EngineError::ResourceLimit)
    );
    assert!(queue.variable_values().is_empty());
    assert_eq!(queue.logical_slots(), MAX_OUTPUT_OWNERS);
    queue.reset(MonoTime(0)).unwrap();
    assert_eq!(queue.logical_slots(), 0);
}
