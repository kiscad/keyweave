#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::*, modifier_scope::ScopedActionPlan, rule_selection::*,
    timeline::Stage, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
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
        ..key("b").into()
    }
}
fn cfg(tapped: bool) -> Arc<ConfigSnapshot> {
    let mut extras = MappingExtras::default();
    extras.from_modifiers.mandatory.insert(Modifier::Shift);
    extras.if_alone = vec![variable()];
    extras.if_held_down = vec![key("right_control").into()];
    let main = Action {
        repeat: !tapped,
        hold_down_ns: 50,
        ..key("right_control").into()
    };
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "generated-scopes".into(),
        simple: vec![],
        complex: vec![
            Mapping {
                id: "a".into(),
                from: key("a"),
                to: vec![main],
                allow_extra_modifiers: true,
                extras,
            },
            Mapping {
                id: "x".into(),
                from: key("x"),
                to: vec![key("right_control").into()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            },
        ],
    })
}
fn queue(config: Arc<ConfigSnapshot>) -> GeneratedStageQueue {
    let mut queue = GeneratedStageQueue::new(Stage::Output, config).unwrap();
    queue.observe_device(device(1), false).unwrap();
    queue.observe_device(device(2), false).unwrap();
    queue
}
fn raw(queue: &mut GeneratedStageQueue, id: u64, source_device: u64, target: &str) {
    let plan = plan_actions(
        &[key(target).into()],
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(0),
            device: device(source_device),
            source: SourceToken(100),
            config: queue.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    queue.enqueue(plan.events().to_vec()).unwrap();
}
fn select(queue: &GeneratedStageQueue) -> RuleSelection {
    queue
        .select(
            key("a"),
            &ContextSnapshot::default(),
            device(1),
            MonoTime(0),
        )
        .unwrap()
        .unwrap()
}
fn main(rule: &RuleSelection, id: u64) -> ScopedActionPlan {
    ScopedActionPlan::main(
        rule,
        BatchId(id),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap()
}
fn modifiers(queue: &GeneratedStageQueue) -> BTreeSet<KeyId> {
    queue.modifiers()
}
#[test]
fn main_consumed_visibility_restores_when_up_is_generated_before_timed_delivery() {
    let mut queue = queue(cfg(false));
    raw(&mut queue, 1, 2, "left_shift");
    let rule = select(&queue);
    let planned = main(&rule, 2);
    let mut saved = queue.enqueue_scoped(&planned).unwrap();
    assert_eq!(modifiers(&queue), BTreeSet::from([key("right_control")]));
    assert!(
        queue
            .select(
                key("a"),
                &ContextSnapshot::default(),
                device(1),
                MonoTime(0)
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(queue.hidden_modifier_references(), 1);
    assert!(queue.input_up_scoped(&mut saved, MonoTime(10)).unwrap());
    assert_eq!(modifiers(&queue), BTreeSet::from([key("left_shift")]));
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert_eq!(queue.next_deadline(), Some(MonoTime(0)));
    assert!(!queue.input_up_scoped(&mut saved, MonoTime(10)).unwrap());
    while queue.poll(MonoTime(100)).unwrap().is_some() {}
    assert_eq!(modifiers(&queue), BTreeSet::from([key("left_shift")]));
}
#[test]
fn overlapping_scopes_keep_independent_hidden_references_in_either_up_order() {
    for reverse in [false, true] {
        let mut queue = queue(cfg(false));
        raw(&mut queue, 1, 2, "left_shift");
        let rule = select(&queue);
        let mut first = queue.enqueue_scoped(&main(&rule, 2)).unwrap();
        let mut second = queue.enqueue_scoped(&main(&rule, 3)).unwrap();
        assert_eq!(queue.hidden_modifier_references(), 2);
        let (early, late) = if reverse {
            (&mut second, &mut first)
        } else {
            (&mut first, &mut second)
        };
        queue.input_up_scoped(early, MonoTime(10)).unwrap();
        assert_eq!(queue.hidden_modifier_references(), 1);
        assert_eq!(modifiers(&queue), BTreeSet::from([key("right_control")]));
        queue.input_up_scoped(late, MonoTime(11)).unwrap();
        assert_eq!(queue.hidden_modifier_references(), 0);
        assert_eq!(modifiers(&queue), BTreeSet::from([key("left_shift")]));
    }
}
#[test]
fn snapshot_tap_scope_restores_view_after_generation_and_retains_variable_up_effect() {
    let mut queue = queue(cfg(false));
    raw(&mut queue, 1, 2, "left_shift");
    raw(&mut queue, 2, 2, "left_alt");
    let rule = select_rule(
        queue.snapshot().clone(),
        MappingStage::Complex,
        key("a"),
        BTreeSet::from([key("left_shift"), key("left_control")]),
        |_| Truth::True,
    )
    .unwrap();
    let branch = rule
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    let planned = ScopedActionPlan::branch(
        &branch,
        BatchId(3),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap();
    let mut saved = queue.enqueue_scoped(&planned).unwrap();
    assert_eq!(
        modifiers(&queue),
        BTreeSet::from([key("left_shift"), key("left_alt")])
    );
    assert_eq!(queue.variable_values()["v"], VariableValue::Integer(2));
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert!(!queue.input_up_scoped(&mut saved, MonoTime(10)).unwrap());
}
#[test]
fn scoped_timeline_overflow_does_not_commit_variable_or_hidden_state_and_same_id_can_retry() {
    let mut config = (*cfg(true)).clone();
    config.complex[0].to.insert(0, variable());
    let mut queue = queue(Arc::new(config));
    raw(&mut queue, 1, 2, "left_shift");
    queue.poll(MonoTime(u64::MAX - 10)).unwrap();
    let rule = select(&queue);
    let planned =
        ScopedActionPlan::main(&rule, BatchId(2), MonoTime(0), device(1), SourceToken(100))
            .unwrap();
    assert!(matches!(
        queue.enqueue_scoped(&planned),
        Err(GeneratedError::Timeline(_))
    ));
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert!(queue.variable_values().is_empty());
    assert_eq!(queue.logical_slots(), 1);
    assert_eq!(queue.queued_events(), 0);
    let retry = queue
        .select(
            key("x"),
            &ContextSnapshot::default(),
            device(1),
            MonoTime(u64::MAX - 10),
        )
        .unwrap()
        .unwrap();
    queue
        .enqueue_scoped(
            &ScopedActionPlan::main(&retry, BatchId(2), MonoTime(0), device(1), SourceToken(100))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(queue.logical_slots(), 2);
}
#[test]
fn device_cleanup_and_reset_restore_scopes_and_invalidate_original_up_credentials() {
    let mut queue = queue(cfg(false));
    raw(&mut queue, 1, 2, "left_shift");
    let rule = select(&queue);
    let mut saved = queue.enqueue_scoped(&main(&rule, 2)).unwrap();
    queue.observe_device(device(1), true).unwrap();
    assert_eq!(modifiers(&queue), BTreeSet::from([key("left_shift")]));
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert_eq!(
        queue.input_up_scoped(&mut saved, MonoTime(10)),
        Err(GeneratedError::Lifecycle)
    );
    queue.reset(MonoTime(0)).unwrap();
    assert_eq!(
        queue.input_up_scoped(&mut saved, MonoTime(10)),
        Err(GeneratedError::Provenance)
    );
}
#[test]
fn tapped_last_modifier_has_no_logical_down_but_scope_waits_for_original_up_recipe() {
    let mut queue = queue(cfg(true));
    raw(&mut queue, 1, 2, "left_shift");
    let rule = select(&queue);
    let planned = main(&rule, 2);
    assert!(planned.actions().releases().is_empty());
    let mut saved = queue.enqueue_scoped(&planned).unwrap();
    assert!(modifiers(&queue).is_empty());
    assert_eq!(queue.hidden_modifier_references(), 1);
    assert_eq!(queue.logical_slots(), 1);
    let count = queue.queued_events();
    queue.input_up_scoped(&mut saved, MonoTime(20)).unwrap();
    assert_eq!(queue.queued_events(), count);
    assert_eq!(modifiers(&queue), BTreeSet::from([key("left_shift")]));
    assert_eq!(queue.hidden_modifier_references(), 0);
    while queue.poll(MonoTime(100)).unwrap().is_some() {}
    assert_eq!(modifiers(&queue), BTreeSet::from([key("left_shift")]));
}
#[test]
fn exact_logical_reference_budget_rejection_changes_no_queue_or_state_then_can_retry() {
    let mut queue = queue(cfg(false));
    for id in 1..=128 {
        raw(&mut queue, id, 2, "left_shift");
    }
    let rule = select(&queue);
    let mut saved = vec![];
    for id in 129..=256 {
        saved.push(queue.enqueue_scoped(&main(&rule, id)).unwrap());
    }
    assert_eq!(
        queue.hidden_modifier_references(),
        MAX_PERSISTENT_SUPPRESSION_REFERENCES
    );
    let count = queue.queued_events();
    let denied = main(&rule, 257);
    assert!(matches!(
        queue.enqueue_scoped(&denied),
        Err(GeneratedError::Engine(EngineError::ResourceLimit))
    ));
    assert_eq!(queue.queued_events(), count);
    assert_eq!(queue.logical_slots(), 256);
    queue.input_up_scoped(&mut saved[0], MonoTime(10)).unwrap();
    queue.enqueue_scoped(&denied).unwrap();
    assert_eq!(
        queue.hidden_modifier_references(),
        MAX_PERSISTENT_SUPPRESSION_REFERENCES
    );
    assert_eq!(queue.logical_slots(), 256);
}
#[test]
fn credentials_are_queue_bound_and_empty_scope_still_checks_stage_and_snapshot() {
    let mut queue = queue(cfg(false));
    raw(&mut queue, 1, 2, "left_shift");
    let rule = select(&queue);
    let planned = main(&rule, 2);
    let mut saved = queue.enqueue_scoped(&planned).unwrap();
    let mut other = GeneratedStageQueue::new(Stage::Output, queue.snapshot().clone()).unwrap();
    assert_eq!(
        other.input_up_scoped(&mut saved, MonoTime(10)),
        Err(GeneratedError::Provenance)
    );
    assert!(matches!(
        queue.enqueue_scoped(&planned),
        Err(GeneratedError::Lifecycle)
    ));
    queue.cancel_batch(BatchId(2));
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert_eq!(
        queue.input_up_scoped(&mut saved, MonoTime(10)),
        Err(GeneratedError::Lifecycle)
    );
    let empty = rule
        .select_branch(RuleBranch::Held, true, |_| panic!("halted"))
        .unwrap();
    let empty =
        ScopedActionPlan::branch(&empty, BatchId(3), MonoTime(0), device(1), SourceToken(100))
            .unwrap();
    let mut wrong_stage =
        GeneratedStageQueue::new(Stage::Complex, queue.snapshot().clone()).unwrap();
    assert!(matches!(
        wrong_stage.enqueue_scoped(&empty),
        Err(GeneratedError::Stage)
    ));
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert!(matches!(
        queue.enqueue_scoped(&empty),
        Err(GeneratedError::Provenance)
    ));
}
#[test]
fn saved_old_arc_up_rejection_preserves_scope_and_clock_until_capacity_is_available() {
    let mut queue = queue(cfg(false));
    raw(&mut queue, 1, 2, "left_shift");
    let rule = select(&queue);
    let mut saved = queue.enqueue_scoped(&main(&rule, 2)).unwrap();
    for id in 1000..5094 {
        let actions = if id == 1000 {
            vec![key("b").into(), key("b").into()]
        } else {
            vec![key("b").into()]
        };
        let plan = plan_actions(
            &actions,
            PlanOrigin {
                batch: BatchId(id),
                stage: Stage::Output,
                start: MonoTime(100),
                device: device(2),
                source: SourceToken(100),
                config: queue.snapshot().clone(),
            },
            true,
        )
        .unwrap();
        queue.enqueue(plan.events().to_vec()).unwrap();
    }
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert_eq!(
        queue.queued_events(),
        keyweave_engine::timeline::MAX_STAGE_EVENTS
    );
    assert_eq!(
        queue.input_up_scoped(&mut saved, MonoTime(10)),
        Err(GeneratedError::Timeline(
            keyweave_engine::timeline::TimelineError::Capacity
        ))
    );
    assert_eq!(queue.hidden_modifier_references(), 1);
    assert_eq!(modifiers(&queue), BTreeSet::from([key("right_control")]));
    assert_eq!(queue.logical_slots(), 2);
    assert!(queue.poll(MonoTime(0)).unwrap().is_some());
    assert!(queue.input_up_scoped(&mut saved, MonoTime(10)).unwrap());
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert_eq!(modifiers(&queue), BTreeSet::from([key("left_shift")]));
    assert_eq!(
        queue.input_up_scoped(&mut saved, MonoTime(9)),
        Err(GeneratedError::Timeline(
            keyweave_engine::timeline::TimelineError::TimeReversed
        ))
    );
}
#[test]
fn a_released_owner_slot_cannot_be_rebound_while_a_scope_still_references_it() {
    let mut queue = queue(cfg(false));
    let original = plan_actions(
        &[key("left_shift").into()],
        PlanOrigin {
            batch: BatchId(1),
            stage: Stage::Output,
            start: MonoTime(0),
            device: device(2),
            source: SourceToken(100),
            config: queue.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    queue.enqueue(original.events().to_vec()).unwrap();
    let rule = select(&queue);
    let mut saved = queue.enqueue_scoped(&main(&rule, 2)).unwrap();
    queue
        .enqueue(original.input_up_events(MonoTime(10)))
        .unwrap();
    assert_eq!(queue.hidden_modifier_references(), 1);
    assert_eq!(queue.logical_slots(), 1);
    assert_eq!(
        queue.enqueue(original.events().to_vec()).unwrap_err().error,
        GeneratedError::Lifecycle
    );
    assert_eq!(modifiers(&queue), BTreeSet::from([key("right_control")]));
    queue.input_up_scoped(&mut saved, MonoTime(11)).unwrap();
    assert_eq!(queue.hidden_modifier_references(), 0);
    assert_eq!(queue.logical_slots(), 0);
}
