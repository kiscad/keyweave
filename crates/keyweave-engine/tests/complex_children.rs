#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::*, modifier_scope::ScopedActionPlan,
    output_stage::OutputStageActor, rule_selection::*, stage_bindings::*, timeline::*, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
struct Producer;
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
        name: "simple-parent".into(),
        simple: vec![Mapping {
            id: "simple".into(),
            from: key("a"),
            to: vec![Action {
                hold_down_ns: 50,
                ..key("b").into()
            }],
            allow_extra_modifiers: true,
            extras,
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
fn arena(config: Arc<ConfigSnapshot>) -> BindingArena<Producer> {
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(config, 10, Stage::Simple), 1, None)
        .unwrap()
        .commit();
    arena.insert(slot(10), Producer).unwrap();
    arena
}
fn queue(config: Arc<ConfigSnapshot>) -> GeneratedStageQueue {
    let mut queue = GeneratedStageQueue::new(Stage::Complex, config).unwrap();
    queue.observe_device(device(), false).unwrap();
    queue
}
fn plan(config: Arc<ConfigSnapshot>, snapshot: bool) -> ScopedActionPlan {
    let selected = select_rule(
        config,
        MappingStage::Simple,
        key("a"),
        BTreeSet::from([key("left_shift"), key("left_control")]),
        |_| Truth::True,
    )
    .unwrap();
    if snapshot {
        ScopedActionPlan::branch(
            &selected
                .select_branch(RuleBranch::Alone, false, |_| Truth::True)
                .unwrap(),
            BatchId(20),
            MonoTime(100),
            device(),
            SourceToken(100),
        )
        .unwrap()
    } else {
        ScopedActionPlan::main(
            &selected,
            BatchId(20),
            MonoTime(100),
            device(),
            SourceToken(100),
        )
        .unwrap()
    }
}
fn count(arena: &BindingArena<Producer>) -> usize {
    let mut count = 0;
    arena.visit_subtree(slot(10), |_| count += 1).unwrap();
    count
}
fn register<'a>(
    arena: &'a mut BindingArena<Producer>,
    config: Arc<ConfigSnapshot>,
) -> Reservation<'a, Producer> {
    arena
        .prepare(origin(config, 20, Stage::Complex), 1, Some(slot(10)))
        .unwrap()
}
#[test]
fn old_simple_parent_generates_child_and_saved_up_across_reload_without_premature_parent_release() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut queue = queue(old.clone());
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let mut saved = queue
        .prepare_complex_child_scoped(&plan(old.clone(), false), register(&mut arena, old.clone()))
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_sources(), 2);
    assert_eq!(queue.reserved_slots(), 1);
    assert!(matches!(
        arena.take(slot(10)),
        Err(BindingError::ChildPending)
    ));
    let down = queue.poll(MonoTime(100)).unwrap().unwrap();
    assert_eq!(down.event.payload.edge, KeyEdge::Down);
    assert!(Arc::ptr_eq(&down.event.config, &old));
    assert_eq!(down.event.payload.target, ActionTarget::Key(key("b")));
    assert!(queue.poll(MonoTime(100)).unwrap().is_none());
    assert!(queue.poll(MonoTime(109)).unwrap().is_none());
    queue.input_up_scoped(&mut saved, MonoTime(110)).unwrap();
    let up = queue.poll(MonoTime(110)).unwrap().unwrap();
    assert_eq!(up.event.payload.slot, down.event.payload.slot);
    assert_eq!(up.event.payload.edge, KeyEdge::Up);
    assert!(Arc::ptr_eq(&up.event.config, &old));
    queue.retire_batch(BatchId(20)).unwrap();
    assert!(matches!(
        arena.take(slot(10)),
        Err(BindingError::ChildPending)
    ));
    let canceled = arena.cancel_subtree(BatchId(20)).unwrap();
    assert!(canceled[0].payloads.is_empty());
    assert!(Arc::ptr_eq(&canceled[0].origin.config, &old));
    arena.take(slot(10)).unwrap();
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn preparation_abort_keeps_generated_variable_scope_and_parent_tree_unchanged_and_allows_retry() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut queue = queue(old.clone());
    let shift = plan_actions(
        &[key("left_shift").into()],
        origin(old.clone(), 1, Stage::Complex),
        false,
    )
    .unwrap();
    queue.enqueue(shift.events().to_vec()).unwrap();
    let before = queue.modifiers();
    let planned = plan(old.clone(), true);
    queue
        .prepare_complex_child_scoped(&planned, register(&mut arena, old.clone()))
        .unwrap()
        .abort();
    assert_eq!(queue.modifiers(), before);
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert!(queue.variable_values().is_empty());
    assert_eq!(queue.queued_events(), 1);
    assert_eq!(arena.reserved_sources(), 1);
    assert_eq!(count(&arena), 1);
    {
        let _drop_guard = queue
            .prepare_complex_child_scoped(&planned, register(&mut arena, old.clone()))
            .unwrap();
    }
    assert_eq!(count(&arena), 1);
    let _saved = queue
        .prepare_complex_child_scoped(&planned, register(&mut arena, old))
        .unwrap()
        .commit();
    assert_eq!(arena.extra_source_reservations(), 2);
    assert_eq!(
        queue.variable_values().get("v"),
        Some(&VariableValue::Integer(2))
    );
    assert_eq!(count(&arena), 2);
}
#[test]
fn every_origin_field_and_down_slot_count_must_match_exclusive_registration() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut queue = queue(old.clone());
    let correct = origin(old.clone(), 20, Stage::Complex);
    let mut origins = vec![];
    let mut bad = correct.clone();
    bad.batch = BatchId(21);
    origins.push(bad);
    let mut bad = correct.clone();
    bad.start = MonoTime(101);
    origins.push(bad);
    let mut bad = correct.clone();
    bad.device.generation = 2;
    origins.push(bad);
    let mut bad = correct.clone();
    bad.source = SourceToken(101);
    origins.push(bad);
    let mut bad = correct.clone();
    bad.config = Arc::new((*old).clone());
    origins.push(bad);
    let mut bad = correct.clone();
    bad.stage = Stage::Output;
    origins.push(bad);
    for bad in origins {
        // Arena rejects each inconsistent registration before generation is possible.
        let reservation = arena.prepare(bad, 1, Some(slot(10)));
        match reservation {
            Err(_) => {}
            Ok(registration) => {
                assert!(
                    queue
                        .prepare_complex_child_scoped(&plan(old.clone(), false), registration)
                        .is_err()
                );
            }
        }
        assert_eq!(count(&arena), 1);
        assert_eq!(queue.queued_events(), 0);
    }
    let registration = arena.prepare(correct, 2, Some(slot(10))).unwrap();
    assert!(matches!(
        queue.prepare_complex_child_scoped(&plan(old.clone(), false), registration),
        Err(GeneratedError::Provenance)
    ));
    assert_eq!(count(&arena), 1);
}
#[test]
fn ordinary_old_roots_future_and_same_generation_foreign_parent_configs_are_rejected() {
    let old = config();
    let mut current = queue(old.clone());
    current
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert!(matches!(
        current.prepare_scoped(&plan(old.clone(), false)),
        Err(GeneratedError::Provenance)
    ));
    for generation in [1, 2] {
        let mut foreign = (*old).clone();
        foreign.generation = ConfigGeneration(generation);
        let foreign = Arc::new(foreign);
        let mut arena = arena(foreign.clone());
        let mut queue = queue(old.clone());
        assert!(matches!(
            queue.prepare_complex_child_scoped(
                &plan(foreign.clone(), false),
                register(&mut arena, foreign)
            ),
            Err(GeneratedError::Provenance)
        ));
        assert_eq!(arena.reserved_sources(), 1);
        assert_eq!(queue.pending_batches(), 0);
    }
}
#[test]
fn snapshot_sources_share_exact_tree_limit_and_wait_for_external_cleanup_after_delivery() {
    let old = config();
    let mut arena = arena(old.clone());
    arena
        .prepare(
            origin(old.clone(), 30, Stage::Simple),
            MAX_OUTPUT_OWNERS - 3,
            None,
        )
        .unwrap()
        .commit();
    let mut queue = queue(old.clone());
    let planned = plan(old.clone(), true);
    assert!(matches!(
        queue.prepare_complex_child_scoped(&planned, register(&mut arena, old.clone())),
        Err(GeneratedError::Binding(BindingError::Capacity))
    ));
    assert_eq!(arena.extra_source_reservations(), 0);
    assert!(queue.variable_values().is_empty());
    arena.cancel(BatchId(30)).unwrap();
    arena
        .prepare(
            origin(old.clone(), 30, Stage::Simple),
            MAX_OUTPUT_OWNERS - 4,
            None,
        )
        .unwrap()
        .commit();
    queue
        .prepare_complex_child_scoped(&planned, register(&mut arena, old.clone()))
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS);
    assert!(matches!(
        arena.prepare(origin(old, 40, Stage::Simple), 1, None),
        Err(BindingError::Capacity)
    ));
    while queue.poll(MonoTime(150)).unwrap().is_some() {}
    queue.retire_batch(BatchId(20)).unwrap();
    assert_eq!(queue.reserved_slots(), 0);
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS);
    let canceled = arena.cancel_subtree(BatchId(20)).unwrap();
    assert_eq!(canceled[0].extra_sources, 2);
    assert!(canceled[0].payloads.is_empty());
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS - 3);
}
#[test]
fn full_timeline_rejection_aborts_already_prepared_variable_scope_and_registration() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut queue = queue(old.clone());
    let targets = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for batch in 1..=8 {
        let planned =
            plan_actions(&targets, origin(old.clone(), batch, Stage::Complex), true).unwrap();
        queue.enqueue(planned.events().to_vec()).unwrap();
    }
    assert_eq!(queue.queued_events(), MAX_STAGE_EVENTS);
    let planned = plan(old.clone(), true);
    assert!(matches!(
        queue.prepare_complex_child_scoped(&planned, register(&mut arena, old.clone())),
        Err(GeneratedError::Timeline(TimelineError::Capacity))
    ));
    assert_eq!(arena.reserved_sources(), 1);
    assert_eq!(count(&arena), 1);
    assert!(queue.variable_values().is_empty());
    assert_eq!(queue.hidden_modifier_references(), 0);
    queue.cancel_batch(BatchId(1));
    queue
        .prepare_complex_child_scoped(&planned, register(&mut arena, old))
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_sources(), 4);
}
#[test]
fn non_complex_destination_and_parentless_registration_cannot_use_the_child_path() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut queue = queue(old.clone());
    let parentless = arena
        .prepare(origin(old.clone(), 20, Stage::Complex), 1, None)
        .unwrap();
    assert!(matches!(
        queue.prepare_complex_child_scoped(&plan(old.clone(), false), parentless),
        Err(GeneratedError::Binding(BindingError::Provenance))
    ));
    for stage in [Stage::Simple, Stage::Output] {
        let mut destination = GeneratedStageQueue::new(stage, old.clone()).unwrap();
        assert!(matches!(
            destination.prepare_complex_child_scoped(
                &plan(old.clone(), false),
                register(&mut arena, old.clone())
            ),
            Err(GeneratedError::Stage)
        ));
        assert_eq!(arena.reserved_sources(), 1);
    }
    let mut reserved = BindingArena::<Producer>::default();
    reserved
        .prepare(origin(old.clone(), 10, Stage::Simple), 1, None)
        .unwrap()
        .commit();
    assert!(matches!(
        reserved.prepare(origin(old, 20, Stage::Complex), 1, Some(slot(10))),
        Err(BindingError::Lifecycle)
    ));
}
#[test]
fn reset_does_not_resurrect_old_variable_names_through_external_live_parent() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut queue = queue(old.clone());
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    queue.reset(MonoTime(0)).unwrap();
    assert!(matches!(
        queue.prepare_complex_child_scoped(&plan(old.clone(), true), register(&mut arena, old)),
        Err(GeneratedError::Engine(EngineError::InvalidVariable))
    ));
    assert_eq!(arena.reserved_sources(), 1);
    assert!(queue.variable_values().is_empty());
    assert_eq!(queue.pending_batches(), 0);
}
#[test]
fn device_removal_discards_generated_child_without_claiming_external_binding_cleanup() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut queue = queue(old.clone());
    queue
        .prepare_complex_child_scoped(&plan(old.clone(), true), register(&mut arena, old))
        .unwrap()
        .commit();
    let (_, events) = queue.observe_device(device(), true).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(queue.reserved_slots(), 0);
    assert_eq!(queue.queued_events(), 0);
    assert_eq!(arena.reserved_sources(), 4);
    let canceled = arena.cancel_subtree(BatchId(10)).unwrap();
    assert_eq!(
        canceled
            .iter()
            .map(|batch| batch.extra_sources)
            .collect::<Vec<_>>(),
        [2, 0]
    );
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn retained_simple_complex_output_tree_uses_original_config_and_releases_children_before_parent() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut complex = queue(old.clone());
    let mut output = OutputStageActor::new(old.clone()).unwrap();
    output.observe_device(device(), false, MonoTime(0)).unwrap();
    let current = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    complex.activate(current.clone()).unwrap();
    output.activate(current).unwrap();
    let mut saved = complex
        .prepare_complex_child_scoped(&plan(old.clone(), false), register(&mut arena, old.clone()))
        .unwrap()
        .commit();
    let down = complex.poll(MonoTime(100)).unwrap().unwrap();
    arena.insert(down.event.payload.slot, Producer).unwrap();
    let selection = select_rule(
        down.event.config.clone(),
        MappingStage::Complex,
        key("b"),
        complex.modifiers(),
        |_| Truth::True,
    )
    .unwrap();
    let child = ScopedActionPlan::main(
        &selection,
        BatchId(40),
        MonoTime(100),
        device(),
        SourceToken(100),
    )
    .unwrap();
    let registration = arena
        .prepare(origin(old, 40, Stage::Output), 1, Some(slot(20)))
        .unwrap();
    output
        .prepare_child_scoped(child, registration)
        .unwrap()
        .commit();
    let down = output.poll(MonoTime(100)).unwrap().unwrap();
    assert_eq!(down.outputs[0].key, key("c"));
    assert!(complex.poll(MonoTime(109)).unwrap().is_none());
    complex.input_up_scoped(&mut saved, MonoTime(110)).unwrap();
    let up = complex.poll(MonoTime(110)).unwrap().unwrap();
    assert_eq!(up.event.payload.edge, KeyEdge::Up);
    assert!(matches!(
        arena.take(up.event.payload.slot),
        Err(BindingError::ChildPending)
    ));
    output.input_up(BatchId(40), MonoTime(110)).unwrap();
    let up = output.poll(MonoTime(110)).unwrap().unwrap();
    assert_eq!(up.outputs[0].edge, KeyEdge::Up);
    assert_eq!(up.completed_batch, Some(BatchId(40)));
    arena.cancel_subtree(BatchId(40)).unwrap();
    arena.take(slot(20)).unwrap();
    arena.retire(BatchId(20)).unwrap();
    complex.retire_batch(BatchId(20)).unwrap();
    arena.take(slot(10)).unwrap();
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
    assert_eq!(output.pending_batches(), 0);
    assert_eq!(complex.pending_batches(), 0);
}
