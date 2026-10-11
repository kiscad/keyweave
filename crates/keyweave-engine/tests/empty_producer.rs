#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::*, modifier_scope::ScopedActionPlan, output_stage::*,
    producer_decision::ProducerDecision, rule_selection::*, stage_bindings::*,
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
fn parent() -> ActionSlot {
    ActionSlot {
        batch: BatchId(5000),
        index: 0,
    }
}
fn config(empty: bool) -> Arc<ConfigSnapshot> {
    let mapping = || Mapping {
        id: "consume".into(),
        from: key("a"),
        to: vec![Action {
            conditions: if empty {
                vec![Condition::Variable {
                    name: "v".into(),
                    value: VariableValue::Integer(1),
                    unless: false,
                }]
            } else {
                vec![]
            },
            ..key("b").into()
        }],
        allow_extra_modifiers: true,
        extras: MappingExtras {
            after_key_up: vec![Action {
                hold_down_ns: 20,
                ..key("c").into()
            }],
            ..Default::default()
        },
    };
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "empty-producer".into(),
        simple: vec![mapping()],
        complex: vec![mapping()],
    })
}
fn origin(config: Arc<ConfigSnapshot>, batch: u64, stage: Stage, start: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(batch),
        stage,
        start: MonoTime(start),
        device: device(),
        source: SourceToken(100),
        config,
    }
}
fn receipt(config: Arc<ConfigSnapshot>, stage: Stage) -> DueEvent<PlannedAction> {
    let plan = plan_actions(&[key("a").into()], origin(config, 5000, stage, 100), false).unwrap();
    DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(125),
    }
}
fn arena(config: Arc<ConfigSnapshot>, stage: Stage) -> BindingArena<ProducerDecision> {
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(config, 5000, stage, 100), 1, None)
        .unwrap()
        .commit();
    arena
}
fn output(config: Arc<ConfigSnapshot>) -> OutputStageActor {
    let mut output = OutputStageActor::new(config).unwrap();
    output.observe_device(device(), false, MonoTime(0)).unwrap();
    output
}
fn queue(config: Arc<ConfigSnapshot>) -> GeneratedStageQueue {
    let mut queue = GeneratedStageQueue::new(Stage::Complex, config).unwrap();
    queue.observe_device(device(), false).unwrap();
    queue
}
fn assert_reserved(arena: &BindingArena<ProducerDecision>) {
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_sources(), 1);
    arena.check_down(parent()).unwrap();
}
#[test]
fn consumed_old_output_main_activates_only_parent_and_consumes_output_id_without_fake_child_or_owner()
 {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    assert!(plan.actions().events().is_empty());
    assert!(matches!(
        output.prepare_scoped(plan),
        Err(OutputStageError::Generated(GeneratedError::Provenance))
    ));
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(origin(old, 6000, Stage::Output, 125), down, decision)
        .unwrap();
    output
        .prepare_empty_child_scoped(plan, activation)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(arena.reserved_sources(), 1);
    assert_eq!(arena.extra_source_reservations(), 0);
    let mut nodes = 0;
    arena.visit_subtree(parent(), |_| nodes += 1).unwrap();
    assert_eq!(nodes, 1);
    assert_eq!(output.pending_batches(), 0);
    assert_eq!(output.reserved_slots(), 0);
    assert_eq!(output.queued_events(), 0);
    assert!(output.pressed_outputs().is_empty());
    assert!(output.poll(MonoTime(125)).unwrap().is_none());
    let lower = plan_actions(
        &[key("z").into()],
        origin(output.snapshot().clone(), 5999, Stage::Output, 150),
        false,
    )
    .unwrap();
    assert!(matches!(
        output.prepare(lower),
        Err(OutputStageError::Terminal(ActorError::InvalidBatch))
    ));
    assert!(!output.input_up(BatchId(6000), MonoTime(150)).unwrap());
    let saved = arena.take(parent()).unwrap();
    assert_eq!(saved.selection().unwrap().actions().len(), 0);
    arena.retire(BatchId(5000)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn empty_output_abort_preserves_parent_and_output_id_then_same_frozen_main_can_retry() {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(
            origin(old.clone(), 6000, Stage::Output, 125),
            down,
            decision,
        )
        .unwrap();
    output
        .prepare_empty_child_scoped(plan, activation)
        .unwrap()
        .abort();
    assert_reserved(&arena);
    let lower = plan_actions(
        &[key("z").into()],
        origin(old.clone(), 5999, Stage::Output, 0),
        false,
    )
    .unwrap();
    output.prepare(lower).unwrap().abort();
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(origin(old, 6000, Stage::Output, 125), down, decision)
        .unwrap();
    {
        let _drop = output.prepare_empty_child_scoped(plan, activation).unwrap();
    }
    assert_reserved(&arena);
    assert_eq!(output.pending_batches(), 0);
    assert_eq!(output.queued_events(), 0);
}
#[test]
fn empty_complex_generation_returns_noop_up_credential_and_retains_saved_simple_producer_for_original_up()
 {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Simple);
    let mut queue = queue(old.clone());
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let down = receipt(old.clone(), Stage::Simple);
    let decision = queue
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(
            origin(old.clone(), 6000, Stage::Complex, 125),
            down,
            decision,
        )
        .unwrap();
    let mut saved = queue
        .prepare_empty_complex_child_scoped(&plan, activation)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(queue.pending_batches(), 0);
    assert_eq!(queue.reserved_slots(), 0);
    assert_eq!(queue.queued_events(), 0);
    assert!(!queue.input_up_scoped(&mut saved, MonoTime(150)).unwrap());
    assert!(queue.poll(MonoTime(140)).unwrap().is_none());
    let canceled = arena.cancel_subtree(BatchId(5000)).unwrap();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].extra_sources, 0);
    assert_eq!(canceled[0].payloads.len(), 1);
    assert!(Arc::ptr_eq(
        &canceled[0].payloads[0].payload.down().config,
        &old
    ));
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn consumed_down_rejects_receipt_and_next_origin_mismatches_and_replayed_live_or_released_parent() {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Complex);
    let output = output(old.clone());
    let base = origin(old.clone(), 6000, Stage::Output, 125);
    let mut cases = vec![];
    let mut bad = base.clone();
    bad.batch = BatchId(5000);
    cases.push(bad);
    let mut bad = base.clone();
    bad.stage = Stage::Complex;
    cases.push(bad);
    let mut bad = base.clone();
    bad.device.generation = 2;
    cases.push(bad);
    let mut bad = base.clone();
    bad.source = SourceToken(101);
    cases.push(bad);
    let mut bad = base.clone();
    bad.config = Arc::new((*old).clone());
    cases.push(bad);
    let mut bad = base.clone();
    bad.start = MonoTime(124);
    cases.push(bad);
    for bad in cases {
        let down = receipt(old.clone(), Stage::Complex);
        let decision = output
            .decide_producer_down(&down, &ContextSnapshot::default())
            .unwrap();
        assert!(arena.prepare_consumed_down(bad, down, decision).is_err());
        assert_reserved(&arena);
    }
    for edge in [KeyEdge::Up, KeyEdge::Repeat] {
        let mut down = receipt(old.clone(), Stage::Complex);
        let decision = output
            .decide_producer_down(&down, &ContextSnapshot::default())
            .unwrap();
        down.event.payload.edge = edge;
        assert!(matches!(
            arena.prepare_consumed_down(base.clone(), down, decision),
            Err(BindingError::Lifecycle)
        ));
        assert_reserved(&arena);
    }
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    arena
        .prepare_consumed_down(base.clone(), down, decision)
        .unwrap()
        .commit();
    for released in [false, true] {
        if released {
            arena.take(parent()).unwrap();
        }
        let down = receipt(old.clone(), Stage::Complex);
        let decision = output
            .decide_producer_down(&down, &ContextSnapshot::default())
            .unwrap();
        assert!(matches!(
            arena.prepare_consumed_down(base.clone(), down, decision),
            Err(BindingError::Lifecycle)
        ));
    }
}
#[test]
fn empty_destination_transaction_rejects_nonempty_plan_without_latching_actor_or_activating_parent()
{
    let old = config(false);
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(origin(old, 6000, Stage::Output, 125), down, decision)
        .unwrap();
    assert!(matches!(
        output.prepare_empty_child_scoped(plan, activation),
        Err(OutputStageError::Generated(GeneratedError::Lifecycle))
    ));
    assert_reserved(&arena);
    assert!(!output.is_failed());
    assert_eq!(output.pending_batches(), 0);
}
#[test]
fn empty_plan_origin_must_match_activation_witness_and_wrong_queue_stage_does_not_activate() {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6001)).unwrap();
    let activation = arena
        .prepare_consumed_down(
            origin(old.clone(), 6000, Stage::Output, 125),
            down,
            decision,
        )
        .unwrap();
    assert!(matches!(
        output.prepare_empty_child_scoped(plan, activation),
        Err(OutputStageError::Generated(GeneratedError::Provenance))
    ));
    assert_reserved(&arena);
    let mut wrong = GeneratedStageQueue::new(Stage::Output, old.clone()).unwrap();
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(origin(old, 6000, Stage::Output, 125), down, decision)
        .unwrap();
    assert!(matches!(
        wrong.prepare_empty_complex_child_scoped(&plan, activation),
        Err(GeneratedError::Stage)
    ));
    assert_reserved(&arena);
}
#[test]
fn full_arena_allows_consumed_activation_without_extra_slot_or_child_batch() {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Simple);
    arena
        .prepare(
            origin(old.clone(), 8000, Stage::Simple, 0),
            MAX_OUTPUT_OWNERS - 1,
            None,
        )
        .unwrap()
        .commit();
    let mut queue = queue(old.clone());
    let down = receipt(old.clone(), Stage::Simple);
    let decision = queue
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(origin(old, 6000, Stage::Complex, 125), down, decision)
        .unwrap();
    queue
        .prepare_empty_complex_child_scoped(&plan, activation)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS);
    assert_eq!(arena.extra_source_reservations(), 0);
    assert_eq!(queue.pending_batches(), 0);
}
#[test]
fn full_output_batch_capacity_rejects_empty_main_before_parent_activation_then_same_main_retries() {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    for batch in 1..=MAX_BINDINGS as u64 {
        output
            .enqueue(
                plan_actions(
                    &[key("z").into()],
                    origin(old.clone(), batch, Stage::Output, 0),
                    false,
                )
                .unwrap(),
            )
            .unwrap();
    }
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(
            origin(old.clone(), 6000, Stage::Output, 125),
            down,
            decision,
        )
        .unwrap();
    assert!(matches!(
        output.prepare_empty_child_scoped(plan, activation),
        Err(OutputStageError::Terminal(ActorError::Capacity))
    ));
    assert_reserved(&arena);
    assert_eq!(output.pending_batches(), MAX_BINDINGS);
    output.cancel_batch(BatchId(1), MonoTime(0)).unwrap();
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(origin(old, 6000, Stage::Output, 125), down, decision)
        .unwrap();
    output
        .prepare_empty_child_scoped(plan, activation)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(output.pending_batches(), MAX_BINDINGS - 1);
    assert_eq!(output.queued_events(), MAX_BINDINGS - 1);
}
#[test]
fn empty_main_retains_original_rule_for_later_after_up_child_and_cleanup_waits_its_duration() {
    let old = config(true);
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let down = receipt(old.clone(), Stage::Complex);
    let decision = output
        .decide_producer_down(&down, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(6000)).unwrap();
    let activation = arena
        .prepare_consumed_down(
            origin(old.clone(), 6000, Stage::Output, 125),
            down,
            decision,
        )
        .unwrap();
    output
        .prepare_empty_child_scoped(plan, activation)
        .unwrap()
        .commit();
    let original = arena.live(parent()).unwrap().selection().unwrap();
    let branch = original
        .select_branch(RuleBranch::AfterKeyUp, false, |_| Truth::True)
        .unwrap();
    let plan = ScopedActionPlan::branch(
        &branch,
        BatchId(7000),
        MonoTime(150),
        device(),
        SourceToken(100),
    )
    .unwrap();
    let registration = arena
        .prepare(origin(old, 7000, Stage::Output, 150), 1, Some(parent()))
        .unwrap();
    output
        .prepare_child_scoped(plan, registration)
        .unwrap()
        .commit();
    assert!(matches!(
        arena.take(parent()),
        Err(BindingError::ChildPending)
    ));
    let down = output.poll(MonoTime(150)).unwrap().unwrap();
    assert_eq!(down.outputs[0].key, key("c"));
    assert_eq!(down.outputs[0].edge, KeyEdge::Down);
    assert!(output.poll(MonoTime(169)).unwrap().is_none());
    let up = output.poll(MonoTime(170)).unwrap().unwrap();
    assert_eq!(up.outputs[0].edge, KeyEdge::Up);
    assert_eq!(up.completed_batch, Some(BatchId(7000)));
    arena.cancel_subtree(BatchId(7000)).unwrap();
    let original = arena.take(parent()).unwrap();
    assert_eq!(original.selection().unwrap().actions().len(), 0);
    arena.retire(BatchId(5000)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
    assert_eq!(output.pending_batches(), 0);
}
