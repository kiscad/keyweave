#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, complex_actor::*, generated_stage::*, modifier_scope::ScopedActionPlan,
    producer_decision::ProducerDecision, stage_bindings::*, timeline::*, *,
};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device() -> DeviceInstanceId {
    DeviceInstanceId {
        id: 1,
        generation: 1,
    }
}
fn parent() -> ActionSlot {
    ActionSlot {
        batch: BatchId(5000),
        index: 0,
    }
}
fn cfg(action: Option<Action>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "retained-input".into(),
        simple: vec![Mapping {
            id: "simple".into(),
            from: key("a"),
            to: action.into_iter().collect(),
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
        complex: vec![Mapping {
            id: "complex".into(),
            from: key("b"),
            to: vec![key("c").into()],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
    })
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
        ..key("vk_none").into()
    }
}
fn origin(cfg: Arc<ConfigSnapshot>, id: u64, stage: Stage, at: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(id),
        stage,
        start: MonoTime(at),
        device: device(),
        source: SourceToken(100),
        config: cfg,
    }
}
fn copy_due(due: &DueEvent<PlannedAction>) -> DueEvent<PlannedAction> {
    DueEvent {
        event: due.event.clone(),
        delivered_at: due.delivered_at,
    }
}
struct Setup {
    old: Arc<ConfigSnapshot>,
    queue: GeneratedStageQueue,
    actor: ComplexStageActor,
    arena: BindingArena<ProducerDecision>,
    down: DueEvent<PlannedAction>,
}
fn setup(old: Arc<ConfigSnapshot>) -> Setup {
    let current = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    let mut queue = GeneratedStageQueue::new(Stage::Complex, old.clone()).unwrap();
    queue.observe_device(device(), false).unwrap();
    let mut actor = ComplexStageActor::new(old.clone()).unwrap();
    actor.observe_device(device(), false, MonoTime(0)).unwrap();
    queue.activate(current.clone()).unwrap();
    actor.activate(current).unwrap();
    let root = plan_actions(
        &[key("a").into()],
        origin(old.clone(), 5000, Stage::Simple, 100),
        false,
    )
    .unwrap();
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(old.clone(), 5000, Stage::Simple, 100), 1, None)
        .unwrap()
        .commit();
    Setup {
        old,
        queue,
        actor,
        arena,
        down: DueEvent {
            event: root.events()[0].clone(),
            delivered_at: MonoTime(125),
        },
    }
}
fn freeze(s: &Setup, id: u64) -> (ProducerDecision, ScopedActionPlan) {
    let decision = s
        .queue
        .decide_producer_down(&s.down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(id)).unwrap();
    (decision, plan)
}
fn assert_unchanged(s: &Setup) {
    assert_eq!(s.arena.live_slots(), 0);
    assert_eq!(s.arena.reserved_sources(), 1);
    s.arena.check_down(parent()).unwrap();
    assert_eq!(s.actor.pending_inputs(), 0);
    assert_eq!(s.actor.reserved_sources(), 0);
    assert_eq!(s.actor.next_batch_id().unwrap(), BatchId(1));
    assert_eq!(s.queue.reserved_slots(), 0);
    assert!(s.queue.variable_values().is_empty());
}
#[test]
fn old_simple_parent_drives_old_complex_mapping_and_saved_up_until_actual_completion() {
    let mut s = setup(cfg(Some(key("b").into())));
    let (decision, plan) = freeze(&s, 10);
    assert!(matches!(
        s.actor.prepare_input(plan.actions().clone()),
        Err(ComplexActorError::Input)
    ));
    let registration = s
        .arena
        .prepare_child_with_down(
            origin(s.old.clone(), 10, Stage::Complex, 125),
            1,
            copy_due(&s.down),
            decision,
        )
        .unwrap();
    let mut saved = s
        .actor
        .prepare_generated_child_input(&mut s.queue, &plan, registration)
        .unwrap()
        .commit();
    assert_eq!(s.arena.live_slots(), 1);
    let down = s.queue.poll(MonoTime(125)).unwrap().unwrap();
    assert!(Arc::ptr_eq(&down.event.config, &s.old));
    s.actor.admit(&down, &ContextSnapshot::default()).unwrap();
    let mut outputs = Vec::new();
    while let Some(step) = s.actor.poll(MonoTime(125)).unwrap() {
        outputs.extend(step.outputs);
    }
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].key, key("c"));
    assert_eq!(outputs[0].edge, KeyEdge::Down);
    let mut raw_up = copy_due(&s.down);
    raw_up.event.payload.edge = KeyEdge::Up;
    raw_up.event.at_ns = MonoTime(150);
    raw_up.delivered_at = MonoTime(150);
    let parent_up = s.arena.prepare_producer_up(&raw_up).unwrap();
    s.queue
        .prepare_input_up_scoped(&mut saved, MonoTime(150))
        .unwrap()
        .unwrap()
        .commit();
    parent_up.commit();
    assert!(matches!(
        s.arena.finish_producer(parent()),
        Err(
            keyweave_engine::producer_decision::ProducerUpError::Binding(
                BindingError::ChildPending
            )
        )
    ));
    let up = s.queue.poll(MonoTime(150)).unwrap().unwrap();
    outputs.extend(
        s.actor
            .admit(&up, &ContextSnapshot::default())
            .unwrap()
            .outputs,
    );
    while let Some(step) = s.actor.poll(MonoTime(150)).unwrap() {
        outputs.extend(step.outputs);
    }
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[1].key, key("c"));
    assert_eq!(outputs[1].edge, KeyEdge::Up);
    assert_eq!(s.actor.pending_inputs(), 0);
    assert_eq!(s.actor.reserved_sources(), 0);
    s.queue.retire_batch(BatchId(10)).unwrap();
    let child = s.arena.cancel_subtree(BatchId(10)).unwrap();
    assert!(child[0].payloads.is_empty());
    s.arena.finish_producer(parent()).unwrap();
    s.arena.retire(BatchId(5000)).unwrap();
    assert_eq!(s.arena.reserved_sources(), 0);
}
#[test]
fn abort_and_drop_leave_old_parent_reserved_and_do_not_generate_variable_or_consume_id() {
    let mut s = setup(cfg(Some(variable())));
    for explicit in [true, false] {
        let (decision, plan) = freeze(&s, 10);
        let registration = s
            .arena
            .prepare_child_with_down(
                origin(s.old.clone(), 10, Stage::Complex, 125),
                1,
                copy_due(&s.down),
                decision,
            )
            .unwrap();
        let guard = s
            .actor
            .prepare_generated_child_input(&mut s.queue, &plan, registration)
            .unwrap();
        if explicit {
            guard.abort();
        } else {
            drop(guard);
        }
        assert_unchanged(&s);
        assert_eq!(s.queue.queued_events(), 0);
    }
}
#[test]
fn full_generated_queue_rejects_without_committing_actor_input_or_parent_activation() {
    let mut s = setup(cfg(Some(variable())));
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("z").into()
        };
        512
    ];
    for id in 100..108 {
        let plan = plan_actions(
            &actions,
            origin(s.queue.snapshot().clone(), id, Stage::Complex, 0),
            true,
        )
        .unwrap();
        s.queue.enqueue(plan.events().to_vec()).unwrap();
    }
    let (decision, plan) = freeze(&s, 10);
    let registration = s
        .arena
        .prepare_child_with_down(
            origin(s.old.clone(), 10, Stage::Complex, 125),
            1,
            copy_due(&s.down),
            decision,
        )
        .unwrap();
    assert!(matches!(
        s.actor
            .prepare_generated_child_input(&mut s.queue, &plan, registration),
        Err(ComplexActorError::Generated(GeneratedError::Timeline(
            TimelineError::Capacity
        )))
    ));
    assert_eq!(s.arena.live_slots(), 0);
    s.arena.check_down(parent()).unwrap();
    assert_eq!(s.actor.reserved_sources(), 0);
    assert_eq!(s.actor.next_batch_id().unwrap(), BatchId(1));
    assert!(s.queue.variable_values().is_empty());
    assert_eq!(s.queue.queued_events(), MAX_STAGE_EVENTS);
}
#[test]
fn full_actual_input_budget_rejects_before_generated_variable_or_parent_commit() {
    let mut s = setup(cfg(Some(variable())));
    let actions = vec![key("z").into(); 512];
    for id in 1..=32 {
        let plan = plan_actions(
            &actions,
            origin(s.actor.snapshot().clone(), id, Stage::Complex, 0),
            false,
        )
        .unwrap();
        s.actor.register_input(plan).unwrap();
    }
    let (decision, plan) = freeze(&s, 33);
    let registration = s
        .arena
        .prepare_child_with_down(
            origin(s.old.clone(), 33, Stage::Complex, 125),
            1,
            copy_due(&s.down),
            decision,
        )
        .unwrap();
    assert!(matches!(
        s.actor
            .prepare_generated_child_input(&mut s.queue, &plan, registration),
        Err(ComplexActorError::Binding(BindingError::Capacity))
    ));
    assert_eq!(s.arena.live_slots(), 0);
    s.arena.check_down(parent()).unwrap();
    assert_eq!(s.actor.reserved_sources(), MAX_OUTPUT_OWNERS);
    assert_eq!(s.actor.next_batch_id().unwrap(), BatchId(33));
    assert_eq!(s.queue.queued_events(), 0);
    assert!(s.queue.variable_values().is_empty());
}
#[test]
fn old_empty_consumption_aborts_then_commits_without_child_identity_and_up_is_noop() {
    let mut s = setup(cfg(None));
    let (decision, plan) = freeze(&s, 10);
    let activation = s
        .arena
        .prepare_consumed_down(
            origin(s.old.clone(), 10, Stage::Complex, 125),
            copy_due(&s.down),
            decision,
        )
        .unwrap();
    s.actor
        .prepare_generated_empty_input(&mut s.queue, &plan, activation)
        .unwrap()
        .abort();
    assert_unchanged(&s);
    let (decision, plan) = freeze(&s, 10);
    let activation = s
        .arena
        .prepare_consumed_down(
            origin(s.old.clone(), 10, Stage::Complex, 125),
            copy_due(&s.down),
            decision,
        )
        .unwrap();
    let mut saved = s
        .actor
        .prepare_generated_empty_input(&mut s.queue, &plan, activation)
        .unwrap()
        .commit();
    assert_eq!(s.arena.live_slots(), 1);
    assert_eq!(s.arena.reserved_sources(), 1);
    assert_eq!(s.actor.next_batch_id().unwrap(), BatchId(11));
    assert_eq!(s.actor.pending_inputs(), 0);
    assert_eq!(s.queue.reserved_slots(), 0);
    assert!(!s.queue.input_up_scoped(&mut saved, MonoTime(150)).unwrap());
    let mut up = copy_due(&s.down);
    up.event.payload.edge = KeyEdge::Up;
    up.event.at_ns = MonoTime(150);
    up.delivered_at = MonoTime(150);
    s.arena.prepare_producer_up(&up).unwrap().commit();
    s.arena.finish_producer(parent()).unwrap();
    s.arena.retire(BatchId(5000)).unwrap();
}
#[test]
fn different_current_arcs_and_wrong_child_metadata_reject_without_relaxing_roots() {
    let mut s = setup(cfg(Some(variable())));
    let (decision, plan) = freeze(&s, 10);
    let wrong = s
        .arena
        .prepare_child_with_down(
            origin(s.old.clone(), 11, Stage::Complex, 125),
            1,
            copy_due(&s.down),
            decision,
        )
        .unwrap();
    assert!(matches!(
        s.actor
            .prepare_generated_child_input(&mut s.queue, &plan, wrong),
        Err(ComplexActorError::Input)
    ));
    assert_unchanged(&s);
    let foreign = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    let mut different = ComplexStageActor::new(foreign).unwrap();
    different
        .observe_device(device(), false, MonoTime(0))
        .unwrap();
    let (decision, plan) = freeze(&s, 10);
    let registration = s
        .arena
        .prepare_child_with_down(
            origin(s.old.clone(), 10, Stage::Complex, 125),
            1,
            copy_due(&s.down),
            decision,
        )
        .unwrap();
    assert!(matches!(
        different.prepare_generated_child_input(&mut s.queue, &plan, registration),
        Err(ComplexActorError::Input)
    ));
    assert_unchanged(&s);
    assert!(matches!(
        different.prepare_input(plan.actions().clone()),
        Err(ComplexActorError::Input)
    ));
}
