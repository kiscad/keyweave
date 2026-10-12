#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, complex_actor::*, timeline::*, *};
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
fn config(to: Vec<Action>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "owned-complex".into(),
        simple: vec![],
        complex: vec![mapping("a", to)],
    })
}
fn actor(config: Arc<ConfigSnapshot>) -> ComplexStageActor {
    let mut actor = ComplexStageActor::new(config).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor.observe_device(device(2), false, MonoTime(0)).unwrap();
    actor
}
fn input(actor: &mut ComplexStageActor, actions: &[Action], device_id: u64, at: u64) -> ActionPlan {
    let id = actor.next_batch_id().unwrap();
    let plan = plan_actions(
        actions,
        PlanOrigin {
            batch: id,
            stage: Stage::Complex,
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
fn down(plan: &ActionPlan, index: u16, at: u64) -> DueEvent<PlannedAction> {
    DueEvent {
        event: plan
            .events()
            .iter()
            .find(|e| e.payload.edge == KeyEdge::Down && e.payload.slot.index == index)
            .unwrap()
            .clone(),
        delivered_at: MonoTime(at),
    }
}
fn up(plan: &ActionPlan, index: u16, at: u64) -> DueEvent<PlannedAction> {
    let event = plan
        .input_up_events(MonoTime(at))
        .into_iter()
        .find(|e| e.payload.slot.index == index)
        .or_else(|| {
            plan.events()
                .iter()
                .find(|e| e.payload.edge == KeyEdge::Up && e.payload.slot.index == index)
                .cloned()
        })
        .unwrap();
    DueEvent {
        event,
        delivered_at: MonoTime(at),
    }
}
fn drain(actor: &mut ComplexStageActor, at: u64) -> (Vec<(String, KeyEdge)>, Vec<ActionSlot>) {
    let mut outputs = vec![];
    let mut released = vec![];
    while let Some(step) = actor.poll(MonoTime(at)).unwrap() {
        outputs.extend(step.outputs.into_iter().map(|o| (o.key.name(), o.edge)));
        released.extend(step.released_producers);
    }
    (outputs, released)
}
#[test]
fn last_control_reference_restores_source_ahead_of_due_held_without_executing_branch() {
    let mut cfg = (*config(vec![key("b").into()])).clone();
    cfg.complex[0].extras.if_held_down = vec![key("c").into()];
    cfg.complex[0].extras.held_threshold_ns = 50;
    let mut actor = actor(Arc::new(cfg));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let first = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    let second = actor
        .begin_descendant_projection(slot, MonoTime(95))
        .unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    assert!(
        actor
            .restore_descendant_projection_control(first, MonoTime(150))
            .unwrap()
            .outputs
            .is_empty()
    );
    let restored = actor
        .restore_descendant_projection_control(second, MonoTime(150))
        .unwrap();
    assert_eq!(
        restored
            .outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("b"), KeyEdge::Down)]
    );
    assert_eq!(actor.next_deadline(), Some(MonoTime(150)));
    assert_eq!(
        drain(&mut actor, 150).0,
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
    actor
        .admit(&up(&plan, 0, 160), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 160).0, [("c".into(), KeyEdge::Up)]);
}
#[test]
fn clearing_reserved_parent_template_makes_future_children_visible_with_original_identity() {
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let id = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    assert!(
        actor
            .restore_descendant_projection_control(id, MonoTime(95))
            .unwrap()
            .outputs
            .is_empty()
    );
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Down)]);
    assert!(
        actor
            .restore_descendant_projection_control(id, MonoTime(110))
            .is_err()
    );
    assert!(!actor.is_failed());
}
#[test]
fn unrelated_device_removal_keeps_saved_source_restorable_and_original_up_valid() {
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let id = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    actor
        .observe_device(device(2), true, MonoTime(110))
        .unwrap();
    let restored = actor
        .restore_descendant_projection_control(id, MonoTime(110))
        .unwrap();
    assert_eq!(
        restored
            .outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("b"), KeyEdge::Down)]
    );
    actor
        .admit(&up(&plan, 0, 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 120).0, [("b".into(), KeyEdge::Up)]);
}
