#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, output_stage::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn variable(down: Option<i64>, up: Option<i64>) -> Action {
    Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: down.map(VariableValue::Integer),
                key_up_value: up.map(VariableValue::Integer),
            },
        }),
        ..key("vk_none").into()
    }
}
fn cfg(generation: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "projection vars".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "declare".into(),
            from: key("a"),
            to: vec![variable(Some(9), Some(8))],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
    })
}
fn actor() -> OutputStageActor {
    let mut actor = OutputStageActor::new(cfg(1)).unwrap();
    actor
        .observe_device(
            DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            false,
            MonoTime(0),
        )
        .unwrap();
    actor
}
fn plan(actor: &OutputStageActor, id: u64, at: u64, action: Action) -> ActionPlan {
    plan_actions(
        &[action],
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(at),
            device: DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            source: SourceToken(id),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap()
}
fn drain(actor: &mut OutputStageActor, at: u64) {
    for _ in 0..10000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return;
        };
        assert!(step.outputs.is_empty());
    }
    panic!("fixture failed to drain");
}
fn generated(actor: &OutputStageActor) -> Option<VariableValue> {
    actor.generated_variables().get("v").cloned()
}
fn actual(actor: &OutputStageActor) -> Option<VariableValue> {
    actor.executed_variables().get("v").cloned()
}
fn n(value: i64) -> Option<VariableValue> {
    Some(VariableValue::Integer(value))
}
#[test]
fn variable_projection_pauses_at_generation_then_actual_fifo_and_restores_saved_old_operation() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 100, variable(Some(1), Some(2))))
        .unwrap();
    drain(&mut actor, 100);
    actor
        .prepare_key_projection(BatchId(1), MonoTime(200))
        .unwrap()
        .unwrap()
        .abort();
    assert_eq!(generated(&actor), n(1));
    assert_eq!(actual(&actor), n(1));
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    assert_eq!(generated(&actor), n(2));
    assert_eq!(actual(&actor), n(1));
    drain(&mut actor, 110);
    assert_eq!(actual(&actor), n(2));
    actor.activate(cfg(2)).unwrap();
    actor
        .prepare_key_projection_close(id, MonoTime(200))
        .unwrap()
        .abort();
    assert_eq!(generated(&actor), n(2));
    actor.close_key_projection(id, MonoTime(120)).unwrap();
    assert_eq!(generated(&actor), n(1));
    assert_eq!(actual(&actor), n(2));
    drain(&mut actor, 120);
    assert_eq!(actual(&actor), n(1));
}
#[test]
fn nested_reference_pauses_once_and_last_close_replays_original_down_after_other_assignment() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 100, variable(Some(1), Some(2))))
        .unwrap();
    drain(&mut actor, 100);
    let first = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    drain(&mut actor, 110);
    actor
        .enqueue(plan(&actor, 2, 120, variable(Some(3), Some(4))))
        .unwrap();
    drain(&mut actor, 120);
    let second = actor
        .begin_key_projection(BatchId(1), MonoTime(130))
        .unwrap()
        .unwrap();
    assert_eq!(generated(&actor), n(3));
    drain(&mut actor, 130);
    assert_eq!(actual(&actor), n(3));
    actor.close_key_projection(first, MonoTime(140)).unwrap();
    assert_eq!(generated(&actor), n(3));
    drain(&mut actor, 140);
    assert_eq!(actual(&actor), n(3));
    actor.close_key_projection(second, MonoTime(150)).unwrap();
    assert_eq!(generated(&actor), n(1));
    drain(&mut actor, 150);
    assert_eq!(actual(&actor), n(1));
}
#[test]
fn original_up_releases_source_and_close_does_not_restore_over_new_variable_assignment() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 100, variable(Some(1), Some(2))))
        .unwrap();
    drain(&mut actor, 100);
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    drain(&mut actor, 110);
    actor.input_up(BatchId(1), MonoTime(120)).unwrap();
    drain(&mut actor, 120);
    actor
        .enqueue(plan(&actor, 2, 130, variable(Some(50), Some(60))))
        .unwrap();
    drain(&mut actor, 130);
    actor.close_key_projection(id, MonoTime(140)).unwrap();
    assert_eq!(generated(&actor), n(50));
    drain(&mut actor, 140);
    assert_eq!(actual(&actor), n(50));
}
#[test]
fn unset_and_missing_down_value_keep_configured_operation_semantics() {
    for unset in [true, false] {
        let mut actor = actor();
        actor
            .enqueue(plan(&actor, 1, 100, variable(Some(3), Some(4))))
            .unwrap();
        drain(&mut actor, 100);
        let action = if unset {
            Action {
                target: ActionTarget::SetVariable(VariableAction {
                    name: "v".into(),
                    operation: VariableOperation::Unset,
                }),
                ..key("vk_none").into()
            }
        } else {
            variable(None, Some(2))
        };
        actor.enqueue(plan(&actor, 2, 110, action)).unwrap();
        drain(&mut actor, 110);
        let id = actor
            .begin_key_projection(BatchId(2), MonoTime(120))
            .unwrap()
            .unwrap();
        drain(&mut actor, 120);
        actor.close_key_projection(id, MonoTime(130)).unwrap();
        drain(&mut actor, 130);
        assert_eq!(generated(&actor), if unset { None } else { n(2) });
        assert_eq!(actual(&actor), if unset { None } else { n(2) });
    }
}
#[test]
fn generated_tap_up_already_removed_source_never_overwrites_later_assignment_on_projection() {
    let mut actor = actor();
    let tap = Action {
        repeat: false,
        hold_down_ns: 50,
        ..variable(Some(1), Some(2))
    };
    actor.enqueue(plan(&actor, 1, 100, tap)).unwrap();
    assert_eq!(generated(&actor), n(2));
    drain(&mut actor, 100);
    assert_eq!(actual(&actor), n(1));
    actor
        .enqueue(plan(&actor, 2, 110, variable(Some(3), Some(4))))
        .unwrap();
    assert_eq!(generated(&actor), n(3));
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(120))
        .unwrap()
        .unwrap();
    assert_eq!(generated(&actor), n(3));
    drain(&mut actor, 150);
    assert_eq!(actual(&actor), n(3));
    actor.close_key_projection(id, MonoTime(160)).unwrap();
    drain(&mut actor, 160);
    assert_eq!(generated(&actor), n(3));
    assert_eq!(actual(&actor), n(3));
}
#[test]
fn cancel_uses_original_up_and_reset_clears_variable_without_temporary_down_restore() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 100, variable(Some(1), Some(2))))
        .unwrap();
    drain(&mut actor, 100);
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    drain(&mut actor, 110);
    actor.cancel_batch(BatchId(1), MonoTime(120)).unwrap();
    assert_eq!(generated(&actor), n(2));
    assert_eq!(actual(&actor), n(2));
    assert!(actor.close_key_projection(id, MonoTime(120)).is_err());
    actor.reset(MonoTime(130)).unwrap();
    assert_eq!(generated(&actor), None);
    assert_eq!(actual(&actor), None);
}
