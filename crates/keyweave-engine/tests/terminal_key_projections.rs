#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, terminal_actor::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn actor() -> TerminalActor {
    let mut actor =
        TerminalActor::new(Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor.observe_device(device(2), false, MonoTime(0)).unwrap();
    actor
}
fn plan(
    actor: &TerminalActor,
    id: u64,
    dev: u64,
    at: u64,
    actions: &[Action],
    tap: bool,
) -> ActionPlan {
    plan_actions(
        actions,
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(at),
            device: device(dev),
            source: SourceToken(id),
            config: actor.snapshot().clone(),
        },
        tap,
    )
    .unwrap()
}
fn drain(actor: &mut TerminalActor, at: u64) -> Vec<(KeyId, KeyEdge)> {
    let mut outputs = vec![];
    for _ in 0..10000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return outputs;
        };
        outputs.extend(
            step.outputs
                .into_iter()
                .map(|output| (output.key, output.edge)),
        );
    }
    panic!("fixture did not drain");
}
#[test]
fn saved_key_is_hidden_and_restored_without_config_reselection_and_nested_scope_only_last_restores()
{
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 1, 100, &[key("x").into()], false))
        .unwrap();
    assert_eq!(drain(&mut actor, 100), vec![(key("x"), KeyEdge::Down)]);
    let first = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    let second = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    assert_eq!(actor.suppression_references(), 2);
    assert!(!actor.projection_is_active(first));
    assert_eq!(drain(&mut actor, 110), vec![(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor.close_key_projection(first, MonoTime(120)).unwrap();
    assert!(drain(&mut actor, 120).is_empty());
    assert_eq!(actor.suppression_references(), 1);
    actor.close_key_projection(second, MonoTime(130)).unwrap();
    assert_eq!(drain(&mut actor, 130), vec![(key("x"), KeyEdge::Down)]);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("x")));
    assert_eq!(actor.suppression_references(), 0);
}
#[test]
fn projecting_one_source_does_not_release_other_same_key_and_released_source_never_restores() {
    let mut actor = actor();
    for (id, dev) in [(1, 1), (2, 2)] {
        actor
            .enqueue(plan(&actor, id, dev, 100, &[key("x").into()], false))
            .unwrap();
        drain(&mut actor, 100);
    }
    let scope = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    assert!(drain(&mut actor, 110).is_empty());
    assert_eq!(actor.physical_repeat_target(BatchId(2)), Some(key("x")));
    actor.input_up(BatchId(1), MonoTime(120)).unwrap();
    assert!(drain(&mut actor, 120).is_empty());
    actor.close_key_projection(scope, MonoTime(130)).unwrap();
    assert!(drain(&mut actor, 130).is_empty());
    assert_eq!(actor.suppression_references(), 0);
    actor.input_up(BatchId(2), MonoTime(140)).unwrap();
    assert_eq!(drain(&mut actor, 140), vec![(key("x"), KeyEdge::Up)]);
}
#[test]
fn projection_markers_wait_for_original_future_array_then_queued_close_preserves_fifo() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
            1,
            1,
            100,
            &[
                Action {
                    repeat: false,
                    hold_down_ns: 50,
                    ..key("x").into()
                },
                key("y").into(),
            ],
            false,
        ))
        .unwrap();
    assert_eq!(drain(&mut actor, 100), vec![(key("x"), KeyEdge::Down)]);
    let scope = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    actor.close_key_projection(scope, MonoTime(120)).unwrap();
    assert!(drain(&mut actor, 149).is_empty());
    assert_eq!(
        drain(&mut actor, 150),
        vec![
            (key("x"), KeyEdge::Up),
            (key("y"), KeyEdge::Down),
            (key("y"), KeyEdge::Up),
            (key("y"), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.suppression_references(), 0);
}
#[test]
fn modifier_projection_remains_lazy_until_new_real_key_synchronization() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 1, 100, &[key("left_shift").into()], false))
        .unwrap();
    drain(&mut actor, 100);
    let scope = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    assert!(drain(&mut actor, 110).is_empty());
    actor
        .enqueue(plan(&actor, 2, 1, 120, &[key("x").into()], false))
        .unwrap();
    assert_eq!(
        drain(&mut actor, 120),
        vec![(key("left_shift"), KeyEdge::Up), (key("x"), KeyEdge::Down)]
    );
    actor.close_key_projection(scope, MonoTime(130)).unwrap();
    assert!(drain(&mut actor, 130).is_empty());
    actor
        .enqueue(plan(&actor, 3, 1, 140, &[key("y").into()], false))
        .unwrap();
    assert_eq!(
        drain(&mut actor, 140),
        vec![
            (key("left_shift"), KeyEdge::Down),
            (key("y"), KeyEdge::Down)
        ]
    );
}
#[test]
fn remove_and_reset_clear_projection_and_future_controls_without_fake_key_restoration_or_id_reuse()
{
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 1, 100, &[key("x").into()], false))
        .unwrap();
    drain(&mut actor, 100);
    let old = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    drain(&mut actor, 110);
    assert!(
        actor
            .observe_device(device(1), true, MonoTime(120))
            .unwrap()
            .is_empty()
    );
    assert_eq!(actor.suppression_references(), 0);
    assert!(!actor.projection_is_active(old));
    assert!(matches!(
        actor.close_key_projection(old, MonoTime(120)),
        Err(ActorError::InvalidProjection)
    ));
    actor
        .enqueue(plan(&actor, 2, 2, 130, &[key("y").into()], false))
        .unwrap();
    drain(&mut actor, 130);
    let new = actor
        .begin_key_projection(BatchId(2), MonoTime(140))
        .unwrap()
        .unwrap();
    assert_ne!(old, new);
    actor.reset(MonoTime(140)).unwrap();
    assert_eq!(actor.suppression_references(), 0);
    assert!(drain(&mut actor, 200).is_empty());
}
#[test]
fn variable_projection_is_queued_before_actual_variable_pause_and_reference_release() {
    let variable = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        ..key("vk_none").into()
    };
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "variable".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![variable.clone()],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
    });
    let mut actor = TerminalActor::new(cfg).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor
        .enqueue(plan(&actor, 1, 1, 100, &[variable], false))
        .unwrap();
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(100))
        .unwrap()
        .unwrap();
    assert_eq!(actor.suppression_references(), 1);
    assert!(drain(&mut actor, 100).is_empty());
    assert_eq!(
        actor.variable_values().get("v"),
        Some(&VariableValue::Integer(2))
    );
    actor.close_key_projection(id, MonoTime(110)).unwrap();
    drain(&mut actor, 110);
    assert_eq!(
        actor.variable_values().get("v"),
        Some(&VariableValue::Integer(1))
    );
    assert_eq!(actor.suppression_references(), 0);
}
#[test]
fn original_up_while_hidden_then_close_never_recreates_released_source() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 1, 100, &[key("x").into()], false))
        .unwrap();
    drain(&mut actor, 100);
    let scope = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    drain(&mut actor, 110);
    actor.input_up(BatchId(1), MonoTime(120)).unwrap();
    assert!(drain(&mut actor, 120).is_empty());
    actor.close_key_projection(scope, MonoTime(130)).unwrap();
    assert!(drain(&mut actor, 130).is_empty());
    assert!(actor.pressed_outputs().is_empty());
    assert_eq!(actor.suppression_references(), 0);
}
#[test]
fn snapshot_temporary_sources_are_included_in_booked_projection_references() {
    use keyweave_engine::{modifier_scope::ScopedActionPlan, rule_selection::*};
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "snapshot".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![],
            allow_extra_modifiers: true,
            extras: MappingExtras {
                if_alone: vec![Action {
                    hold_down_ns: 10,
                    modifiers: vec![key("left_alt")],
                    ..key("x").into()
                }],
                ..Default::default()
            },
        }],
    });
    let selection = select_rule(
        cfg.clone(),
        MappingStage::Complex,
        key("a"),
        std::collections::BTreeSet::from([key("left_shift"), key("left_control")]),
        |_| Truth::True,
    )
    .unwrap();
    let branch = selection
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    let mut actor = TerminalActor::new(cfg).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor
        .enqueue_scoped(
            ScopedActionPlan::branch(
                &branch,
                BatchId(1),
                MonoTime(100),
                device(1),
                SourceToken(1),
            )
            .unwrap(),
        )
        .unwrap();
    drain(&mut actor, 100);
    let scope = actor
        .begin_key_projection(BatchId(1), MonoTime(100))
        .unwrap()
        .unwrap();
    assert_eq!(actor.suppression_references(), 4); // two slots + two snapshot sources
    actor.close_key_projection(scope, MonoTime(100)).unwrap();
    drain(&mut actor, 110);
    assert_eq!(actor.suppression_references(), 0);
}
