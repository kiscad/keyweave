#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, modifier_scope::ScopedActionPlan, rule_selection::*, terminal_actor::*,
    timeline::Stage, *,
};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn config(consumed: bool, main: &str) -> Arc<ConfigSnapshot> {
    let mut extras = MappingExtras::default();
    if consumed {
        extras.from_modifiers.mandatory.insert(Modifier::Shift);
    }
    extras.if_alone = vec![Action {
        hold_down_ns: 50,
        ..key("b").into()
    }];
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "scoped".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key(main).into()],
            allow_extra_modifiers: true,
            extras,
        }],
    })
}
fn actor(config: Arc<ConfigSnapshot>) -> TerminalActor {
    let mut actor = TerminalActor::new(config).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor.observe_device(device(2), false, MonoTime(0)).unwrap();
    actor
}
fn select(actor: &TerminalActor, modifiers: &[&str]) -> RuleSelection {
    select_rule(
        actor.snapshot().clone(),
        MappingStage::Complex,
        key("a"),
        modifiers.iter().map(|name| key(name)).collect(),
        |_| Truth::True,
    )
    .unwrap()
}
fn plain(
    actor: &TerminalActor,
    id: u64,
    start: u64,
    source_device: u64,
    target: &str,
) -> ActionPlan {
    plan_actions(
        &[key(target).into()],
        PlanOrigin {
            batch: BatchId(id),
            start: MonoTime(start),
            device: device(source_device),
            source: SourceToken(id + 1000),
            config: actor.snapshot().clone(),
            stage: Stage::Output,
        },
        false,
    )
    .unwrap()
}
fn branch(rule: &RuleSelection, id: u64, start: u64) -> ScopedActionPlan {
    let branch = rule
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    ScopedActionPlan::branch(
        &branch,
        BatchId(id),
        MonoTime(start),
        device(1),
        SourceToken(500),
    )
    .unwrap()
}
fn main(rule: &RuleSelection, id: u64, start: u64) -> ScopedActionPlan {
    ScopedActionPlan::main(
        rule,
        BatchId(id),
        MonoTime(start),
        device(1),
        SourceToken(500),
    )
    .unwrap()
}
fn drain(actor: &mut TerminalActor, at: u64) -> Vec<(String, KeyEdge)> {
    let mut outputs = vec![];
    while let Some(step) = actor.poll(MonoTime(at)).unwrap() {
        outputs.extend(
            step.outputs
                .into_iter()
                .map(|output| (output.key.name(), output.edge)),
        );
    }
    outputs
}
fn summary(outputs: Vec<OutputIntent>) -> Vec<(String, KeyEdge)> {
    outputs
        .into_iter()
        .map(|output| (output.key.name(), output.edge))
        .collect()
}
#[test]
fn snapshot_scope_opens_on_execution_and_restores_original_modifiers_after_duration() {
    let mut actor = actor(config(false, "c"));
    let rule = select(&actor, &["left_control"]);
    actor.enqueue(plain(&actor, 1, 0, 2, "left_alt")).unwrap();
    assert_eq!(drain(&mut actor, 0), [("left_alt".into(), KeyEdge::Down)]);
    actor.enqueue_scoped(branch(&rule, 2, 10)).unwrap();
    assert_eq!(actor.reserved_slots(), 3);
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert!(drain(&mut actor, 9).is_empty());
    assert_eq!(
        drain(&mut actor, 10),
        [
            ("left_control".into(), KeyEdge::Down),
            ("left_alt".into(), KeyEdge::Up),
            ("b".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.hidden_modifier_references(), 1);
    assert_eq!(
        drain(&mut actor, 60),
        [
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up),
            ("left_alt".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert_eq!(actor.reserved_slots(), 1);
    assert_eq!(actor.pressed_outputs(), [key("left_alt")]);
}
#[test]
fn snapshot_owns_its_modifiers_when_the_original_owner_is_released_mid_branch() {
    let mut actor = actor(config(false, "c"));
    let rule = select(&actor, &["left_control"]);
    actor
        .enqueue(plain(&actor, 1, 0, 2, "left_control"))
        .unwrap();
    drain(&mut actor, 0);
    actor.enqueue_scoped(branch(&rule, 2, 10)).unwrap();
    assert_eq!(drain(&mut actor, 10), [("b".into(), KeyEdge::Down)]);
    assert!(
        actor
            .cancel_batch(BatchId(1), MonoTime(11))
            .unwrap()
            .is_empty()
    );
    assert!(actor.pressed_outputs().contains(&key("left_control")));
    assert_eq!(
        drain(&mut actor, 60),
        [
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_slots(), 0);
}
#[test]
fn overlapping_consumed_scopes_release_their_own_reference_in_either_order() {
    for first in [2, 3] {
        let mut actor = actor(config(true, "right_control"));
        let rule = select(&actor, &["left_shift"]);
        actor.enqueue(plain(&actor, 1, 0, 2, "left_shift")).unwrap();
        drain(&mut actor, 0);
        actor.enqueue_scoped(main(&rule, 2, 10)).unwrap();
        assert_eq!(
            drain(&mut actor, 10),
            [
                ("left_shift".into(), KeyEdge::Up),
                ("right_control".into(), KeyEdge::Down)
            ]
        );
        actor.enqueue_scoped(main(&rule, 3, 11)).unwrap();
        assert!(drain(&mut actor, 11).is_empty());
        assert_eq!(actor.hidden_modifier_references(), 2);
        assert!(
            actor
                .cancel_batch(BatchId(first), MonoTime(12))
                .unwrap()
                .is_empty()
        );
        assert_eq!(actor.hidden_modifier_references(), 1);
        assert_eq!(
            summary(
                actor
                    .cancel_batch(BatchId(5 - first), MonoTime(13))
                    .unwrap()
            ),
            [
                ("right_control".into(), KeyEdge::Up),
                ("left_shift".into(), KeyEdge::Down)
            ]
        );
        assert_eq!(actor.hidden_modifier_references(), 0);
    }
}
#[test]
fn pending_cancel_never_opens_scope_and_reset_cleans_actual_temporary_owners() {
    let mut actor = actor(config(false, "c"));
    let rule = select(&actor, &["left_control"]);
    actor.enqueue_scoped(branch(&rule, 1, 100)).unwrap();
    assert_eq!(actor.reserved_slots(), 2);
    assert!(
        actor
            .cancel_batch(BatchId(1), MonoTime(10))
            .unwrap()
            .is_empty()
    );
    assert_eq!(actor.reserved_slots(), 0);
    actor.enqueue_scoped(branch(&rule, 2, 20)).unwrap();
    drain(&mut actor, 20);
    assert_eq!(
        summary(actor.reset(MonoTime(21)).unwrap()),
        [
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_slots(), 0);
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert!(drain(&mut actor, 100).is_empty());
}
#[test]
fn removing_scoped_device_cancels_future_up_and_restores_other_device_owner() {
    let mut actor = actor(config(false, "c"));
    let rule = select(&actor, &["left_control"]);
    actor.enqueue(plain(&actor, 1, 0, 2, "left_alt")).unwrap();
    drain(&mut actor, 0);
    actor.enqueue_scoped(branch(&rule, 2, 10)).unwrap();
    drain(&mut actor, 10);
    assert_eq!(
        summary(actor.observe_device(device(1), true, MonoTime(20)).unwrap()),
        [
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up),
            ("left_alt".into(), KeyEdge::Down)
        ]
    );
    assert!(drain(&mut actor, 60).is_empty());
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert_eq!(actor.live_slots(), 1);
}
#[test]
fn scope_metadata_is_validated_and_timeline_rejection_keeps_admission_atomic() {
    let mut actor = actor(config(false, "c"));
    let invalid = select(&actor, &["b"]);
    let branch_selection = invalid
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    assert!(matches!(
        ScopedActionPlan::branch(
            &branch_selection,
            BatchId(1),
            MonoTime(0),
            device(1),
            SourceToken(1)
        ),
        Err(PlanError::InvalidScope)
    ));
    let rule = select(&actor, &["left_control"]);
    actor.poll(MonoTime(u64::MAX - 10)).unwrap();
    assert!(matches!(
        actor.enqueue_scoped(branch(&rule, 1, 0)),
        Err(ActorError::Timeline(_))
    ));
    assert_eq!(actor.reserved_slots(), 0);
    assert_eq!(actor.queued_events(), 0);
    actor.enqueue_scoped(main(&rule, 1, 0)).unwrap();
    assert_eq!(
        drain(&mut actor, u64::MAX - 10),
        [("c".into(), KeyEdge::Down)]
    );
}
#[test]
fn exact_suppression_budget_failure_changes_no_owners_and_latches_until_reset() {
    let mut actor = actor(config(true, "right_control"));
    let rule = select(&actor, &["left_shift"]);
    for id in 1..=128 {
        actor
            .enqueue(plain(&actor, id, 0, 2, "left_shift"))
            .unwrap();
    }
    drain(&mut actor, 0);
    for id in 129..=257 {
        actor.enqueue_scoped(main(&rule, id, 0)).unwrap();
    }
    for _ in 0..128 {
        actor.poll(MonoTime(0)).unwrap().unwrap();
    }
    assert_eq!(
        actor.hidden_modifier_references(),
        MAX_PERSISTENT_SUPPRESSION_REFERENCES
    );
    assert_eq!(actor.live_slots(), 256);
    assert_eq!(
        actor.poll(MonoTime(0)).unwrap_err(),
        ActorError::Engine(EngineError::ResourceLimit)
    );
    assert_eq!(actor.live_slots(), 256);
    assert_eq!(
        actor.hidden_modifier_references(),
        MAX_PERSISTENT_SUPPRESSION_REFERENCES
    );
    assert_eq!(actor.poll(MonoTime(0)).unwrap_err(), ActorError::Failed);
    assert_eq!(
        actor.input_up(BatchId(129), MonoTime(0)),
        Err(ActorError::Failed)
    );
    assert_eq!(
        actor.enqueue(plain(&actor, 258, 0, 2, "d")),
        Err(ActorError::Failed)
    );
    assert_eq!(
        summary(actor.reset(MonoTime(0)).unwrap()),
        [("right_control".into(), KeyEdge::Up)]
    );
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert_eq!(actor.reserved_slots(), 0);
    actor.enqueue(plain(&actor, 258, 1, 2, "d")).unwrap();
    assert_eq!(drain(&mut actor, 1), [("d".into(), KeyEdge::Down)]);
}
#[test]
fn tapped_last_modifier_keeps_consumed_scope_until_original_up_even_without_saved_key_up() {
    let mut config = (*config(true, "right_control")).clone();
    config.complex[0].to[0].repeat = false;
    config.complex[0].to[0].hold_down_ns = 50;
    let mut actor = actor(Arc::new(config));
    let rule = select(&actor, &["left_shift"]);
    actor.enqueue(plain(&actor, 1, 0, 2, "left_shift")).unwrap();
    drain(&mut actor, 0);
    let planned = main(&rule, 2, 10);
    assert!(planned.actions().releases().is_empty());
    actor.enqueue_scoped(planned).unwrap();
    drain(&mut actor, 10);
    assert_eq!(
        drain(&mut actor, 60),
        [("right_control".into(), KeyEdge::Up)]
    );
    assert_eq!(actor.hidden_modifier_references(), 1);
    assert_eq!(actor.pending_batches(), 2);
    assert!(actor.input_up(BatchId(2), MonoTime(70)).unwrap());
    assert_eq!(
        actor.input_up(BatchId(2), MonoTime(70)),
        Err(ActorError::Engine(EngineError::DeadlinePending))
    );
    assert_eq!(
        drain(&mut actor, 70),
        [("left_shift".into(), KeyEdge::Down)]
    );
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert_eq!(actor.pending_batches(), 1);
    assert!(!actor.input_up(BatchId(2), MonoTime(70)).unwrap());
}
#[test]
fn already_queued_scoped_work_keeps_its_old_snapshot_across_activation() {
    let mut actor = actor(config(false, "c"));
    let rule = select(&actor, &["left_control"]);
    actor.enqueue_scoped(branch(&rule, 1, 10)).unwrap();
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert_eq!(
        actor.enqueue_scoped(branch(&rule, 2, 10)),
        Err(ActorError::InvalidSnapshot)
    );
    assert_eq!(
        drain(&mut actor, 10),
        [
            ("left_control".into(), KeyEdge::Down),
            ("b".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(
        drain(&mut actor, 60),
        [
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_slots(), 0);
}
#[test]
fn early_original_up_waits_behind_tapped_modifier_duration_before_scope_restoration() {
    let mut config = (*config(true, "right_control")).clone();
    config.complex[0].to[0].repeat = false;
    config.complex[0].to[0].hold_down_ns = 50;
    let mut actor = actor(Arc::new(config));
    let rule = select(&actor, &["left_shift"]);
    actor.enqueue(plain(&actor, 1, 0, 2, "left_shift")).unwrap();
    drain(&mut actor, 0);
    actor.enqueue_scoped(main(&rule, 2, 10)).unwrap();
    drain(&mut actor, 10);
    assert!(actor.input_up(BatchId(2), MonoTime(20)).unwrap());
    assert!(drain(&mut actor, 59).is_empty());
    assert_eq!(actor.hidden_modifier_references(), 1);
    assert_eq!(
        drain(&mut actor, 60),
        [
            ("right_control".into(), KeyEdge::Up),
            ("left_shift".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert_eq!(actor.pending_batches(), 1);
}
#[test]
fn filtered_empty_branch_never_opens_a_scope_or_leaves_a_phantom_input_up() {
    let mut actor = actor(config(false, "c"));
    let rule = select(&actor, &["left_control"]);
    let empty = rule
        .select_branch(RuleBranch::Alone, true, |_| panic!("halted"))
        .unwrap();
    actor
        .enqueue_scoped(
            ScopedActionPlan::branch(
                &empty,
                BatchId(1),
                MonoTime(10),
                device(1),
                SourceToken(500),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(actor.reserved_slots(), 0);
    assert_eq!(actor.pending_batches(), 0);
    assert!(drain(&mut actor, 10).is_empty());
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert!(!actor.input_up(BatchId(1), MonoTime(10)).unwrap());
}
#[test]
fn last_nonmodifier_restores_logical_consumed_state_after_main_work_before_original_up() {
    let mut actor = actor(config(true, "c"));
    let rule = select(&actor, &["left_shift"]);
    actor.enqueue(plain(&actor, 1, 0, 2, "left_shift")).unwrap();
    drain(&mut actor, 0);
    actor.enqueue_scoped(main(&rule, 2, 10)).unwrap();
    assert_eq!(
        drain(&mut actor, 10),
        [
            ("left_shift".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.hidden_modifier_references(), 0);
    assert_eq!(actor.pending_batches(), 2);
    actor.enqueue(plain(&actor, 3, 11, 2, "d")).unwrap();
    assert_eq!(
        drain(&mut actor, 11),
        [
            ("left_shift".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(
        summary(actor.cancel_batch(BatchId(2), MonoTime(12)).unwrap()),
        [("c".into(), KeyEdge::Up)]
    );
}
