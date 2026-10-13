#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::*, output_stage::*, producer_decision::ProducerDecision,
    rule_selection::*, stage_bindings::*, timeline::*, *,
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
fn rule(from: &str, to: Vec<Action>) -> Mapping {
    Mapping {
        id: from.into(),
        from: key(from),
        to,
        allow_extra_modifiers: true,
        extras: Default::default(),
    }
}
fn config(simple: Vec<Mapping>, complex: Vec<Mapping>, generation: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "producer-decisions".into(),
        simple,
        complex,
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
fn queue(stage: Stage, config: Arc<ConfigSnapshot>) -> GeneratedStageQueue {
    let mut queue = GeneratedStageQueue::new(stage, config).unwrap();
    queue.observe_device(device(), false).unwrap();
    queue
}
fn down(
    config: Arc<ConfigSnapshot>,
    stage: Stage,
    target: Action,
    at: u64,
) -> DueEvent<PlannedAction> {
    let plan = plan_actions(&[target], origin(config, 10, stage, 100), false).unwrap();
    DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(at),
    }
}
fn targets(decision: &ProducerDecision) -> Vec<ActionTarget> {
    decision
        .plan_main(BatchId(20))
        .unwrap()
        .actions()
        .events()
        .iter()
        .filter(|event| event.payload.edge == KeyEdge::Down)
        .map(|event| event.payload.target.clone())
        .collect()
}
fn condition(value: i64) -> Condition {
    Condition::Variable {
        name: "v".into(),
        value: VariableValue::Integer(value),
        unless: false,
    }
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
        ..key("vk_none").into()
    }
}
fn set(queue: &mut GeneratedStageQueue, batch: u64, value: i64) {
    let plan = plan_actions(
        &[variable(value, None)],
        origin(queue.snapshot().clone(), batch, Stage::Output, 0),
        false,
    )
    .unwrap();
    queue.enqueue(plan.events().to_vec()).unwrap();
}
#[test]
fn saved_event_arc_and_actual_delivery_time_win_over_current_config_without_mutating_clock() {
    let old = config(vec![], vec![rule("a", vec![key("b").into()])], 1);
    let mut queue = queue(Stage::Output, old.clone());
    queue
        .activate(config(vec![], vec![rule("a", vec![key("c").into()])], 2))
        .unwrap();
    let receipt = down(old.clone(), Stage::Complex, key("a").into(), 125);
    let saved = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    assert_eq!(targets(&saved), [ActionTarget::Key(key("b"))]);
    assert_eq!(saved.destination(), Stage::Output);
    assert_eq!(saved.delivered_at(), MonoTime(125));
    assert_eq!(saved.selection().unwrap().stage(), MappingStage::Complex);
    assert!(Arc::ptr_eq(saved.selection().unwrap().snapshot(), &old));
    let plan = saved.plan_main(BatchId(20)).unwrap();
    assert_eq!(plan.actions().events()[0].at_ns, MonoTime(125));
    assert_eq!(plan.actions().events()[0].source, SourceToken(100));
    assert!(Arc::ptr_eq(&plan.actions().events()[0].config, &old));
    assert_eq!(
        queue
            .select(
                key("a"),
                &ContextSnapshot::default(),
                device(),
                MonoTime(125)
            )
            .unwrap()
            .unwrap()
            .actions()
            .next()
            .unwrap()
            .key(),
        Some(key("c"))
    );
    assert_eq!(queue.queued_events(), 0);
    assert_eq!(queue.pending_batches(), 0);
    assert!(queue.poll(MonoTime(50)).unwrap().is_none());
    // Selection never authorizes an old root without the retained parent transaction.
    assert!(matches!(
        queue.prepare_scoped(&plan),
        Err(GeneratedError::Provenance)
    ));
}
#[test]
fn frozen_main_indices_survive_destination_variable_change_and_reload() {
    let old = config(
        vec![],
        vec![rule(
            "a",
            vec![
                Action {
                    conditions: vec![condition(1)],
                    ..key("b").into()
                },
                Action {
                    conditions: vec![condition(2)],
                    ..key("c").into()
                },
            ],
        )],
        1,
    );
    let mut queue = queue(Stage::Output, old.clone());
    set(&mut queue, 1, 1);
    let receipt = down(old, Stage::Complex, key("a").into(), 125);
    let saved = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    set(&mut queue, 2, 2);
    queue.activate(config(vec![], vec![], 2)).unwrap();
    assert_eq!(targets(&saved), [ActionTarget::Key(key("b"))]);
    assert_eq!(
        targets(
            &queue
                .decide_producer_down(&receipt, &ContextSnapshot::default())
                .unwrap()
        ),
        [ActionTarget::Key(key("c"))]
    );
    assert_eq!(
        saved.plan_main(BatchId(21)).unwrap().actions().events()[0]
            .payload
            .slot
            .batch,
        BatchId(21)
    );
}
#[test]
fn destination_generated_modifier_view_is_captured_before_any_timed_terminal_delivery() {
    let mut mapping = rule("a", vec![key("b").into()]);
    mapping
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    let cfg = config(vec![mapping], vec![], 1);
    let mut queue = queue(Stage::Complex, cfg.clone());
    let plan = plan_actions(
        &[Action {
            lazy: true,
            ..key("left_shift").into()
        }],
        origin(cfg.clone(), 1, Stage::Complex, 1000),
        false,
    )
    .unwrap();
    queue.enqueue(plan.events().to_vec()).unwrap();
    let receipt = down(cfg, Stage::Simple, key("a").into(), 125);
    let decision = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    let selected = decision.selection().unwrap();
    assert_eq!(selected.stage(), MappingStage::Simple);
    assert!(selected.modifiers_at_down().contains(&key("left_shift")));
    assert!(selected.consumed_modifiers().contains(&key("left_shift")));
    assert_eq!(targets(&decision), [ActionTarget::Key(key("b"))]);
    assert_eq!(queue.next_deadline(), Some(MonoTime(1000)));
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert!(queue.poll(MonoTime(125)).unwrap().is_none());
}
#[test]
fn context_expiry_is_evaluated_at_delivery_not_original_earliest_and_unless_unknown_does_not_match()
{
    for unless in [false, true] {
        let mut restricted = rule("a", vec![key("b").into()]);
        restricted.extras.conditions.push(Condition::Application {
            bundle_identifiers: vec![],
            file_paths: vec![Pattern::compile(if unless { "never" } else { "editor" }).unwrap()],
            unless,
        });
        let cfg = config(
            vec![],
            vec![restricted, rule("a", vec![key("c").into()])],
            1,
        );
        let queue = queue(Stage::Output, cfg.clone());
        let context = ContextSnapshot {
            revision: 1,
            provider: "test".into(),
            observed_at_ns: MonoTime(100),
            valid_until_ns: MonoTime(120),
            application: Some(ApplicationContext {
                file_path: Some("editor".into()),
                bundle_identifier: None,
            }),
            ..Default::default()
        };
        let early = down(cfg.clone(), Stage::Complex, key("a").into(), 110);
        assert_eq!(
            targets(&queue.decide_producer_down(&early, &context).unwrap()),
            [ActionTarget::Key(key("b"))]
        );
        let late = down(cfg, Stage::Complex, key("a").into(), 125);
        assert_eq!(
            targets(&queue.decide_producer_down(&late, &context).unwrap()),
            [ActionTarget::Key(key("c"))]
        );
    }
}
#[test]
fn unmatched_passthrough_preserves_lazy_target_and_saved_up_but_halt_remains_upstream_metadata() {
    let cfg = config(vec![], vec![], 1);
    let queue = queue(Stage::Output, cfg.clone());
    let receipt = down(
        cfg.clone(),
        Stage::Complex,
        Action {
            lazy: true,
            halt: true,
            ..key("spacebar").into()
        },
        125,
    );
    let decision = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    assert!(decision.selection().is_none());
    assert!(decision.originating_halt());
    assert_eq!(
        decision.down().payload.slot,
        ActionSlot {
            batch: BatchId(10),
            index: 0
        }
    );
    let plan = decision.plan_main(BatchId(20)).unwrap();
    assert_eq!(plan.actions().events().len(), 1);
    let generated = &plan.actions().events()[0];
    assert!(generated.payload.lazy);
    assert!(!generated.payload.halt);
    assert_eq!(generated.payload.target, ActionTarget::Key(key("spacebar")));
    assert!(Arc::ptr_eq(&generated.config, &cfg));
    let up = plan.actions().input_up_events(MonoTime(150));
    assert_eq!(up.len(), 1);
    assert!(up[0].payload.lazy);
    assert_eq!(up[0].payload.edge, KeyEdge::Up);
    assert_eq!(up[0].payload.slot, generated.payload.slot);
}
#[test]
fn variable_passthrough_waits_original_up_and_preserves_assignment_instead_of_remapping_dummy_key()
{
    let cfg = config(vec![], vec![rule("vk_none", vec![variable(9, None)])], 1);
    let mut queue = queue(Stage::Output, cfg.clone());
    let receipt = down(cfg, Stage::Complex, variable(1, Some(2)), 125);
    let decision = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    assert!(decision.selection().is_none());
    let plan = decision.plan_main(BatchId(20)).unwrap();
    assert_eq!(
        plan.actions().events()[0].payload.target,
        variable(1, Some(2)).target
    );
    assert_eq!(plan.actions().events().len(), 1);
    let mut saved = queue.enqueue_scoped(&plan).unwrap();
    assert_eq!(
        queue.variable_values().get("v"),
        Some(&VariableValue::Integer(1))
    );
    queue.input_up_scoped(&mut saved, MonoTime(150)).unwrap();
    assert_eq!(
        queue.variable_values().get("v"),
        Some(&VariableValue::Integer(2))
    );
}
#[test]
fn matched_empty_to_stays_consumed_and_never_falls_back_when_environment_changes() {
    let cfg = config(
        vec![],
        vec![
            rule(
                "a",
                vec![Action {
                    conditions: vec![condition(1)],
                    ..key("b").into()
                }],
            ),
            rule("a", vec![key("c").into()]),
        ],
        1,
    );
    let mut queue = queue(Stage::Output, cfg.clone());
    let receipt = down(cfg, Stage::Complex, key("a").into(), 125);
    let decision = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    assert!(decision.selection().is_some());
    assert_eq!(decision.selection().unwrap().index(), 0);
    assert!(
        decision
            .plan_main(BatchId(20))
            .unwrap()
            .actions()
            .events()
            .is_empty()
    );
    set(&mut queue, 1, 1);
    assert!(targets(&decision).is_empty());
    assert_eq!(
        targets(
            &queue
                .decide_producer_down(&receipt, &ContextSnapshot::default())
                .unwrap()
        ),
        [ActionTarget::Key(key("b"))]
    );
}
#[test]
fn invalid_stage_edge_and_receipt_or_destination_clock_reversal_are_rejected_without_state_change()
{
    let cfg = config(vec![], vec![], 1);
    let receipt = down(cfg.clone(), Stage::Complex, key("a").into(), 125);
    let simple = queue(Stage::Simple, cfg.clone());
    assert!(matches!(
        simple.decide_producer_down(&receipt, &ContextSnapshot::default()),
        Err(GeneratedError::Stage)
    ));
    let mut queue = queue(Stage::Output, cfg.clone());
    let wrong_stage = down(cfg, Stage::Simple, key("a").into(), 125);
    assert!(matches!(
        queue.decide_producer_down(&wrong_stage, &ContextSnapshot::default()),
        Err(GeneratedError::Stage)
    ));
    for edge in [KeyEdge::Up, KeyEdge::Repeat] {
        let mut wrong = DueEvent {
            event: receipt.event.clone(),
            delivered_at: receipt.delivered_at,
        };
        wrong.event.payload.edge = edge;
        assert!(matches!(
            queue.decide_producer_down(&wrong, &ContextSnapshot::default()),
            Err(GeneratedError::Lifecycle)
        ));
    }
    let reversed = DueEvent {
        event: receipt.event.clone(),
        delivered_at: MonoTime(99),
    };
    assert!(matches!(
        queue.decide_producer_down(&reversed, &ContextSnapshot::default()),
        Err(GeneratedError::Timeline(TimelineError::TimeReversed))
    ));
    queue.poll(MonoTime(130)).unwrap();
    assert!(matches!(
        queue.decide_producer_down(&receipt, &ContextSnapshot::default()),
        Err(GeneratedError::Timeline(TimelineError::TimeReversed))
    ));
    assert_eq!(queue.pending_batches(), 0);
    assert_eq!(queue.queued_events(), 0);
    assert!(queue.modifiers().is_empty());
}
#[test]
fn future_foreign_same_generation_and_invalid_old_schema_are_not_selectable_producer_snapshots() {
    let cfg = config(vec![], vec![], 1);
    let queue = queue(Stage::Output, cfg);
    for generation in [1, 2] {
        let foreign = config(vec![], vec![], generation);
        assert!(matches!(
            queue.decide_producer_down(
                &down(foreign, Stage::Complex, key("a").into(), 125),
                &ContextSnapshot::default()
            ),
            Err(GeneratedError::Provenance)
        ));
    }
    let mut invalid = rule("a", vec![key("b").into()]);
    // Simple Held is now valid on the staged path; Delayed remains gated.
    invalid.extras.delayed = Some(DelayedAction {
        delay_ns: 50,
        if_invoked: vec![key("c").into()],
        if_canceled: vec![],
    });
    let invalid = config(vec![invalid], vec![], 0);
    assert!(matches!(
        queue.decide_producer_down(
            &down(invalid, Stage::Complex, key("a").into(), 125),
            &ContextSnapshot::default()
        ),
        Err(GeneratedError::Engine(EngineError::InvalidMapping))
    ));
}

