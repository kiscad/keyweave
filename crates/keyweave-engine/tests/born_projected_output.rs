#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, modifier_scope::ScopedActionPlan, output_stage::*, rule_selection::*,
    stage_bindings::*, timeline::*, *,
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
fn ordinary_source_is_hidden_from_first_down_and_close_emits_one_saved_down_without_replaying_array()
 {
    let cfg = config(vec![key("x").into()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    assert!(drain(&mut actor, 100).is_empty());
    assert!(actor.pressed_outputs().is_empty());
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    actor.close_key_projection(id, MonoTime(110)).unwrap();
    assert_eq!(drain(&mut actor, 110), vec![(key("x"), KeyEdge::Down)]);
    actor.input_up(BatchId(1), MonoTime(120)).unwrap();
    assert_eq!(drain(&mut actor, 120), vec![(key("x"), KeyEdge::Up)]);
}
#[test]
fn variable_source_never_exposes_original_down_value_while_born_projected() {
    let cfg = config(vec![variable()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    assert!(actor.executed_variables().is_empty());
    assert!(drain(&mut actor, 100).is_empty());
    assert_eq!(
        actor.executed_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    actor.close_key_projection(id, MonoTime(110)).unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    assert!(drain(&mut actor, 110).is_empty());
    assert_eq!(
        actor.executed_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
}
#[test]
fn abort_does_not_consume_batch_or_projection_or_generated_variable_and_same_id_can_retry() {
    let cfg = config(vec![variable()]);
    let mut actor = actor(cfg.clone());
    actor
        .prepare_projected_scoped(scoped(cfg.clone(), 1))
        .unwrap()
        .abort();
    assert_eq!(actor.pending_batches(), 0);
    assert_eq!(actor.queued_events(), 0);
    assert_eq!(actor.reserved_slots(), 0);
    assert!(actor.generated_variables().is_empty());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    assert!(drain(&mut actor, 100).is_empty());
    actor.close_key_projection(id, MonoTime(110)).unwrap();
    drain(&mut actor, 110);
}
#[test]
fn early_close_stays_after_first_hidden_delivery_and_source_is_restored_once() {
    let cfg = config(vec![key("x").into()]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    actor.close_key_projection(id, MonoTime(90)).unwrap();
    assert!(drain(&mut actor, 99).is_empty());
    assert_eq!(drain(&mut actor, 100), vec![(key("x"), KeyEdge::Down)]);
    assert!(!actor.is_failed());
}
#[test]
fn future_tap_array_finishes_while_hidden_and_close_never_generates_unexecuted_or_released_keys() {
    let cfg = config(vec![
        Action {
            repeat: false,
            hold_down_ns: 50,
            ..key("x").into()
        },
        key("y").into(),
    ]);
    let mut actor = actor(cfg.clone());
    let id = actor
        .prepare_projected_scoped(scoped(cfg, 1))
        .unwrap()
        .commit();
    assert!(drain(&mut actor, 100).is_empty());
    assert!(drain(&mut actor, 150).is_empty());
    actor.close_key_projection(id, MonoTime(160)).unwrap();
    assert_eq!(drain(&mut actor, 160), vec![(key("y"), KeyEdge::Down)]);
}
#[test]
fn retained_old_parent_projected_child_registration_aborts_and_commits_with_same_saved_arc() {
    let old = config(vec![key("x").into()]);
    let mut actor = actor(old.clone());
    let mut arena: BindingArena<()> = BindingArena::default();
    let parent = ActionSlot {
        batch: BatchId(5000),
        index: 0,
    };
    arena
        .prepare(
            PlanOrigin {
                batch: parent.batch,
                stage: Stage::Complex,
                start: MonoTime(0),
                device: device(),
                source: SourceToken(1),
                config: old.clone(),
            },
            1,
            None,
        )
        .unwrap()
        .commit();
    arena.insert(parent, ()).unwrap();
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    for abort in [true, false] {
        let plan = scoped(old.clone(), 1);
        let registration = arena
            .prepare(
                PlanOrigin {
                    batch: BatchId(1),
                    stage: Stage::Output,
                    start: MonoTime(100),
                    device: device(),
                    source: SourceToken(1),
                    config: old.clone(),
                },
                1,
                Some(parent),
            )
            .unwrap();
        let guard = actor
            .prepare_projected_child_scoped(plan, registration)
            .unwrap();
        if abort {
            guard.abort();
            assert_eq!(arena.reserved_sources(), 1);
            assert_eq!(actor.pending_batches(), 0);
        } else {
            let id = guard.commit();
            assert_eq!(actor.parent_binding(BatchId(1)), Some(parent));
            assert!(drain(&mut actor, 100).is_empty());
            actor.close_key_projection(id, MonoTime(110)).unwrap();
            assert_eq!(drain(&mut actor, 110), vec![(key("x"), KeyEdge::Down)]);
        }
    }
}
#[test]
fn snapshot_and_attached_modifier_sources_are_hidden_before_any_temporary_owner_exists() {
    let mut cfg = (*config(vec![])).clone();
    cfg.complex[0].extras.if_alone = vec![Action {
        hold_down_ns: 10,
        modifiers: vec![key("left_alt")],
        ..key("x").into()
    }];
    let cfg = Arc::new(cfg);
    let selection = select_rule(
        cfg.clone(),
        MappingStage::Complex,
        key("a"),
        BTreeSet::from([key("left_shift"), key("left_control")]),
        |_| Truth::True,
    )
    .unwrap();
    let branch = selection
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    let mut actor = actor(cfg);
    let plan =
        ScopedActionPlan::branch(&branch, BatchId(1), MonoTime(100), device(), SourceToken(1))
            .unwrap();
    let id = actor.prepare_projected_scoped(plan).unwrap().commit();
    assert!(drain(&mut actor, 100).is_empty());
    assert!(actor.pressed_outputs().is_empty());
    assert!(drain(&mut actor, 110).is_empty());
    actor.close_key_projection(id, MonoTime(120)).unwrap();
    assert!(drain(&mut actor, 120).is_empty());
    assert!(actor.pressed_outputs().is_empty());
}
