#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, output_stage::*, terminal_actor::ActorError, timeline::*, *,
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
fn actor() -> OutputStageActor {
    let mut actor =
        OutputStageActor::new(Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))).unwrap();
    actor.observe_device(device(), false, MonoTime(0)).unwrap();
    actor
}
fn plan(actor: &OutputStageActor, id: u64, at: u64, actions: &[Action]) -> ActionPlan {
    plan_actions(
        actions,
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(at),
            device: device(),
            source: SourceToken(id),
            config: actor.snapshot().clone(),
        },
        false,
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
fn modifier_generation_hides_at_commit_and_real_source_follows_fifo_then_close_restores() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 100, &[key("left_shift").into()]))
        .unwrap();
    drain(&mut actor, 100);
    let guard = actor
        .prepare_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    guard.abort();
    assert!(actor.generated_modifiers().contains(&key("left_shift")));
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    assert!(!actor.generated_modifiers().contains(&key("left_shift")));
    assert!(actor.pressed_outputs().contains(&key("left_shift")));
    assert!(drain(&mut actor, 110).is_empty());
    actor
        .enqueue(plan(&actor, 2, 120, &[key("x").into()]))
        .unwrap();
    assert_eq!(
        drain(&mut actor, 120),
        vec![(key("left_shift"), KeyEdge::Up), (key("x"), KeyEdge::Down)]
    );
    actor
        .prepare_key_projection_close(id, MonoTime(130))
        .unwrap()
        .abort();
    assert!(!actor.generated_modifiers().contains(&key("left_shift")));
    actor.close_key_projection(id, MonoTime(130)).unwrap();
    assert!(actor.generated_modifiers().contains(&key("left_shift")));
    assert!(drain(&mut actor, 130).is_empty());
    actor
        .enqueue(plan(&actor, 3, 140, &[key("y").into()]))
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
fn ordinary_nested_projection_preserves_old_binding_across_reload_and_exact_receipts() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 100, &[key("x").into()]))
        .unwrap();
    drain(&mut actor, 100);
    let first = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    let second = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    assert_eq!(drain(&mut actor, 110), vec![(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor.close_key_projection(first, MonoTime(120)).unwrap();
    assert!(drain(&mut actor, 120).is_empty());
    actor.close_key_projection(second, MonoTime(130)).unwrap();
    assert_eq!(drain(&mut actor, 130), vec![(key("x"), KeyEdge::Down)]);
    assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key("x")));
}
#[test]
fn projection_close_after_target_actual_retirement_does_not_restore_or_require_old_recipe() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, 100, &[key("x").into()]))
        .unwrap();
    drain(&mut actor, 100);
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    drain(&mut actor, 110);
    actor.input_up(BatchId(1), MonoTime(120)).unwrap();
    assert!(drain(&mut actor, 120).is_empty());
    assert_eq!(actor.pending_batches(), 0);
    actor.close_key_projection(id, MonoTime(130)).unwrap();
    assert!(drain(&mut actor, 130).is_empty());
    assert!(!actor.is_failed());
    assert!(actor.pressed_outputs().is_empty());
}
#[test]
fn future_macro_projection_markers_stay_paired_behind_original_array() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
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
        ))
        .unwrap();
    drain(&mut actor, 100);
    let id = actor
        .begin_key_projection(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    actor.close_key_projection(id, MonoTime(120)).unwrap();
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
    assert!(!actor.is_failed());
}
#[test]
fn device_and_reset_cancel_both_projection_sides_and_queued_controls() {
    for removed in [true, false] {
        let mut actor = actor();
        actor
            .enqueue(plan(&actor, 1, 100, &[key("x").into()]))
            .unwrap();
        drain(&mut actor, 100);
        let id = actor
            .begin_key_projection(BatchId(1), MonoTime(110))
            .unwrap()
            .unwrap();
        drain(&mut actor, 110);
        let outputs = if removed {
            actor.observe_device(device(), true, MonoTime(120)).unwrap()
        } else {
            actor.reset(MonoTime(120)).unwrap()
        };
        assert!(outputs.is_empty());
        assert_eq!(actor.next_deadline(), None);
        assert!(matches!(
            actor.close_key_projection(id, MonoTime(120)),
            Err(OutputStageError::Terminal(ActorError::InvalidProjection))
        ));
        assert!(!actor.is_failed());
        assert!(drain(&mut actor, 200).is_empty());
    }
}
