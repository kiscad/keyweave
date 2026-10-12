#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, modifier_scope::ScopedActionPlan, output_stage::*, rule_selection::*,
    timeline::*, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
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
fn config(actions: Vec<Action>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "birth".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: actions,
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
    })
}
fn device() -> DeviceInstanceId {
    DeviceInstanceId {
        id: 1,
        generation: 1,
    }
}
fn actor(cfg: Arc<ConfigSnapshot>) -> OutputStageActor {
    let mut actor = OutputStageActor::new(cfg).unwrap();
    actor.observe_device(device(), false, MonoTime(0)).unwrap();
    actor
}
fn scoped(cfg: Arc<ConfigSnapshot>, id: u64) -> ScopedActionPlan {
    let selection = select_rule(
        cfg,
        MappingStage::Complex,
        key("a"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    ScopedActionPlan::main(
        &selection,
        BatchId(id),
        MonoTime(100),
        device(),
        SourceToken(1),
    )
    .unwrap()
}
fn drain(actor: &mut OutputStageActor, at: u64) -> Vec<(KeyId, KeyEdge)> {
    let mut outputs = vec![];
    for _ in 0..10000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return outputs;
        };
        outputs.extend(step.outputs.into_iter().map(|o| (o.key, o.edge)));
    }
    panic!("fixture did not drain");
}
#[test]
fn active_original_source_restores_once_without_reselection_and_saved_up_still_releases() {
    let cfg = config(vec![key("x").into()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    assert!(drain(&mut actor, 100).is_empty());
    assert_eq!(
        actor
            .restore_key_projection_control(id, MonoTime(110))
            .unwrap()
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("x"), KeyEdge::Down)]
    );
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("x")));
    assert!(
        actor
            .restore_key_projection_control(id, MonoTime(110))
            .is_err()
    );
    assert!(!actor.is_failed());
    actor.input_up(BatchId(1), MonoTime(120)).unwrap();
    assert_eq!(drain(&mut actor, 120), [(key("x"), KeyEdge::Up)]);
}
#[test]
fn control_before_birth_clears_pending_mask_and_future_original_down_is_visible() {
    let cfg = config(vec![key("x").into()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    assert!(
        actor
            .restore_key_projection_control(id, MonoTime(90))
            .unwrap()
            .is_empty()
    );
    assert!(drain(&mut actor, 90).is_empty());
    assert_eq!(drain(&mut actor, 100), [(key("x"), KeyEdge::Down)]);
}
#[test]
fn active_other_projection_keeps_source_hidden_until_its_own_final_close() {
    let cfg = config(vec![key("x").into()]);
    let mut actor = actor(cfg.clone());
    let first = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    drain(&mut actor, 100);
    let second = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    assert!(drain(&mut actor, 110).is_empty());
    assert!(
        actor
            .restore_key_projection_control(first, MonoTime(120))
            .unwrap()
            .is_empty()
    );
    assert!(actor.pressed_outputs().is_empty());
    actor.close_key_projection(second, MonoTime(130)).unwrap();
    assert_eq!(drain(&mut actor, 130), [(key("x"), KeyEdge::Down)]);
}
#[test]
fn active_variable_restores_frozen_down_on_both_sides_and_keeps_original_up() {
    let cfg = config(vec![variable()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    drain(&mut actor, 100);
    assert_eq!(
        actor.executed_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    assert!(
        actor
            .restore_key_projection_control(id, MonoTime(110))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    assert_eq!(
        actor.executed_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    actor.input_up(BatchId(1), MonoTime(120)).unwrap();
    drain(&mut actor, 120);
    assert_eq!(
        actor.executed_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
}
fn origin(cfg: Arc<ConfigSnapshot>, id: u64, at: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(id),
        stage: Stage::Output,
        start: MonoTime(at),
        device: device(),
        source: SourceToken(id),
        config: cfg,
    }
}
#[test]
fn pending_close_is_discarded_and_already_restored_generation_does_not_overwrite_later_variable_assignment()
 {
    let cfg = config(vec![variable()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg.clone(), 1))
        .unwrap()
        .commit();
    drain(&mut actor, 100);
    actor.close_key_projection(id, MonoTime(1000)).unwrap();
    let mut other = variable();
    other.target = ActionTarget::SetVariable(VariableAction {
        name: "v".into(),
        operation: VariableOperation::Set {
            value: Some(VariableValue::Integer(3)),
            key_up_value: Some(VariableValue::Integer(4)),
        },
    });
    actor
        .enqueue(plan_actions(&[other], origin(cfg, 2, 1000), false).unwrap())
        .unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(3))
    );
    assert!(
        actor
            .restore_key_projection_control(id, MonoTime(1000))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(3))
    );
    assert_eq!(
        actor.executed_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    drain(&mut actor, 1000);
    assert_eq!(
        actor.executed_variables().get("v"),
        Some(&VariableValue::Integer(3))
    );
}
#[test]
fn finished_sources_are_not_revived_by_control_after_recipe_retirement() {
    let cfg = config(vec![Action {
        repeat: false,
        hold_down_ns: 50,
        ..key("x").into()
    }]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    assert!(drain(&mut actor, 150).is_empty());
    assert_eq!(actor.pending_batches(), 0);
    assert!(
        actor
            .restore_key_projection_control(id, MonoTime(160))
            .unwrap()
            .is_empty()
    );
    assert!(actor.pressed_outputs().is_empty());
}
#[test]
fn full_action_fifo_cannot_block_lifecycle_restoration_and_future_actions_are_not_delivered() {
    let cfg = config(vec![key("x").into()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg.clone(), 1))
        .unwrap()
        .commit();
    drain(&mut actor, 100);
    let bulk = vec![
        Action {
            hold_down_ns: 1,
            ..key("y").into()
        };
        512
    ];
    for batch in 100..108 {
        actor
            .enqueue(plan_actions(&bulk, origin(cfg.clone(), batch, 200), true).unwrap())
            .unwrap();
    }
    let deadline = actor.next_deadline();
    let outputs = actor
        .restore_key_projection_control(id, MonoTime(200))
        .unwrap();
    assert_eq!(
        outputs.iter().map(|o| (o.key, o.edge)).collect::<Vec<_>>(),
        [(key("x"), KeyEdge::Down)]
    );
    assert_eq!(actor.next_deadline(), deadline);
    assert_eq!(actor.pressed_outputs(), vec![key("x")]);
}
#[test]
fn control_precedes_due_original_up_without_discarding_or_retiming_that_up() {
    let cfg = config(vec![key("x").into()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    drain(&mut actor, 100);
    actor.input_up(BatchId(1), MonoTime(110)).unwrap();
    let outputs = actor
        .restore_key_projection_control(id, MonoTime(110))
        .unwrap();
    assert_eq!(
        outputs.iter().map(|o| (o.key, o.edge)).collect::<Vec<_>>(),
        [(key("x"), KeyEdge::Down)]
    );
    assert_eq!(drain(&mut actor, 110), [(key("x"), KeyEdge::Up)]);
}
#[test]
fn standard_modifier_restoration_stays_lazy_until_a_real_nonmodifier_down() {
    let cfg = config(vec![key("left_shift").into()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg.clone(), 1))
        .unwrap()
        .commit();
    assert!(drain(&mut actor, 100).is_empty());
    assert!(
        actor
            .restore_key_projection_control(id, MonoTime(110))
            .unwrap()
            .is_empty()
    );
    assert!(actor.pressed_outputs().is_empty());
    actor
        .enqueue(plan_actions(&[key("x").into()], origin(cfg, 2, 120), false).unwrap())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 120),
        [
            (key("left_shift"), KeyEdge::Down),
            (key("x"), KeyEdge::Down)
        ]
    );
}
