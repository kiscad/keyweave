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
fn reserved_parent_main_is_hidden_from_birth_and_final_reference_restores_saved_source() {
    let mut actor = actor(config(vec![key("b").into()]));
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
    assert_eq!(actor.physical_repeat_target(slot), None);
    actor
        .close_descendant_projection(first, MonoTime(110))
        .unwrap();
    assert!(drain(&mut actor, 110).0.is_empty());
    actor
        .close_descendant_projection(second, MonoTime(120))
        .unwrap();
    assert_eq!(drain(&mut actor, 120).0, [("b".into(), KeyEdge::Down)]);
    actor
        .admit(&up(&plan, 0, 130), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 130).0, [("b".into(), KeyEdge::Up)]);
}
#[test]
fn existing_main_and_held_replacement_share_template_and_only_live_held_source_restores() {
    let mut cfg = (*config(vec![key("b").into()])).clone();
    cfg.complex[0].extras.if_held_down = vec![key("c").into()];
    cfg.complex[0].extras.held_threshold_ns = 50;
    let mut actor = actor(Arc::new(cfg));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Down)]);
    let projection = actor
        .begin_descendant_projection(slot, MonoTime(110))
        .unwrap();
    assert_eq!(drain(&mut actor, 110).0, [("b".into(), KeyEdge::Up)]);
    assert!(drain(&mut actor, 150).0.is_empty());
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor
        .close_descendant_projection(projection, MonoTime(160))
        .unwrap();
    assert_eq!(drain(&mut actor, 160).0, [("c".into(), KeyEdge::Down)]);
    actor
        .admit(&up(&plan, 0, 170), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 170).0, [("c".into(), KeyEdge::Up)]);
}
#[test]
fn empty_main_delayed_after_original_up_remains_hidden_and_retired_parent_can_close() {
    let mut cfg = (*config(vec![])).clone();
    cfg.complex[0].extras.delayed = Some(DelayedAction {
        if_invoked: vec![Action {
            hold_down_ns: 10,
            ..key("c").into()
        }],
        if_canceled: vec![],
        delay_ns: 50,
    });
    let mut actor = actor(Arc::new(cfg));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let projection = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 150).0.is_empty());
    let (outputs, released) = drain(&mut actor, 160);
    assert!(outputs.is_empty());
    assert_eq!(released, [slot]);
    assert_eq!(actor.pending_inputs(), 0);
    actor
        .close_descendant_projection(projection, MonoTime(170))
        .unwrap();
    assert!(drain(&mut actor, 170).0.is_empty());
}
#[test]
fn alone_and_after_up_taps_inherit_and_close_does_not_replay_finished_actions() {
    let mut cfg = (*config(vec![])).clone();
    cfg.complex[0].extras.if_alone = vec![Action {
        hold_down_ns: 10,
        ..key("b").into()
    }];
    cfg.complex[0].extras.after_key_up = vec![Action {
        hold_down_ns: 10,
        ..key("c").into()
    }];
    let mut actor = actor(Arc::new(cfg));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let projection = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 150).0.is_empty());
    actor
        .close_descendant_projection(projection, MonoTime(160))
        .unwrap();
    assert!(drain(&mut actor, 160).0.is_empty());
}
#[test]
fn removed_generation_invalidates_template_after_parent_retirement_without_touching_other_device() {
    let mut actor = actor(config(vec![]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let projection = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.pending_inputs(), 0);
    actor
        .observe_device(device(2), true, MonoTime(120))
        .unwrap();
    let extra = actor
        .begin_descendant_projection(slot, MonoTime(125))
        .unwrap();
    actor
        .close_descendant_projection(extra, MonoTime(126))
        .unwrap();
    actor
        .observe_device(device(1), true, MonoTime(130))
        .unwrap();
    assert!(matches!(
        actor.close_descendant_projection(projection, MonoTime(140)),
        Err(ComplexActorError::Input)
    ));
    assert!(!actor.is_failed());
}
#[test]
fn reset_invalidates_tokens_and_new_template_identity_is_not_reused() {
    let mut actor = actor(config(vec![]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let first = actor
        .begin_descendant_projection(down(&plan, 0, 100).event.payload.slot, MonoTime(90))
        .unwrap();
    actor.reset(MonoTime(95)).unwrap();
    let next = input(&mut actor, &[key("a").into()], 1, 100);
    let second = actor
        .begin_descendant_projection(down(&next, 0, 100).event.payload.slot, MonoTime(96))
        .unwrap();
    assert_ne!(first, second);
    assert!(matches!(
        actor.close_descendant_projection(first, MonoTime(97)),
        Err(ComplexActorError::Input)
    ));
    actor
        .close_descendant_projection(second, MonoTime(98))
        .unwrap();
}
#[test]
fn due_preflight_rejection_preserves_reference_for_retry_without_latching() {
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let projection = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(matches!(
        actor.close_descendant_projection(projection, MonoTime(100)),
        Err(ComplexActorError::DeadlinePending)
    ));
    assert!(!actor.is_failed());
    assert!(drain(&mut actor, 100).0.is_empty());
    actor
        .close_descendant_projection(projection, MonoTime(100))
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Down)]);
}
#[test]
fn reference_capacity_rejection_is_cold_and_closing_one_reference_allows_retry() {
    let mut actor = actor(config(vec![]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    let mut handles = Vec::new();
    for _ in 0..MAX_BINDINGS {
        handles.push(
            actor
                .begin_descendant_projection(slot, MonoTime(90))
                .unwrap(),
        );
    }
    assert!(matches!(
        actor.begin_descendant_projection(slot, MonoTime(90)),
        Err(ComplexActorError::Binding(
            keyweave_engine::stage_bindings::BindingError::Capacity
        ))
    ));
    assert!(!actor.is_failed());
    let old = handles.pop().unwrap();
    actor
        .close_descendant_projection(old, MonoTime(90))
        .unwrap();
    let new = actor
        .begin_descendant_projection(slot, MonoTime(90))
        .unwrap();
    assert_ne!(old, new);
    for handle in handles {
        actor
            .close_descendant_projection(handle, MonoTime(90))
            .unwrap();
    }
    actor
        .close_descendant_projection(new, MonoTime(90))
        .unwrap();
}
#[test]
fn delayed_canceled_branch_inherits_while_new_unrelated_parent_remains_visible() {
    let mut cfg = (*config(vec![])).clone();
    cfg.complex[0].extras.delayed = Some(DelayedAction {
        if_invoked: vec![],
        if_canceled: vec![Action {
            hold_down_ns: 10,
            ..key("c").into()
        }],
        delay_ns: 50,
    });
    let mut actor = actor(Arc::new(cfg));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let projection = actor
        .begin_descendant_projection(down(&plan, 0, 100).event.payload.slot, MonoTime(90))
        .unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    let other = input(&mut actor, &[key("d").into()], 2, 110);
    actor
        .admit(&down(&other, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 110).0.is_empty());
    assert_eq!(drain(&mut actor, 120).0, [("d".into(), KeyEdge::Down)]);
    actor
        .close_descendant_projection(projection, MonoTime(130))
        .unwrap();
    assert!(drain(&mut actor, 130).0.is_empty());
}
