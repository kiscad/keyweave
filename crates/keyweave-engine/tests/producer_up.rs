#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, output_stage::*, producer_decision::*, stage_bindings::*,
    terminal_actor::ActorError, timeline::*, *,
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
fn slot() -> ActionSlot {
    ActionSlot {
        batch: BatchId(10),
        index: 0,
    }
}
fn config() -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "producer-up".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "main".into(),
            from: key("a"),
            to: vec![key("b").into()],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
    })
}
fn origin(config: Arc<ConfigSnapshot>, batch: u64, stage: Stage, at: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(batch),
        stage,
        start: MonoTime(at),
        device: device(),
        source: SourceToken(100),
        config,
    }
}
fn down(config: Arc<ConfigSnapshot>, at: u64) -> DueEvent<PlannedAction> {
    let plan = plan_actions(
        &[key("a").into()],
        origin(config, 10, Stage::Complex, 100),
        false,
    )
    .unwrap();
    DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(at),
    }
}
fn up(config: Arc<ConfigSnapshot>, earliest: u64, actual: u64) -> DueEvent<PlannedAction> {
    let mut due = down(config, actual);
    due.event.at_ns = MonoTime(earliest);
    due.event.payload.edge = KeyEdge::Up;
    due
}
fn decision(config: Arc<ConfigSnapshot>, at: u64) -> ProducerDecision {
    let output = OutputStageActor::new(config.clone()).unwrap();
    output
        .decide_producer_down(&down(config, at), &ContextSnapshot::default())
        .unwrap()
}
fn arena(config: Arc<ConfigSnapshot>, at: u64) -> BindingArena<ProducerDecision> {
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(config.clone(), 10, Stage::Complex, 100), 1, None)
        .unwrap()
        .commit();
    arena.insert(slot(), decision(config, at)).unwrap();
    arena
}
#[test]
fn prepared_up_abort_drop_and_same_receipt_retry_do_not_release_or_reselect_saved_decision() {
    let cfg = config();
    let mut arena = arena(cfg.clone(), 125);
    let receipt = up(cfg.clone(), 110, 125);
    arena.prepare_producer_up(&receipt).unwrap().abort();
    assert_eq!(arena.live(slot()).unwrap().up_delivered_at(), None);
    {
        let _drop = arena.prepare_producer_up(&receipt).unwrap();
    }
    assert!(matches!(
        arena.finish_producer(slot()),
        Err(ProducerUpError::Lifecycle)
    ));
    arena.prepare_producer_up(&receipt).unwrap().commit();
    assert_eq!(
        arena.live(slot()).unwrap().up_delivered_at(),
        Some(MonoTime(125))
    );
    assert!(matches!(
        arena.prepare_producer_up(&receipt),
        Err(ProducerUpError::Lifecycle)
    ));
    let saved = arena.finish_producer(slot()).unwrap();
    assert!(Arc::ptr_eq(&saved.down().config, &cfg));
    assert_eq!(
        saved.selection().unwrap().actions().next().unwrap().key(),
        Some(key("b"))
    );
    assert!(matches!(
        arena.finish_producer(slot()),
        Err(ProducerUpError::Lifecycle)
    ));
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn up_rejects_every_saved_header_target_lazy_halt_and_causal_time_mismatch_without_marking() {
    let cfg = config();
    let mut saved = decision(cfg.clone(), 125);
    let correct = up(cfg.clone(), 110, 125);
    let copy = || DueEvent {
        event: correct.event.clone(),
        delivered_at: correct.delivered_at,
    };
    let mut cases = vec![];
    let mut bad = copy();
    bad.event.payload.slot.index = 1;
    cases.push(bad);
    let mut bad = copy();
    bad.event.payload.slot.batch = BatchId(11);
    cases.push(bad);
    let mut bad = copy();
    bad.event.stage = Stage::Simple;
    cases.push(bad);
    let mut bad = copy();
    bad.event.device.generation = 2;
    cases.push(bad);
    let mut bad = copy();
    bad.event.source = SourceToken(101);
    cases.push(bad);
    let mut bad = copy();
    bad.event.config = Arc::new((*cfg).clone());
    cases.push(bad);
    let mut bad = copy();
    bad.event.payload.target = ActionTarget::Key(key("c"));
    cases.push(bad);
    let mut bad = copy();
    bad.event.payload.lazy = true;
    cases.push(bad);
    let mut bad = copy();
    bad.event.payload.halt = true;
    cases.push(bad);
    let mut bad = copy();
    bad.event.at_ns = MonoTime(99);
    cases.push(bad);
    let mut bad = copy();
    bad.delivered_at = MonoTime(109);
    cases.push(bad);
    let mut bad = copy();
    bad.delivered_at = MonoTime(124);
    cases.push(bad);
    for bad in cases {
        assert!(saved.prepare_up(&bad).is_err());
        assert_eq!(saved.up_delivered_at(), None);
    }
    for edge in [KeyEdge::Down, KeyEdge::Repeat] {
        let mut bad = copy();
        bad.event.payload.edge = edge;
        assert!(matches!(
            saved.prepare_up(&bad),
            Err(ProducerUpError::Lifecycle)
        ));
    }
    saved.prepare_up(&correct).unwrap().commit();
    assert_eq!(saved.up_delivered_at(), Some(MonoTime(125)));
}
#[test]
fn reserved_unknown_and_released_arena_slots_cannot_receive_normal_producer_up() {
    let cfg = config();
    let mut arena = BindingArena::<ProducerDecision>::default();
    arena
        .prepare(origin(cfg.clone(), 10, Stage::Complex, 100), 1, None)
        .unwrap()
        .commit();
    let receipt = up(cfg.clone(), 150, 150);
    assert!(matches!(
        arena.prepare_producer_up(&receipt),
        Err(ProducerUpError::Lifecycle)
    ));
    let mut unknown = up(cfg.clone(), 150, 150);
    unknown.event.payload.slot.batch = BatchId(11);
    assert!(matches!(
        arena.prepare_producer_up(&unknown),
        Err(ProducerUpError::Lifecycle)
    ));
    arena.insert(slot(), decision(cfg, 100)).unwrap();
    arena.prepare_producer_up(&receipt).unwrap().commit();
    arena.finish_producer(slot()).unwrap();
    assert!(matches!(
        arena.prepare_producer_up(&receipt),
        Err(ProducerUpError::Lifecycle)
    ));
}
#[test]
fn parent_up_commit_is_not_child_completion_and_empty_reserved_child_needs_explicit_cleanup() {
    let cfg = config();
    let mut arena = arena(cfg.clone(), 100);
    arena
        .prepare(origin(cfg.clone(), 20, Stage::Output, 100), 1, Some(slot()))
        .unwrap()
        .commit();
    arena
        .prepare_producer_up(&up(cfg, 150, 150))
        .unwrap()
        .commit();
    assert!(matches!(
        arena.finish_producer(slot()),
        Err(ProducerUpError::Binding(BindingError::ChildPending))
    ));
    assert_eq!(arena.live_slots(), 1);
    let child = arena.cancel_subtree(BatchId(20)).unwrap();
    assert!(child[0].payloads.is_empty());
    assert_eq!(
        arena.finish_producer(slot()).unwrap().up_delivered_at(),
        Some(MonoTime(150))
    );
    arena.retire(BatchId(10)).unwrap();
}
#[test]
fn committed_producer_up_pairs_saved_output_up_across_reload_and_parent_finishes_after_actual_child()
 {
    let cfg = config();
    let mut arena = BindingArena::<ProducerDecision>::default();
    arena
        .prepare(origin(cfg.clone(), 10, Stage::Complex, 100), 1, None)
        .unwrap()
        .commit();
    let mut output = OutputStageActor::new(cfg.clone()).unwrap();
    output.observe_device(device(), false, MonoTime(0)).unwrap();
    let receipt = down(cfg.clone(), 125);
    let saved = output
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    let plan = saved.plan_main(BatchId(20)).unwrap();
    let registration = arena
        .prepare_child_with_down(
            origin(cfg.clone(), 20, Stage::Output, 125),
            1,
            receipt,
            saved,
        )
        .unwrap();
    output
        .prepare_child_scoped(plan, registration)
        .unwrap()
        .commit();
    assert_eq!(
        output.poll(MonoTime(125)).unwrap().unwrap().outputs[0].key,
        key("b")
    );
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let original_up = up(cfg.clone(), 150, 150);
    let parent_up = arena.prepare_producer_up(&original_up).unwrap();
    assert!(output.input_up(BatchId(20), MonoTime(150)).unwrap());
    parent_up.commit();
    assert!(matches!(
        arena.finish_producer(slot()),
        Err(ProducerUpError::Binding(BindingError::ChildPending))
    ));
    let actual = output.poll(MonoTime(150)).unwrap().unwrap();
    assert_eq!(actual.outputs[0].key, key("b"));
    assert_eq!(actual.outputs[0].edge, KeyEdge::Up);
    assert_eq!(actual.completed_batch, Some(BatchId(20)));
    // Output completion does not detach external registration.
    assert!(matches!(
        arena.finish_producer(slot()),
        Err(ProducerUpError::Binding(BindingError::ChildPending))
    ));
    arena.cancel_subtree(BatchId(20)).unwrap();
    let saved = arena.finish_producer(slot()).unwrap();
    assert!(Arc::ptr_eq(&saved.down().config, &cfg));
    arena.retire(BatchId(10)).unwrap();
}
#[test]
fn downstream_up_queue_rejection_aborts_parent_mark_then_original_receipt_can_retry() {
    let cfg = config();
    let mut arena = arena(cfg.clone(), 100);
    let mut output = OutputStageActor::new(cfg.clone()).unwrap();
    output.observe_device(device(), false, MonoTime(0)).unwrap();
    let saved = arena.live(slot()).unwrap();
    let plan = saved.plan_main(BatchId(20)).unwrap();
    let registration = arena
        .prepare(origin(cfg.clone(), 20, Stage::Output, 100), 1, Some(slot()))
        .unwrap();
    output
        .prepare_child_scoped(plan, registration)
        .unwrap()
        .commit();
    output.poll(MonoTime(100)).unwrap();
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("c").into()
        };
        512
    ];
    for batch in 21..=28 {
        output
            .enqueue(
                plan_actions(
                    &actions,
                    origin(cfg.clone(), batch, Stage::Output, 101),
                    true,
                )
                .unwrap(),
            )
            .unwrap();
    }
    let receipt = up(cfg, 100, 100);
    {
        let _parent_up = arena.prepare_producer_up(&receipt).unwrap();
        assert!(matches!(
            output.input_up(BatchId(20), MonoTime(100)),
            Err(OutputStageError::Terminal(ActorError::Timeline(
                TimelineError::Capacity
            )))
        ));
    }
    assert_eq!(arena.live(slot()).unwrap().up_delivered_at(), None);
    output.cancel_batch(BatchId(21), MonoTime(100)).unwrap();
    let parent_up = arena.prepare_producer_up(&receipt).unwrap();
    output.input_up(BatchId(20), MonoTime(100)).unwrap();
    parent_up.commit();
    assert!(matches!(
        arena.finish_producer(slot()),
        Err(ProducerUpError::Binding(BindingError::ChildPending))
    ));
    while output.poll(MonoTime(5000)).unwrap().is_some() {}
    arena.cancel_subtree(BatchId(20)).unwrap();
    arena.finish_producer(slot()).unwrap();
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(output.pending_batches(), 0);
}
#[test]
fn consumed_empty_parent_finishes_only_after_valid_original_up_without_any_child_output() {
    let mut raw = (*config()).clone();
    raw.complex[0].to.clear();
    let cfg = Arc::new(raw);
    let mut arena = BindingArena::<ProducerDecision>::default();
    arena
        .prepare(origin(cfg.clone(), 10, Stage::Complex, 100), 1, None)
        .unwrap()
        .commit();
    let mut output = OutputStageActor::new(cfg.clone()).unwrap();
    output.observe_device(device(), false, MonoTime(0)).unwrap();
    let receipt = down(cfg.clone(), 125);
    let saved = output
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    let plan = saved.plan_main(BatchId(20)).unwrap();
    let activation = arena
        .prepare_consumed_down(origin(cfg.clone(), 20, Stage::Output, 125), receipt, saved)
        .unwrap();
    output
        .prepare_empty_child_scoped(plan, activation)
        .unwrap()
        .commit();
    assert!(matches!(
        arena.finish_producer(slot()),
        Err(ProducerUpError::Lifecycle)
    ));
    let receipt = up(cfg, 150, 150);
    let parent_up = arena.prepare_producer_up(&receipt).unwrap();
    assert!(!output.input_up(BatchId(20), MonoTime(150)).unwrap());
    parent_up.commit();
    let saved = arena.finish_producer(slot()).unwrap();
    assert_eq!(saved.up_delivered_at(), Some(MonoTime(150)));
    assert_eq!(saved.selection().unwrap().actions().len(), 0);
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
    assert_eq!(output.pending_batches(), 0);
    assert!(output.poll(MonoTime(150)).unwrap().is_none());
    assert!(output.pressed_outputs().is_empty());
}
#[test]
fn variable_original_up_requires_exact_saved_operation_values_and_preserves_lazy_semantics() {
    let cfg = config();
    let variable = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        lazy: true,
        ..key("vk_none").into()
    };
    let plan = plan_actions(
        &[variable],
        origin(cfg.clone(), 10, Stage::Complex, 100),
        false,
    )
    .unwrap();
    let receipt = DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(125),
    };
    let output = OutputStageActor::new(cfg).unwrap();
    let mut saved = output
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    let correct = DueEvent {
        event: plan.input_up_events(MonoTime(110))[0].clone(),
        delivered_at: MonoTime(125),
    };
    let mut wrong = DueEvent {
        event: correct.event.clone(),
        delivered_at: correct.delivered_at,
    };
    let ActionTarget::SetVariable(variable) = &mut wrong.event.payload.target else {
        panic!("variable target");
    };
    variable.operation = VariableOperation::Set {
        value: Some(VariableValue::Integer(1)),
        key_up_value: Some(VariableValue::Integer(3)),
    };
    assert!(matches!(
        saved.prepare_up(&wrong),
        Err(ProducerUpError::Provenance)
    ));
    assert_eq!(saved.up_delivered_at(), None);
    saved.prepare_up(&correct).unwrap().commit();
    assert_eq!(saved.up_delivered_at(), Some(MonoTime(125)));
    assert!(saved.down().payload.lazy);
}
