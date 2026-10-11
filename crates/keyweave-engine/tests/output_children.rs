#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::GeneratedError, modifier_scope::ScopedActionPlan,
    output_stage::*, rule_selection::*, stage_bindings::*, terminal_actor::ActorError, timeline::*,
    *,
};
use keyweave_types::*;
use std::sync::Arc;
struct Producer;
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
        name: "children".into(),
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
fn parent() -> ActionSlot {
    ActionSlot {
        batch: BatchId(10),
        index: 0,
    }
}
fn origin(config: Arc<ConfigSnapshot>, id: u64, stage: Stage, start: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(id),
        stage,
        start: MonoTime(start),
        device: device(1),
        source: SourceToken(100),
        config,
    }
}
fn arena(config: Arc<ConfigSnapshot>) -> BindingArena<Producer> {
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(config, 10, Stage::Complex, 0), 1, None)
        .unwrap()
        .commit();
    arena.insert(parent(), Producer).unwrap();
    arena
}
fn output(config: Arc<ConfigSnapshot>) -> OutputStageActor {
    let mut output = OutputStageActor::new(config).unwrap();
    for id in [1, 2] {
        output
            .observe_device(device(id), false, MonoTime(0))
            .unwrap();
    }
    output
}
fn child(config: Arc<ConfigSnapshot>, actions: &[Action], tap: bool) -> ActionPlan {
    plan_actions(actions, origin(config, 20, Stage::Output, 100), tap).unwrap()
}
fn drain(output: &mut OutputStageActor, at: u64) -> (Vec<(String, KeyEdge)>, Vec<BatchId>) {
    let mut outputs = vec![];
    let mut completed = vec![];
    while let Some(step) = output.poll(MonoTime(at)).unwrap() {
        outputs.extend(step.outputs.into_iter().map(|o| (o.key.name(), o.edge)));
        completed.extend(step.completed_batch);
    }
    (outputs, completed)
}
fn tree_size(arena: &BindingArena<Producer>) -> usize {
    let mut count = 0;
    arena.visit_subtree(parent(), |_| count += 1).unwrap();
    count
}
fn shift(output: &mut OutputStageActor) {
    let mut origin = origin(output.snapshot().clone(), 1, Stage::Output, 0);
    origin.device = device(2);
    output
        .enqueue(plan_actions(&[key("left_shift").into()], origin, false).unwrap())
        .unwrap();
    drain(output, 0);
}
fn selected(output: &OutputStageActor) -> RuleSelection {
    output
        .select(
            key("a"),
            &ContextSnapshot::default(),
            device(1),
            MonoTime(0),
        )
        .unwrap()
        .unwrap()
}
#[test]
fn live_old_parent_authorizes_child_after_reload_and_parent_waits_for_registry_cleanup() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let registration = arena
        .prepare(
            origin(old.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    output
        .prepare_child(child(old.clone(), &[key("b").into()], false), registration)
        .unwrap()
        .commit();
    assert_eq!(output.parent_binding(BatchId(20)), Some(parent()));
    assert!(matches!(
        arena.take(parent()),
        Err(BindingError::ChildPending)
    ));
    assert_eq!(drain(&mut output, 100).0, [("b".into(), KeyEdge::Down)]);
    assert_eq!(output.physical_repeat_target(BatchId(20)), Some(key("b")));
    output.input_up(BatchId(20), MonoTime(110)).unwrap();
    assert_eq!(drain(&mut output, 110).1, [BatchId(20)]);
    assert_eq!(output.pending_batches(), 0);
    assert!(matches!(
        arena.take(parent()),
        Err(BindingError::ChildPending)
    ));
    let cleanup = arena.cancel_subtree(BatchId(20)).unwrap();
    assert_eq!(cleanup.len(), 1);
    assert_eq!(cleanup[0].parent, Some(parent()));
    assert!(Arc::ptr_eq(&cleanup[0].origin.config, &old));
    assert!(cleanup[0].payloads.is_empty());
    arena.take(parent()).unwrap();
    arena.retire(BatchId(10)).unwrap();
    assert_eq!(arena.reserved_slots(), 0);
}
#[test]
fn old_roots_and_parentless_reservations_are_rejected_without_consuming_identity() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let plan = child(old.clone(), &[key("b").into()], false);
    assert_eq!(
        output.enqueue(plan.clone()),
        Err(OutputStageError::Generated(GeneratedError::Provenance))
    );
    let registration = arena
        .prepare(origin(old, 20, Stage::Output, 100), 1, None)
        .unwrap();
    assert!(matches!(
        output.prepare_child(plan, registration),
        Err(OutputStageError::Binding(BindingError::Provenance))
    ));
    assert_eq!(arena.reserved_slots(), 1);
    assert_eq!(output.pending_batches(), 0);
    output
        .enqueue(child(output.snapshot().clone(), &[key("c").into()], false))
        .unwrap();
    assert_eq!(output.parent_binding(BatchId(20)), None);
}
#[test]
fn dropping_three_party_preparation_keeps_variable_parent_edge_and_output_state_unchanged() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let plan = child(old.clone(), &[variable()], true);
    let registration = arena
        .prepare(
            origin(old.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    output
        .prepare_child(plan.clone(), registration)
        .unwrap()
        .abort();
    assert_eq!(tree_size(&arena), 1);
    assert_eq!(arena.reserved_slots(), 1);
    assert!(output.generated_variables().is_empty());
    assert_eq!(output.reserved_slots(), 0);
    assert_eq!(output.pending_batches(), 0);
    assert_eq!(output.queued_events(), 0);
    let registration = arena
        .prepare(origin(old, 20, Stage::Output, 100), 1, Some(parent()))
        .unwrap();
    output.prepare_child(plan, registration).unwrap().commit();
    assert_eq!(tree_size(&arena), 2);
    assert_eq!(output.generated_variables()["v"], VariableValue::Integer(2));
    assert!(output.executed_variables().is_empty());
    drain(&mut output, 150);
    assert_eq!(output.executed_variables()["v"], VariableValue::Integer(2));
}
#[test]
fn child_witness_requires_exact_batch_start_device_source_arc_stage_and_slot_count() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let correct = origin(old.clone(), 20, Stage::Output, 100);
    let mut cases = vec![];
    let mut wrong = correct.clone();
    wrong.batch = BatchId(21);
    cases.push(wrong);
    let mut wrong = correct.clone();
    wrong.start = MonoTime(101);
    cases.push(wrong);
    let mut wrong = correct.clone();
    wrong.device = device(2);
    cases.push(wrong);
    let mut wrong = correct.clone();
    wrong.source = SourceToken(101);
    cases.push(wrong);
    let mut wrong = correct.clone();
    wrong.config = Arc::new((*old).clone());
    cases.push(wrong);
    let mut wrong = correct.clone();
    wrong.stage = Stage::Complex;
    cases.push(wrong);
    for wrong in cases {
        let plan = plan_actions(&[key("b").into()], wrong, false).unwrap();
        let registration = arena.prepare(correct.clone(), 1, Some(parent())).unwrap();
        assert!(output.prepare_child(plan, registration).is_err());
        assert_eq!(tree_size(&arena), 1);
        assert_eq!(output.reserved_slots(), 0);
        assert!(!output.is_failed());
    }
    let registration = arena.prepare(correct.clone(), 2, Some(parent())).unwrap();
    assert!(
        output
            .prepare_child(child(old, &[key("b").into()], false), registration)
            .is_err()
    );
    assert_eq!(arena.reserved_slots(), 1);
    assert_eq!(tree_size(&arena), 1);
}
#[test]
fn full_output_queue_rejection_drops_parent_registration_and_same_child_can_retry() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 1..=8 {
        output
            .enqueue(
                plan_actions(
                    &actions,
                    origin(output.snapshot().clone(), id, Stage::Output, 0),
                    true,
                )
                .unwrap(),
            )
            .unwrap();
    }
    let registration = arena
        .prepare(
            origin(old.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    let plan = child(old.clone(), &[variable()], true);
    assert!(matches!(
        output.prepare_child(plan.clone(), registration),
        Err(OutputStageError::Generated(GeneratedError::Timeline(
            TimelineError::Capacity
        )))
    ));
    assert_eq!(tree_size(&arena), 1);
    assert!(output.generated_variables().is_empty());
    assert_eq!(output.pending_batches(), 8);
    output.cancel_batch(BatchId(1), MonoTime(0)).unwrap();
    let registration = arena
        .prepare(origin(old, 20, Stage::Output, 100), 1, Some(parent()))
        .unwrap();
    output.prepare_child(plan, registration).unwrap().commit();
    drain(&mut output, 5000);
    assert_eq!(output.pending_batches(), 0);
    assert_eq!(output.executed_variables()["v"], VariableValue::Integer(2));
    assert_eq!(tree_size(&arena), 2);
}
#[test]
fn terminal_key_budget_rejection_aborts_generated_modifier_and_upstream_edge() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let actions: Vec<Action> = (1..=512)
        .map(|usage| KeyId::new(12, usage).into())
        .collect();
    output
        .enqueue(
            plan_actions(
                &actions,
                origin(output.snapshot().clone(), 1, Stage::Output, 0),
                false,
            )
            .unwrap(),
        )
        .unwrap();
    let plan = child(old.clone(), &[key("left_shift").into()], false);
    let registration = arena
        .prepare(
            origin(old.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    assert!(matches!(
        output.prepare_child(plan.clone(), registration),
        Err(OutputStageError::Terminal(ActorError::Capacity))
    ));
    assert!(output.generated_modifiers().is_empty());
    assert_eq!(tree_size(&arena), 1);
    assert_eq!(output.pending_batches(), 1);
    output.cancel_batch(BatchId(1), MonoTime(0)).unwrap();
    let registration = arena
        .prepare(origin(old, 20, Stage::Output, 100), 1, Some(parent()))
        .unwrap();
    output.prepare_child(plan, registration).unwrap().commit();
    assert_eq!(
        output.generated_modifiers(),
        [key("left_shift")].into_iter().collect()
    );
}
#[test]
fn frozen_old_main_scope_is_authorized_by_live_parent_and_original_up_restores_modifier() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    shift(&mut output);
    let selected = selected(&output);
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let plan = ScopedActionPlan::main(
        &selected,
        BatchId(20),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap();
    let registration = arena
        .prepare(origin(old, 20, Stage::Output, 100), 1, Some(parent()))
        .unwrap();
    output
        .prepare_child_scoped(plan, registration)
        .unwrap()
        .commit();
    assert_eq!(
        output.generated_modifiers(),
        [key("right_control")].into_iter().collect()
    );
    assert_eq!(
        drain(&mut output, 100).0,
        [
            ("left_shift".into(), KeyEdge::Up),
            ("right_control".into(), KeyEdge::Down)
        ]
    );
    output.input_up(BatchId(20), MonoTime(110)).unwrap();
    assert_eq!(
        drain(&mut output, 110).0,
        [
            ("right_control".into(), KeyEdge::Up),
            ("left_shift".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(output.pending_batches(), 1);
    assert_eq!(tree_size(&arena), 2);
}
#[test]
fn old_snapshot_branch_retains_variable_and_extra_source_budget_until_actual_completion() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    shift(&mut output);
    let branch = selected(&output)
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let plan = ScopedActionPlan::branch(
        &branch,
        BatchId(20),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap();
    let registration = arena
        .prepare(origin(old, 20, Stage::Output, 100), 1, Some(parent()))
        .unwrap();
    output
        .prepare_child_scoped(plan, registration)
        .unwrap()
        .commit();
    assert_eq!(output.reserved_slots(), 3);
    assert_eq!(arena.reserved_slots(), 2);
    assert_eq!(output.generated_variables()["v"], VariableValue::Integer(2));
    drain(&mut output, 100);
    assert_eq!(output.executed_variables()["v"], VariableValue::Integer(1));
    assert_eq!(drain(&mut output, 150).1, [BatchId(20)]);
    assert_eq!(output.reserved_slots(), 1);
}
#[test]
fn output_reset_does_not_resurrect_old_variable_namespace_from_external_parent() {
    let old = config();
    let mut arena = arena(old.clone());
    let mut output = output(old.clone());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    output.reset(MonoTime(0)).unwrap();
    let registration = arena
        .prepare(
            origin(old.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    assert!(matches!(
        output.prepare_child(child(old, &[variable()], false), registration),
        Err(OutputStageError::Generated(GeneratedError::Engine(
            EngineError::InvalidVariable
        )))
    ));
    assert_eq!(tree_size(&arena), 1);
    assert!(output.generated_variables().is_empty());
    assert_eq!(output.pending_batches(), 0);
}
#[test]
fn reserved_parent_cannot_authorize_child_and_positive_slot_registration_cannot_admit_empty_plan() {
    let old = config();
    let mut output = output(old.clone());
    let mut arena = BindingArena::<Producer>::default();
    arena
        .prepare(origin(old.clone(), 10, Stage::Complex, 0), 1, None)
        .unwrap()
        .commit();
    assert!(matches!(
        arena.prepare(
            origin(old.clone(), 20, Stage::Output, 100),
            1,
            Some(parent())
        ),
        Err(BindingError::Lifecycle)
    ));
    arena.insert(parent(), Producer).unwrap();
    let registration = arena
        .prepare(
            origin(old.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    assert!(
        output
            .prepare_child(child(old, &[], false), registration)
            .is_err()
    );
    assert_eq!(tree_size(&arena), 1);
    assert_eq!(output.pending_batches(), 0);
}
#[test]
fn invalid_old_schema_is_rejected_even_with_matching_child_registration() {
    let mut invalid = (*config()).clone();
    invalid.complex[0].extras.if_alone[0].target = ActionTarget::SetVariable(VariableAction {
        name: "".into(),
        operation: VariableOperation::Unset,
    });
    let invalid = Arc::new(invalid);
    let mut arena = arena(invalid.clone());
    let mut output = output(config());
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let registration = arena
        .prepare(
            origin(invalid.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    assert!(matches!(
        output.prepare_child(child(invalid, &[key("b").into()], false), registration),
        Err(OutputStageError::Generated(GeneratedError::Engine(
            EngineError::InvalidVariable
        )))
    ));
    assert_eq!(tree_size(&arena), 1);
    assert_eq!(output.pending_batches(), 0);
}

#[test]
fn future_or_same_generation_foreign_arc_is_not_a_retained_parent_snapshot() {
    let current = config();
    for generation in [1, 3] {
        let mut foreign = (*current).clone();
        foreign.generation = ConfigGeneration(generation);
        let foreign = Arc::new(foreign);
        let mut arena = arena(foreign.clone());
        let mut output = output(current.clone());
        let registration = arena
            .prepare(
                origin(foreign.clone(), 20, Stage::Output, 100),
                1,
                Some(parent()),
            )
            .unwrap();
        assert!(matches!(
            output.prepare_child(child(foreign, &[key("b").into()], false), registration),
            Err(OutputStageError::Generated(GeneratedError::Provenance))
        ));
        assert_eq!(tree_size(&arena), 1);
        assert_eq!(output.pending_batches(), 0);
    }
}

#[test]
fn root_preparation_abort_keeps_both_output_views_and_strict_identity_uncommitted() {
    let current = config();
    let mut output = output(current.clone());
    output
        .prepare(child(current.clone(), &[variable()], true))
        .unwrap()
        .abort();
    assert!(output.generated_variables().is_empty());
    assert!(output.executed_variables().is_empty());
    assert_eq!(output.pending_batches(), 0);
    assert_eq!(output.reserved_slots(), 0);
    assert_eq!(output.queued_events(), 0);
    let plan = plan_actions(
        &[key("b").into()],
        origin(current, 1, Stage::Output, 100),
        false,
    )
    .unwrap();
    output.prepare(plan).unwrap().commit();
    assert_eq!(output.physical_repeat_target(BatchId(1)), None);
    drain(&mut output, 100);
    assert_eq!(output.physical_repeat_target(BatchId(1)), Some(key("b")));
}
#[test]
fn child_from_exact_current_arc_uses_same_registered_parent_and_empty_extra_budget() {
    let current = config();
    let mut arena = arena(current.clone());
    let mut output = output(current.clone());
    let registration = arena
        .prepare(
            origin(current.clone(), 20, Stage::Output, 100),
            1,
            Some(parent()),
        )
        .unwrap();
    output
        .prepare_child(child(current, &[key("b").into()], true), registration)
        .unwrap()
        .commit();
    assert_eq!(output.parent_binding(BatchId(20)), Some(parent()));
    assert_eq!(output.reserved_slots(), 1);
    assert_eq!(drain(&mut output, 100).1, [BatchId(20)]);
    assert_eq!(output.reserved_slots(), 0);
    let cleanup = arena.cancel_subtree(BatchId(20)).unwrap();
    assert!(cleanup[0].payloads.is_empty());
    assert_eq!(tree_size(&arena), 1);
}
