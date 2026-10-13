#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, simple_actor::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn mapping(from: &str, to: Vec<Action>) -> Mapping {
    Mapping {
        id: from.into(),
        from: key(from),
        to,
        allow_extra_modifiers: true,
        extras: Default::default(),
    }
}
fn config(simple: Vec<Mapping>, complex: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "owned-simple".into(),
        simple,
        complex,
    })
}
fn actor(config: Arc<ConfigSnapshot>) -> SimpleStageActor {
    let mut actor = SimpleStageActor::new(config).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor.observe_device(device(2), false, MonoTime(0)).unwrap();
    actor
}
fn input(actor: &mut SimpleStageActor, actions: &[Action], device_id: u64, at: u64) -> ActionPlan {
    let id = actor.next_batch_id().unwrap();
    let plan = plan_actions(
        actions,
        PlanOrigin {
            batch: id,
            stage: Stage::Simple,
            start: MonoTime(at),
            device: device(device_id),
            source: SourceToken(id.0),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    actor.register_input(plan.clone()).unwrap();
    plan
}
fn down(plan: &ActionPlan, at: u64) -> DueEvent<PlannedAction> {
    DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(at),
    }
}
fn up(plan: &ActionPlan, at: u64) -> DueEvent<PlannedAction> {
    DueEvent {
        event: plan
            .input_up_events(MonoTime(at))
            .into_iter()
            .next()
            .unwrap(),
        delivered_at: MonoTime(at),
    }
}
fn drain(actor: &mut SimpleStageActor, at: u64) -> (Vec<(KeyId, KeyEdge)>, Vec<ActionSlot>) {
    let mut outputs = vec![];
    let mut released = vec![];
    for _ in 0..10000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return (outputs, released);
        };
        outputs.extend(step.outputs.into_iter().map(|o| (o.key, o.edge)));
        released.extend(step.released_producers);
    }
    panic!("poll failed to finish bounded fixture");
}
fn edges(outputs: Vec<OutputIntent>) -> Vec<(KeyId, KeyEdge)> {
    outputs.into_iter().map(|o| (o.key, o.edge)).collect()
}
fn scoped_config() -> Arc<ConfigSnapshot> {
    let mut trigger = mapping("a", vec![key("right_control").into()]);
    trigger
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    config(
        vec![trigger],
        vec![
            mapping("left_shift", vec![key("x").into()]),
            mapping("right_shift", vec![key("y").into()]),
        ],
    )
}
fn press(
    actor: &mut SimpleStageActor,
    name: &str,
    dev: u64,
    at: u64,
) -> (ActionPlan, Vec<(KeyId, KeyEdge)>) {
    let plan = input(actor, &[key(name).into()], dev, at);
    let step = actor
        .admit(&down(&plan, at), &ContextSnapshot::default())
        .unwrap();
    let mut outputs = edges(step.outputs);
    outputs.extend(drain(actor, at).0);
    (plan, outputs)
}
fn release(actor: &mut SimpleStageActor, plan: &ActionPlan, at: u64) -> Vec<(KeyId, KeyEdge)> {
    let step = actor
        .admit(&up(plan, at), &ContextSnapshot::default())
        .unwrap();
    let mut outputs = edges(step.outputs);
    outputs.extend(drain(actor, at).0);
    outputs
}
#[test]
fn consumed_modifier_hides_ordinary_remapped_descendant_and_saved_close_restores_without_reselection()
 {
    let mut actor = actor(scoped_config());
    let (shift, outputs) = press(&mut actor, "left_shift", 1, 100);
    assert_eq!(outputs, [(key("x"), KeyEdge::Down)]);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [
            (key("x"), KeyEdge::Up),
            (key("right_control"), KeyEdge::Down)
        ]
    );
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(release(&mut actor, &shift, 130), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn two_owners_of_same_modifier_are_projected_in_one_group_and_close_is_not_blocked_by_first_new_marker()
 {
    let mut actor = actor(scoped_config());
    let (left, _) = press(&mut actor, "left_shift", 1, 100);
    let (right, _) = press(&mut actor, "left_shift", 2, 105);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [
            (key("x"), KeyEdge::Up),
            (key("right_control"), KeyEdge::Down)
        ]
    );
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert!(release(&mut actor, &left, 130).is_empty());
    assert_eq!(release(&mut actor, &right, 140), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn trigger_device_removal_restores_other_device_source_in_returned_control_intents() {
    let mut actor = actor(scoped_config());
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    press(&mut actor, "a", 2, 110);
    let step = actor
        .observe_device(device(2), true, MonoTime(120))
        .unwrap();
    assert_eq!(
        edges(step.outputs),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert!(drain(&mut actor, 120).0.is_empty());
    assert_eq!(release(&mut actor, &shift, 130), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.pending_children(), 0);
}
#[test]
fn removed_hidden_source_is_pruned_but_trigger_close_remains_valid() {
    let mut actor = actor(scoped_config());
    press(&mut actor, "left_shift", 1, 100);
    let (trigger, _) = press(&mut actor, "a", 2, 110);
    assert!(
        actor
            .observe_device(device(1), true, MonoTime(115))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [(key("right_control"), KeyEdge::Up)]
    );
    assert!(!actor.is_failed());
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn hidden_source_original_up_retires_without_revival_when_trigger_closes_later() {
    let mut actor = actor(scoped_config());
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (trigger, _) = press(&mut actor, "a", 2, 110);
    assert!(release(&mut actor, &shift, 115).is_empty());
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [(key("right_control"), KeyEdge::Up)]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn hot_reload_keeps_original_scope_target_and_original_trigger_up() {
    let mut actor = actor(scoped_config());
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (trigger, _) = press(&mut actor, "a", 2, 110);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(release(&mut actor, &shift, 130), [(key("x"), KeyEdge::Up)]);
}
#[test]
fn transient_main_scope_closes_after_generated_down_but_root_waits_original_up() {
    let mut cfg = (*scoped_config()).clone();
    cfg.simple[0].to = vec![key("b").into()];
    let mut actor = actor(Arc::new(cfg));
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [
            (key("x"), KeyEdge::Up),
            (key("b"), KeyEdge::Down),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [(key("b"), KeyEdge::Up)]
    );
    assert_eq!(release(&mut actor, &shift, 130), [(key("x"), KeyEdge::Up)]);
}
#[test]
fn reset_discards_all_scope_references_and_later_physical_inputs_can_reuse_devices() {
    let mut actor = actor(scoped_config());
    press(&mut actor, "left_shift", 1, 100);
    press(&mut actor, "a", 2, 110);
    let reset = actor.reset(MonoTime(120)).unwrap();
    assert_eq!(edges(reset.outputs), [(key("right_control"), KeyEdge::Up)]);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.pending_children(), 0);
    let (_, outputs) = press(&mut actor, "left_shift", 1, 130);
    assert_eq!(outputs, [(key("x"), KeyEdge::Down)]);
    let (trigger, _) = press(&mut actor, "a", 2, 140);
    assert_eq!(
        release(&mut actor, &trigger, 150),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
}
#[test]
fn trigger_down_cancels_source_held_task_while_scope_retains_original_main() {
    let mut cfg = (*scoped_config()).clone();
    cfg.complex[0].extras.if_held_down = vec![key("z").into()];
    cfg.complex[0].extras.held_threshold_ns = 50;
    let mut actor = actor(Arc::new(cfg));
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (trigger, _) = press(&mut actor, "a", 2, 110);
    // A new Complex Down cancels the stage's previous held task.
    assert!(drain(&mut actor, 150).0.is_empty());
    assert_eq!(
        release(&mut actor, &trigger, 160),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(release(&mut actor, &shift, 170), [(key("x"), KeyEdge::Up)]);
}
#[test]
fn opposite_modifier_side_not_consumed_by_generic_shift_preserves_its_other_target() {
    let mut actor = actor(scoped_config());
    let (left, _) = press(&mut actor, "left_shift", 1, 100);
    let (right, _) = press(&mut actor, "right_shift", 1, 105);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [
            (key("x"), KeyEdge::Up),
            (key("right_control"), KeyEdge::Down)
        ]
    );
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(release(&mut actor, &right, 130), [(key("y"), KeyEdge::Up)]);
    release(&mut actor, &left, 140);
}
#[test]
fn variable_descendant_pause_affects_trigger_condition_and_close_restores_original_value() {
    let mut cfg = (*scoped_config()).clone();
    cfg.complex[0].to = vec![Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        ..key("vk_none").into()
    }];
    let when = |value| Condition::Variable {
        name: "v".into(),
        value: VariableValue::Integer(value),
        unless: false,
    };
    cfg.complex.push(mapping(
        "right_control",
        vec![Action {
            conditions: vec![when(2)],
            ..key("z").into()
        }],
    ));
    cfg.complex.push(mapping(
        "b",
        vec![Action {
            conditions: vec![when(1)],
            ..key("q").into()
        }],
    ));
    let mut actor = actor(Arc::new(cfg));
    let (shift, outputs) = press(&mut actor, "left_shift", 1, 100);
    assert!(outputs.is_empty());
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(outputs, [(key("z"), KeyEdge::Down)]);
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [(key("z"), KeyEdge::Up)]
    );
    let (probe, outputs) = press(&mut actor, "b", 2, 130);
    assert_eq!(outputs, [(key("q"), KeyEdge::Down)]);
    release(&mut actor, &probe, 140);
    assert!(release(&mut actor, &shift, 150).is_empty());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn interleaved_scope_source_up_closes_its_own_scope_without_reviving_target_hidden_by_later_scope()
{
    let mut cfg = (*scoped_config()).clone();
    let mut second = mapping("b", vec![key("right_alt").into()]);
    second
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::RightControl);
    cfg.simple.push(second);
    cfg.complex
        .push(mapping("right_control", vec![key("z").into()]));
    let mut actor = actor(Arc::new(cfg));
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (first, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [(key("x"), KeyEdge::Up), (key("z"), KeyEdge::Down)]
    );
    let (second, outputs) = press(&mut actor, "b", 2, 120);
    assert_eq!(
        outputs,
        [(key("z"), KeyEdge::Up), (key("right_alt"), KeyEdge::Down)]
    );
    assert_eq!(
        release(&mut actor, &first, 130),
        [(key("x"), KeyEdge::Down)]
    );
    assert_eq!(
        release(&mut actor, &second, 140),
        [(key("right_alt"), KeyEdge::Up)]
    );
    assert_eq!(release(&mut actor, &shift, 150), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn new_source_generation_is_not_hidden_by_old_template_after_source_device_replacement() {
    let mut actor = actor(scoped_config());
    press(&mut actor, "left_shift", 1, 100);
    let (trigger, _) = press(&mut actor, "a", 2, 110);
    let new_device = DeviceInstanceId {
        id: 1,
        generation: 2,
    };
    assert!(
        actor
            .observe_device(new_device, false, MonoTime(115))
            .unwrap()
            .outputs
            .is_empty()
    );
    let id = actor.next_batch_id().unwrap();
    let new = plan_actions(
        &[key("left_shift").into()],
        PlanOrigin {
            batch: id,
            stage: Stage::Simple,
            start: MonoTime(116),
            device: new_device,
            source: SourceToken(id.0),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    actor.register_input(new.clone()).unwrap();
    actor
        .admit(&down(&new, 116), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 116).0, [(key("x"), KeyEdge::Down)]);
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [(key("right_control"), KeyEdge::Up)]
    );
    assert_eq!(release(&mut actor, &new, 130), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn trigger_removal_before_queued_scope_close_restores_other_source_and_discards_reserved_future_modifier()
 {
    let mut cfg = (*scoped_config()).clone();
    cfg.simple[0].to.insert(
        0,
        Action {
            hold_down_ns: 50,
            ..key("b").into()
        },
    );
    let mut actor = actor(Arc::new(cfg));
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [(key("x"), KeyEdge::Up), (key("b"), KeyEdge::Down)]
    );
    assert!(release(&mut actor, &trigger, 120).is_empty());
    assert_eq!(
        edges(
            actor
                .observe_device(device(2), true, MonoTime(130))
                .unwrap()
                .outputs
        ),
        [(key("b"), KeyEdge::Up), (key("x"), KeyEdge::Down)]
    );
    assert!(drain(&mut actor, 160).0.is_empty());
    assert_eq!(release(&mut actor, &shift, 170), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn source_removal_before_delayed_scope_open_uses_delivery_ledger_even_after_logical_source_up() {
    let mut cfg = (*scoped_config()).clone();
    cfg.simple.push(mapping(
        "p",
        vec![
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            key("q").into(),
        ],
    ));
    let mut actor = actor(Arc::new(cfg));
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (prior, outputs) = press(&mut actor, "p", 2, 105);
    assert_eq!(outputs, [(key("b"), KeyEdge::Down)]);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert!(outputs.is_empty()); // its scope open is behind the prior array's tail
    assert!(release(&mut actor, &shift, 111).is_empty()); // logical Up, actual source still retained
    assert_eq!(
        edges(
            actor
                .observe_device(device(1), true, MonoTime(120))
                .unwrap()
                .outputs
        ),
        [(key("x"), KeyEdge::Up)]
    );
    assert_eq!(
        drain(&mut actor, 155).0,
        [
            (key("b"), KeyEdge::Up),
            (key("q"), KeyEdge::Down),
            (key("right_control"), KeyEdge::Down)
        ]
    );
    assert_eq!(
        release(&mut actor, &trigger, 160),
        [(key("right_control"), KeyEdge::Up)]
    );
    assert_eq!(release(&mut actor, &prior, 170), [(key("q"), KeyEdge::Up)]);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn trigger_removal_before_scope_open_discards_markers_without_duplicate_restore_or_future_fake_up()
{
    let mut cfg = (*scoped_config()).clone();
    cfg.simple.push(mapping(
        "p",
        vec![
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            key("q").into(),
        ],
    ));
    let mut actor = actor(Arc::new(cfg));
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    press(&mut actor, "p", 2, 105);
    let (_, outputs) = press(&mut actor, "a", 2, 110);
    assert!(outputs.is_empty());
    assert_eq!(
        edges(
            actor
                .observe_device(device(2), true, MonoTime(120))
                .unwrap()
                .outputs
        ),
        [(key("b"), KeyEdge::Up)]
    );
    assert!(actor.generated_modifiers().contains(&key("left_shift")));
    assert!(drain(&mut actor, 155).0.is_empty());
    assert_eq!(release(&mut actor, &shift, 160), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn after_up_scope_hides_saved_modifier_descendant_for_entire_timed_branch() {
    let mut trigger = mapping("a", vec![]);
    trigger
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::LeftShift);
    trigger.extras.after_key_up = vec![Action {
        hold_down_ns: 50,
        ..key("d").into()
    }];
    let mut actor = actor(config(
        vec![trigger],
        vec![mapping("left_shift", vec![key("x").into()])],
    ));
    let (shift, outputs) = press(&mut actor, "left_shift", 1, 100);
    assert_eq!(outputs, [(key("x"), KeyEdge::Down)]);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert!(outputs.is_empty());
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [(key("x"), KeyEdge::Up), (key("d"), KeyEdge::Down)]
    );
    assert_eq!(actor.pending_inputs(), 2);
    assert!(drain(&mut actor, 169).0.is_empty());
    let (outputs, released) = drain(&mut actor, 170);
    assert_eq!(
        outputs,
        [(key("d"), KeyEdge::Up), (key("x"), KeyEdge::Down)]
    );
    assert_eq!(released, [trigger.events()[0].payload.slot]);
    assert_eq!(release(&mut actor, &shift, 180), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn held_modifier_replacement_keeps_consumed_descendant_hidden_until_original_up() {
    let mut trigger = mapping("a", vec![key("b").into()]);
    trigger
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::LeftShift);
    trigger.extras.if_held_down = vec![key("right_control").into()];
    trigger.extras.held_threshold_ns = 50;
    let mut actor = actor(config(
        vec![trigger],
        vec![mapping("left_shift", vec![key("x").into()])],
    ));
    let (shift, outputs) = press(&mut actor, "left_shift", 1, 100);
    assert_eq!(outputs, [(key("x"), KeyEdge::Down)]);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [
            (key("x"), KeyEdge::Up),
            (key("b"), KeyEdge::Down),
            (key("x"), KeyEdge::Down)
        ]
    );
    let outputs = drain(&mut actor, 160).0;
    assert_eq!(
        outputs,
        [
            (key("b"), KeyEdge::Up),
            (key("x"), KeyEdge::Up),
            (key("right_control"), KeyEdge::Down)
        ]
    );
    assert_eq!(
        release(&mut actor, &trigger, 170),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(release(&mut actor, &shift, 180), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn alone_snapshot_keeps_present_saved_modifier_without_duplicate_complex_remap() {
    let mut trigger = mapping("a", vec![]);
    trigger.extras.if_alone = vec![key("d").into()];
    let mut actor = actor(config(
        vec![trigger],
        vec![mapping("left_shift", vec![key("x").into()])],
    ));
    let (shift, outputs) = press(&mut actor, "left_shift", 1, 100);
    assert_eq!(outputs, [(key("x"), KeyEdge::Down)]);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert!(outputs.is_empty());
    assert_eq!(
        release(&mut actor, &trigger, 120),
        [(key("d"), KeyEdge::Down), (key("d"), KeyEdge::Up)]
    );
    assert_eq!(release(&mut actor, &shift, 130), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn alone_missing_snapshot_modifier_passes_through_old_complex_remap_for_whole_array() {
    let mut trigger = mapping("a", vec![]);
    trigger.extras.if_alone = vec![Action {
        hold_down_ns: 50,
        ..key("d").into()
    }];
    let mut actor = actor(config(
        vec![trigger],
        vec![
            mapping("left_shift", vec![key("x").into()]),
            mapping("d", vec![key("e").into()]),
        ],
    ));
    let (shift, outputs) = press(&mut actor, "left_shift", 1, 100);
    assert_eq!(outputs, [(key("x"), KeyEdge::Down)]);
    let (trigger, outputs) = press(&mut actor, "a", 2, 110);
    assert!(outputs.is_empty());
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert_eq!(release(&mut actor, &shift, 120), [(key("x"), KeyEdge::Up)]);
    assert_eq!(
        release(&mut actor, &trigger, 130),
        [(key("x"), KeyEdge::Down), (key("e"), KeyEdge::Down)]
    );
    assert_eq!(actor.pending_inputs(), 1);
    assert!(drain(&mut actor, 179).0.is_empty());
    let (outputs, released) = drain(&mut actor, 180);
    assert_eq!(outputs, [(key("e"), KeyEdge::Up), (key("x"), KeyEdge::Up)]);
    assert_eq!(released, [trigger.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn alone_snapshot_projects_newly_restored_outside_modifier_and_restores_actual_descendant() {
    let mut held = mapping("a", vec![key("right_control").into()]);
    held.extras
        .from_modifiers
        .mandatory
        .insert(Modifier::LeftShift);
    let mut trigger = mapping("b", vec![]);
    trigger.extras.if_alone = vec![Action {
        hold_down_ns: 50,
        ..key("d").into()
    }];
    let mut actor = actor(config(
        vec![held, trigger],
        vec![mapping("left_shift", vec![key("x").into()])],
    ));
    let (shift, _) = press(&mut actor, "left_shift", 1, 100);
    let (held, outputs) = press(&mut actor, "a", 2, 110);
    assert_eq!(
        outputs,
        [
            (key("x"), KeyEdge::Up),
            (key("right_control"), KeyEdge::Down)
        ]
    );
    let (trigger, outputs) = press(&mut actor, "b", 2, 120);
    assert!(outputs.is_empty());
    assert_eq!(
        release(&mut actor, &held, 130),
        [
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(
        release(&mut actor, &trigger, 140),
        [
            (key("x"), KeyEdge::Up),
            (key("right_control"), KeyEdge::Down),
            (key("d"), KeyEdge::Down)
        ]
    );
    let (outputs, released) = drain(&mut actor, 190);
    assert_eq!(
        outputs,
        [
            (key("d"), KeyEdge::Up),
            (key("right_control"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    assert_eq!(released, [trigger.events()[0].payload.slot]);
    assert_eq!(release(&mut actor, &shift, 200), [(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}

fn drain_with_fresh_context(
    actor: &mut SimpleStageActor,
    at: u64,
    context: &ContextSnapshot,
) -> Vec<(KeyId, KeyEdge)> {
    let mut outputs = vec![];
    for _ in 0..10000 {
        let Some(step) = actor.poll_with_context(MonoTime(at), context).unwrap() else {
            return outputs;
        };
        outputs.extend(edges(step.outputs));
    }
    panic!("bounded context fixture did not drain");
}
fn known_device_context() -> ContextSnapshot {
    ContextSnapshot {
        revision: 1,
        provider: "synthetic fixture".into(),
        observed_at_ns: MonoTime(0),
        valid_until_ns: MonoTime(1000),
        devices: Some(vec![
            DeviceContext {
                device: device(1),
                properties: DeviceProperties {
                    vendor_id: Some(123),
                    ..Default::default()
                },
            },
            DeviceContext {
                device: device(2),
                properties: DeviceProperties {
                    vendor_id: Some(123),
                    ..Default::default()
                },
            },
        ]),
        ..Default::default()
    }
}
#[test]
fn synthetic_snapshot_modifiers_have_unknown_event_device_but_inventory_exists_still_works() {
    for (unless, exists, vendor) in [(false, false, 123), (true, false, 999), (false, true, 123)] {
        let mut trigger = mapping("a", vec![]);
        trigger.extras.if_alone = vec![key("d").into()];
        let mut conditional = mapping("left_shift", vec![key("x").into()]);
        conditional.extras.conditions = vec![Condition::Device {
            identifiers: vec![DeviceProperties {
                vendor_id: Some(vendor),
                ..Default::default()
            }],
            unless,
            exists,
        }];
        let mut actor = actor(config(
            vec![trigger],
            vec![conditional, mapping("left_shift", vec![key("y").into()])],
        ));
        let context = known_device_context();
        let shift = input(&mut actor, &[key("left_shift").into()], 1, 100);
        actor.admit(&down(&shift, 100), &context).unwrap();
        assert_eq!(
            drain_with_fresh_context(&mut actor, 100, &context),
            [(key("x"), KeyEdge::Down)]
        );
        let trigger = input(&mut actor, &[key("a").into()], 2, 110);
        actor.admit(&down(&trigger, 110), &context).unwrap();
        drain_with_fresh_context(&mut actor, 110, &context);
        actor.admit(&up(&shift, 120), &context).unwrap();
        assert_eq!(
            drain_with_fresh_context(&mut actor, 120, &context),
            [(key("x"), KeyEdge::Up)]
        );
        actor.admit(&up(&trigger, 130), &context).unwrap();
        let restored = key(if exists { "x" } else { "y" });
        assert_eq!(
            drain_with_fresh_context(&mut actor, 130, &context),
            [
                (restored, KeyEdge::Down),
                (key("d"), KeyEdge::Down),
                (key("d"), KeyEdge::Up),
                (restored, KeyEdge::Up)
            ]
        );
        assert_eq!(actor.reserved_sources(), 0);
    }
}

#[test]
fn snapshot_modifier_saved_complex_after_up_retains_unknown_device_identity() {
    let mut trigger = mapping("a", vec![]);
    trigger.extras.if_alone = vec![Action {
        hold_down_ns: 10,
        ..key("d").into()
    }];
    let mut shift_rule = mapping("left_shift", vec![key("x").into()]);
    shift_rule.extras.after_key_up = vec![
        Action {
            conditions: vec![Condition::Device {
                identifiers: vec![DeviceProperties {
                    vendor_id: Some(123),
                    ..Default::default()
                }],
                unless: false,
                exists: false,
            }],
            ..key("z").into()
        },
        key("y").into(),
    ];
    let mut actor = actor(config(vec![trigger], vec![shift_rule]));
    let context = known_device_context();
    let shift = input(&mut actor, &[key("left_shift").into()], 1, 100);
    actor.admit(&down(&shift, 100), &context).unwrap();
    drain_with_fresh_context(&mut actor, 100, &context);
    let trigger = input(&mut actor, &[key("a").into()], 2, 110);
    actor.admit(&down(&trigger, 110), &context).unwrap();
    drain_with_fresh_context(&mut actor, 110, &context);
    actor.admit(&up(&shift, 120), &context).unwrap();
    assert_eq!(
        drain_with_fresh_context(&mut actor, 120, &context),
        [
            (key("x"), KeyEdge::Up),
            (key("z"), KeyEdge::Down),
            (key("z"), KeyEdge::Up),
            (key("y"), KeyEdge::Down),
            (key("y"), KeyEdge::Up)
        ]
    );
    actor.admit(&up(&trigger, 130), &context).unwrap();
    assert_eq!(
        drain_with_fresh_context(&mut actor, 130, &context),
        [(key("x"), KeyEdge::Down), (key("d"), KeyEdge::Down)]
    );
    assert_eq!(
        drain_with_fresh_context(&mut actor, 140, &context),
        [
            (key("d"), KeyEdge::Up),
            (key("x"), KeyEdge::Up),
            (key("y"), KeyEdge::Down),
            (key("y"), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
