#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::*, modifier_scope::ScopedActionPlan, output_stage::*,
    rule_selection::*, stage_bindings::*, terminal_actor::ActorError, timeline::*, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
// Retained producer state need not implement Clone/Debug or execute any side effect.
struct Producer {
    label: String,
}
fn producer() -> Producer {
    Producer {
        label: "original producer".into(),
    }
}
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device() -> DeviceInstanceId {
    DeviceInstanceId {
        id: 1,
        generation: 1,
    }
}
fn slot(batch: u64) -> ActionSlot {
    ActionSlot {
        batch: BatchId(batch),
        index: 0,
    }
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
        name: "producer-down".into(),
        simple: vec![Mapping {
            id: "simple".into(),
            from: key("a"),
            to: vec![key("b").into()],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
        complex: vec![
            Mapping {
                id: "complex".into(),
                from: key("b"),
                to: vec![key("c").into()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            },
            Mapping {
                id: "snapshot".into(),
                from: key("x"),
                to: vec![variable()],
                allow_extra_modifiers: true,
                extras,
            },
        ],
    })
}
fn origin(config: Arc<ConfigSnapshot>, batch: u64, stage: Stage) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(batch),
        stage,
        start: MonoTime(100),
        device: device(),
        source: SourceToken(100),
        config,
    }
}
fn arena(config: Arc<ConfigSnapshot>, stage: Stage) -> BindingArena<Producer> {
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(config, 10, stage), 1, None)
        .unwrap()
        .commit();
    arena
}
fn receipt(config: Arc<ConfigSnapshot>, stage: Stage) -> DueEvent<PlannedAction> {
    let plan = plan_actions(
        &[key(if stage == Stage::Simple { "a" } else { "b" }).into()],
        origin(config, 10, stage),
        false,
    )
    .unwrap();
    DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(100),
    }
}
fn output(config: Arc<ConfigSnapshot>) -> OutputStageActor {
    let mut output = OutputStageActor::new(config).unwrap();
    output.observe_device(device(), false, MonoTime(0)).unwrap();
    output
}
fn snapshot(config: Arc<ConfigSnapshot>) -> ScopedActionPlan {
    let selected = select_rule(
        config,
        MappingStage::Complex,
        key("x"),
        BTreeSet::from([key("left_shift"), key("left_control")]),
        |_| Truth::True,
    )
    .unwrap();
    let branch = selected
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    ScopedActionPlan::branch(
        &branch,
        BatchId(20),
        MonoTime(100),
        device(),
        SourceToken(100),
    )
    .unwrap()
}
fn prepared_parent_is_unchanged(arena: &BindingArena<Producer>) {
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_sources(), 1);
    assert!(arena.live(slot(10)).is_none());
    arena.check_down(slot(10)).unwrap();
}
#[test]
fn child_commit_activates_original_parent_once_without_charging_its_reserved_slot_again() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old.clone(), Stage::Complex),
            producer(),
        )
        .unwrap()
        .with_extra_sources(2)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(arena.reserved_slots(), 2);
    assert_eq!(arena.reserved_sources(), 4);
    assert_eq!(arena.live(slot(10)).unwrap().label, "original producer");
    assert!(matches!(
        arena.take(slot(10)),
        Err(BindingError::ChildPending)
    ));
    let canceled = arena.cancel_subtree(BatchId(20)).unwrap();
    assert_eq!(canceled[0].extra_sources, 2);
    assert!(canceled[0].payloads.is_empty());
    let parent = arena.take(slot(10)).unwrap();
    assert_eq!(parent.label, "original producer");
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
    assert!(
        arena
            .prepare_child_with_down(
                origin(old.clone(), 30, Stage::Output),
                1,
                receipt(old, Stage::Complex),
                producer()
            )
            .is_err()
    );
}
#[test]
fn dropping_activation_with_extra_sources_changes_neither_parent_nor_budget() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    {
        let _prepared = arena
            .prepare_child_with_down(
                origin(old.clone(), 20, Stage::Output),
                1,
                receipt(old, Stage::Complex),
                producer(),
            )
            .unwrap()
            .with_extra_sources(8)
            .unwrap();
    }
    prepared_parent_is_unchanged(&arena);
    // The original reserved down is still available to the ordinary lifecycle.
    arena.insert(slot(10), producer()).unwrap();
    assert_eq!(arena.live_slots(), 1);
}
#[test]
fn down_receipt_rejects_edge_slot_stage_device_source_arc_and_causal_time_mismatches() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    let correct = receipt(old.clone(), Stage::Complex);
    let mut cases = vec![];
    // Reconstruct receipts because DueEvent need not be Clone.
    let copy = || DueEvent {
        event: correct.event.clone(),
        delivered_at: correct.delivered_at,
    };
    let mut wrong = copy();
    wrong.event.payload.edge = KeyEdge::Up;
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.payload.edge = KeyEdge::Repeat;
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.payload.slot.index = 1;
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.payload.slot.batch = BatchId(11);
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.stage = Stage::Simple;
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.device.generation = 2;
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.source = SourceToken(101);
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.config = Arc::new((*old).clone());
    cases.push(wrong);
    let mut wrong = copy();
    wrong.event.at_ns = MonoTime(99);
    cases.push(wrong);
    let mut wrong = copy();
    wrong.delivered_at = MonoTime(99);
    cases.push(wrong);
    let mut wrong = copy();
    wrong.delivered_at = MonoTime(101);
    cases.push(wrong);
    for wrong in cases {
        assert!(
            arena
                .prepare_child_with_down(
                    origin(old.clone(), 20, Stage::Output),
                    1,
                    wrong,
                    producer()
                )
                .is_err()
        );
        prepared_parent_is_unchanged(&arena);
    }
}
#[test]
fn live_and_released_parent_cannot_be_reactivated_by_replayed_down() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    arena.insert(slot(10), producer()).unwrap();
    assert!(matches!(
        arena.prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old.clone(), Stage::Complex),
            producer()
        ),
        Err(BindingError::Lifecycle)
    ));
    arena.take(slot(10)).unwrap();
    assert!(matches!(
        arena.prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old, Stage::Complex),
            producer()
        ),
        Err(BindingError::Lifecycle)
    ));
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_sources(), 1);
}
#[test]
fn child_provenance_and_capacity_rejection_do_not_activate_reserved_parent() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    let base = origin(old.clone(), 20, Stage::Output);
    let mut cases = vec![];
    let mut bad = base.clone();
    bad.stage = Stage::Complex;
    cases.push(bad);
    let mut bad = base.clone();
    bad.device.id = 2;
    cases.push(bad);
    let mut bad = base.clone();
    bad.source = SourceToken(101);
    cases.push(bad);
    let mut bad = base.clone();
    bad.config = Arc::new((*old).clone());
    cases.push(bad);
    let mut bad = base.clone();
    bad.start = MonoTime(99);
    cases.push(bad);
    let mut bad = base.clone();
    bad.batch = BatchId(10);
    cases.push(bad);
    for bad in cases {
        assert!(
            arena
                .prepare_child_with_down(bad, 1, receipt(old.clone(), Stage::Complex), producer())
                .is_err()
        );
        prepared_parent_is_unchanged(&arena);
    }
    for count in [0, MAX_OUTPUT_OWNERS] {
        assert!(matches!(
            arena.prepare_child_with_down(
                base.clone(),
                count,
                receipt(old.clone(), Stage::Complex),
                producer()
            ),
            Err(BindingError::Capacity)
        ));
        prepared_parent_is_unchanged(&arena);
    }
    // Legacy child admission still requires an already Live parent.
    assert!(matches!(
        arena.prepare(base, 1, Some(slot(10))),
        Err(BindingError::Lifecycle)
    ));
    let mut terminal_parent = BindingArena::<Producer>::default();
    terminal_parent
        .prepare(origin(old.clone(), 10, Stage::Output), 1, None)
        .unwrap()
        .commit();
    assert!(matches!(
        terminal_parent.prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old, Stage::Output),
            producer()
        ),
        Err(BindingError::Provenance)
    ));
}
#[test]
fn output_abort_keeps_pending_parent_reserved_and_retries_original_child_after_reload() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old.clone(), Stage::Complex),
            producer(),
        )
        .unwrap();
    output
        .prepare_child_scoped(snapshot(old.clone()), registration)
        .unwrap()
        .abort();
    prepared_parent_is_unchanged(&arena);
    assert_eq!(output.pending_batches(), 0);
    assert_eq!(output.reserved_slots(), 0);
    assert!(output.generated_variables().is_empty());
    assert!(output.pressed_outputs().is_empty());
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old.clone(), Stage::Complex),
            producer(),
        )
        .unwrap();
    output
        .prepare_child_scoped(snapshot(old), registration)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(arena.reserved_sources(), 4);
    assert_eq!(
        output.generated_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    assert!(output.executed_variables().is_empty());
    while output.poll(MonoTime(150)).unwrap().is_some() {}
    assert_eq!(
        output.executed_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    assert_eq!(output.pending_batches(), 0);
    assert!(matches!(
        arena.take(slot(10)),
        Err(BindingError::ChildPending)
    ));
    arena.cancel_subtree(BatchId(20)).unwrap();
    arena.take(slot(10)).unwrap();
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn terminal_key_capacity_failure_leaves_parent_unexecuted_after_generated_variable_preparation() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut output = output(old.clone());
    let actions: Vec<Action> = (1..=512)
        .map(|usage| KeyId::new(CONSUMER_PAGE, usage).into())
        .collect();
    output
        .enqueue(plan_actions(&actions, origin(old.clone(), 1, Stage::Output), false).unwrap())
        .unwrap();
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old.clone(), Stage::Complex),
            producer(),
        )
        .unwrap();
    assert!(matches!(
        output.prepare_child_scoped(snapshot(old), registration),
        Err(OutputStageError::Terminal(ActorError::Capacity))
    ));
    prepared_parent_is_unchanged(&arena);
    assert!(output.generated_variables().is_empty());
    assert_eq!(output.pending_batches(), 1);
    assert!(!output.is_failed());
}
#[test]
fn full_complex_queue_rejection_preserves_original_simple_down_and_retries_same_frozen_plan() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Simple);
    let mut queue = GeneratedStageQueue::new(Stage::Complex, old.clone()).unwrap();
    queue.observe_device(device(), false).unwrap();
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for batch in 1..=8 {
        let plan =
            plan_actions(&actions, origin(old.clone(), batch, Stage::Complex), true).unwrap();
        queue.enqueue(plan.events().to_vec()).unwrap();
    }
    let selection = select_rule(
        old.clone(),
        MappingStage::Simple,
        key("a"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    let plan = ScopedActionPlan::main(
        &selection,
        BatchId(20),
        MonoTime(100),
        device(),
        SourceToken(100),
    )
    .unwrap();
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Complex),
            1,
            receipt(old.clone(), Stage::Simple),
            producer(),
        )
        .unwrap();
    assert!(matches!(
        queue.prepare_complex_child_scoped(&plan, registration),
        Err(GeneratedError::Timeline(TimelineError::Capacity))
    ));
    prepared_parent_is_unchanged(&arena);
    queue.cancel_batch(BatchId(1));
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Complex),
            1,
            receipt(old, Stage::Simple),
            producer(),
        )
        .unwrap();
    queue
        .prepare_complex_child_scoped(&plan, registration)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(arena.reserved_sources(), 2);
}
#[test]
fn snapshot_extra_overflow_aborts_parent_activation_even_after_child_slot_fits() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    arena
        .prepare(
            origin(old.clone(), 30, Stage::Simple),
            MAX_OUTPUT_OWNERS - 3,
            None,
        )
        .unwrap()
        .commit();
    let mut output = output(old.clone());
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output),
            1,
            receipt(old.clone(), Stage::Complex),
            producer(),
        )
        .unwrap();
    assert!(matches!(
        output.prepare_child_scoped(snapshot(old), registration),
        Err(OutputStageError::Binding(BindingError::Capacity))
    ));
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS - 2);
    assert_eq!(arena.extra_source_reservations(), 0);
    arena.check_down(slot(10)).unwrap();
    assert_eq!(output.pending_batches(), 0);
}
#[test]
fn actual_complex_delivery_and_output_child_commit_both_producer_bindings_then_cleanup_leaf_first()
{
    let old = config();
    let mut arena = arena(old.clone(), Stage::Simple);
    let mut complex = GeneratedStageQueue::new(Stage::Complex, old.clone()).unwrap();
    complex.observe_device(device(), false).unwrap();
    let mut output = output(old.clone());
    let current = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    complex.activate(current.clone()).unwrap();
    output.activate(current).unwrap();
    let selection = select_rule(
        old.clone(),
        MappingStage::Simple,
        key("a"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    let plan = ScopedActionPlan::main(
        &selection,
        BatchId(20),
        MonoTime(100),
        device(),
        SourceToken(100),
    )
    .unwrap();
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Complex),
            1,
            receipt(old.clone(), Stage::Simple),
            producer(),
        )
        .unwrap();
    let mut saved = complex
        .prepare_complex_child_scoped(&plan, registration)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 1);
    assert!(arena.live(slot(20)).is_none());
    let due = complex.poll(MonoTime(125)).unwrap().unwrap();
    assert_eq!(due.delivered_at, MonoTime(125));
    let selection = select_rule(
        due.event.config.clone(),
        MappingStage::Complex,
        key("b"),
        complex.modifiers(),
        |_| Truth::True,
    )
    .unwrap();
    let child = ScopedActionPlan::main(
        &selection,
        BatchId(40),
        due.delivered_at,
        device(),
        SourceToken(100),
    )
    .unwrap();
    let mut child_origin = origin(old.clone(), 40, Stage::Output);
    child_origin.start = MonoTime(125);
    let registration = arena
        .prepare_child_with_down(child_origin, 1, due, producer())
        .unwrap();
    output
        .prepare_child_scoped(child, registration)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 2);
    assert_eq!(arena.reserved_sources(), 3);
    assert!(matches!(
        arena.take(slot(20)),
        Err(BindingError::ChildPending)
    ));
    let emitted = output.poll(MonoTime(125)).unwrap().unwrap();
    assert_eq!(emitted.outputs[0].key, key("c"));
    assert_eq!(emitted.outputs[0].edge, KeyEdge::Down);
    complex.input_up_scoped(&mut saved, MonoTime(150)).unwrap();
    let up = complex.poll(MonoTime(150)).unwrap().unwrap();
    assert_eq!(up.event.payload.edge, KeyEdge::Up);
    output.input_up(BatchId(40), MonoTime(150)).unwrap();
    let up = output.poll(MonoTime(150)).unwrap().unwrap();
    assert_eq!(up.outputs[0].edge, KeyEdge::Up);
    assert_eq!(up.completed_batch, Some(BatchId(40)));
    arena.cancel_subtree(BatchId(40)).unwrap();
    arena.take(slot(20)).unwrap();
    arena.retire(BatchId(20)).unwrap();
    complex.retire_batch(BatchId(20)).unwrap();
    arena.take(slot(10)).unwrap();
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn late_rebased_down_accepts_future_child_start_without_rewriting_parent_origin() {
    let old = config();
    let mut arena = arena(old.clone(), Stage::Complex);
    let mut due = receipt(old.clone(), Stage::Complex);
    due.event.at_ns = MonoTime(120);
    due.delivered_at = MonoTime(125);
    let mut child_origin = origin(old.clone(), 20, Stage::Output);
    child_origin.start = MonoTime(130);
    arena
        .prepare_child_with_down(child_origin, 1, due, producer())
        .unwrap()
        .commit();
    let mut origins = vec![];
    arena
        .visit_subtree(slot(10), |node| {
            origins.push((node.origin.batch, node.origin.start))
        })
        .unwrap();
    assert_eq!(
        origins,
        [(BatchId(10), MonoTime(100)), (BatchId(20), MonoTime(130))]
    );
    let canceled = arena.cancel_subtree(BatchId(10)).unwrap();
    assert_eq!(canceled[0].payloads.len(), 0);
    assert_eq!(canceled[1].payloads[0].payload.label, "original producer");
}