#[test]
fn old_simple_held_schema_retains_original_rule_on_staged_producer_selection() {
    let mut mapping = rule("a", vec![key("b").into()]);
    mapping.extras.if_held_down = vec![key("c").into()];
    mapping.extras.held_threshold_ns = 50;
    let old = config(vec![mapping], vec![], 1);
    let queue = queue(Stage::Complex, config(vec![], vec![], 2));
    let decision = queue
        .decide_producer_down(
            &down(old.clone(), Stage::Simple, key("a").into(), 125),
            &ContextSnapshot::default(),
        )
        .unwrap();
    assert_eq!(targets(&decision), [ActionTarget::Key(key("b"))]);
    assert!(Arc::ptr_eq(decision.selection().unwrap().snapshot(), &old));
    assert_eq!(queue.queued_events(), 0);
    assert_eq!(queue.pending_batches(), 0);
}
#[test]
fn saved_decision_is_the_nonclone_actual_producer_payload_of_joint_output_admission_and_original_up()
 {
    let old = config(vec![], vec![rule("a", vec![key("b").into()])], 1);
    let mut output = OutputStageActor::new(old.clone()).unwrap();
    output.observe_device(device(), false, MonoTime(0)).unwrap();
    output
        .activate(config(vec![], vec![rule("a", vec![key("c").into()])], 2))
        .unwrap();
    let receipt = down(old.clone(), Stage::Complex, key("a").into(), 125);
    let decision = output
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(20)).unwrap();
    let mut arena = BindingArena::<ProducerDecision>::default();
    arena
        .prepare(origin(old.clone(), 10, Stage::Complex, 100), 1, None)
        .unwrap()
        .commit();
    let parent = ActionSlot {
        batch: BatchId(10),
        index: 0,
    };
    let registration = arena
        .prepare_child_with_down(
            origin(old.clone(), 20, Stage::Output, 125),
            1,
            receipt,
            decision,
        )
        .unwrap();
    output
        .prepare_child_scoped(plan, registration)
        .unwrap()
        .commit();
    let step = output.poll(MonoTime(125)).unwrap().unwrap();
    assert_eq!(step.outputs[0].key, key("b"));
    assert_eq!(
        targets(arena.live(parent).unwrap()),
        [ActionTarget::Key(key("b"))]
    );
    assert!(matches!(
        arena.take(parent),
        Err(BindingError::ChildPending)
    ));
    output.input_up(BatchId(20), MonoTime(150)).unwrap();
    let step = output.poll(MonoTime(150)).unwrap().unwrap();
    assert_eq!(step.outputs[0].key, key("b"));
    assert_eq!(step.outputs[0].edge, KeyEdge::Up);
    arena.cancel_subtree(BatchId(20)).unwrap();
    let saved = arena.take(parent).unwrap();
    assert!(Arc::ptr_eq(&saved.down().config, &old));
    assert_eq!(saved.delivered_at(), MonoTime(125));
    arena.retire(BatchId(10)).unwrap();
}
#[test]
fn saved_branch_rule_filters_fresh_conditions_without_reselecting_frozen_main_work() {
    let mut mapping = rule("a", vec![key("b").into()]);
    mapping.extras.if_alone = vec![Action {
        conditions: vec![condition(1)],
        ..key("c").into()
    }];
    let cfg = config(vec![], vec![mapping], 1);
    let mut queue = queue(Stage::Output, cfg.clone());
    set(&mut queue, 1, 1);
    let receipt = down(cfg, Stage::Complex, key("a").into(), 125);
    let decision = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    let selected = decision.selection().unwrap();
    let before = selected
        .select_branch(RuleBranch::Alone, false, |condition| {
            queue.condition_truth(
                condition,
                &ContextSnapshot::default(),
                device(),
                MonoTime(125),
            )
        })
        .unwrap();
    assert_eq!(before.actions().len(), 1);
    set(&mut queue, 2, 2);
    let after = selected
        .select_branch(RuleBranch::Alone, false, |condition| {
            queue.condition_truth(
                condition,
                &ContextSnapshot::default(),
                device(),
                MonoTime(150),
            )
        })
        .unwrap();
    assert_eq!(after.actions().len(), 0);
    assert_eq!(targets(&decision), [ActionTarget::Key(key("b"))]);
}
#[test]
fn whole_main_array_is_filtered_before_its_first_generated_variable_changes_destination_view() {
    let cfg = config(
        vec![],
        vec![rule(
            "a",
            vec![
                variable(1, None),
                Action {
                    conditions: vec![condition(1)],
                    ..key("b").into()
                },
            ],
        )],
        1,
    );
    let mut queue = queue(Stage::Output, cfg.clone());
    let receipt = down(cfg, Stage::Complex, key("a").into(), 125);
    let decision = queue
        .decide_producer_down(&receipt, &ContextSnapshot::default())
        .unwrap();
    let plan = decision.plan_main(BatchId(20)).unwrap();
    assert_eq!(targets(&decision), [variable(1, None).target]);
    queue.enqueue_scoped(&plan).unwrap();
    assert_eq!(
        queue.variable_values().get("v"),
        Some(&VariableValue::Integer(1))
    );
    assert_eq!(targets(&decision), [variable(1, None).target]);
    assert_eq!(
        targets(
            &queue
                .decide_producer_down(&receipt, &ContextSnapshot::default())
                .unwrap()
        ),
        [variable(1, None).target, ActionTarget::Key(key("b"))]
    );
}
