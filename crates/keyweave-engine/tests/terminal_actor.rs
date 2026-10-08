#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, terminal_actor::*, timeline::Stage, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn cfg() -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn actor() -> TerminalActor {
    let mut actor = TerminalActor::new(cfg()).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor
}
fn plan(
    actor: &TerminalActor,
    id: u64,
    device: DeviceInstanceId,
    actions: &[Action],
    tap: bool,
) -> ActionPlan {
    plan_actions(
        actions,
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(0),
            device,
            source: SourceToken(100 + id),
            config: actor.snapshot().clone(),
        },
        tap,
    )
    .unwrap()
}
fn drain(actor: &mut TerminalActor, at: u64) -> Vec<(u64, String, KeyEdge)> {
    let mut result = vec![];
    while let Some(step) = actor.poll(MonoTime(at)).unwrap() {
        result.extend(
            step.outputs
                .into_iter()
                .map(|o| (o.at_ns.0, o.key.name(), o.edge)),
        );
    }
    result
}
#[test]
fn scheduled_up_and_following_down_execute_only_when_due_then_original_up_releases_saved_bound() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
            1,
            device(1),
            &[
                Action {
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("c").into(),
            ],
            false,
        ))
        .unwrap();
    assert_eq!(drain(&mut actor, 0), [(0, "b".into(), KeyEdge::Down)]);
    assert!(drain(&mut actor, 49).is_empty());
    assert_eq!(
        drain(&mut actor, 50),
        [
            (50, "b".into(), KeyEdge::Up),
            (50, "c".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.live_slots(), 1);
    assert!(actor.input_up(BatchId(1), MonoTime(60)).unwrap());
    assert_eq!(
        actor.input_up(BatchId(1), MonoTime(60)),
        Err(ActorError::Engine(EngineError::DeadlinePending))
    );
    assert_eq!(drain(&mut actor, 60), [(60, "c".into(), KeyEdge::Up)]);
    assert!(!actor.input_up(BatchId(1), MonoTime(60)).unwrap());
    assert_eq!(actor.reserved_slots(), 0);
    assert_eq!(actor.pending_batches(), 0);
}
#[test]
fn early_original_up_waits_for_prior_planned_taps_and_due_work_must_drain_first() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
            1,
            device(1),
            &[
                Action {
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("c").into(),
            ],
            false,
        ))
        .unwrap();
    assert_eq!(
        actor.input_up(BatchId(1), MonoTime(0)),
        Err(ActorError::Engine(EngineError::DeadlinePending))
    );
    drain(&mut actor, 0);
    actor.input_up(BatchId(1), MonoTime(10)).unwrap();
    assert!(drain(&mut actor, 49).is_empty());
    assert_eq!(
        drain(&mut actor, 50),
        [
            (50, "b".into(), KeyEdge::Up),
            (50, "c".into(), KeyEdge::Down),
            (50, "c".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.live_slots(), 0);
}
#[test]
fn scopes_hold_across_duration_and_lazy_neutral_flush_releases_only_at_batch_end() {
    let mut actor = actor();
    let action = Action {
        modifiers: vec![key("left_control")],
        hold_down_ns: 50,
        ..key("b").into()
    };
    actor
        .enqueue(plan(&actor, 1, device(1), &[action], true))
        .unwrap();
    assert_eq!(
        drain(&mut actor, 0),
        [
            (0, "left_control".into(), KeyEdge::Down),
            (0, "b".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(
        drain(&mut actor, 50),
        [
            (50, "b".into(), KeyEdge::Up),
            (50, "left_control".into(), KeyEdge::Up)
        ]
    );
    assert!(actor.pressed_outputs().is_empty());
}
#[test]
fn two_batches_can_own_same_key_and_one_cancel_cannot_release_the_other() {
    let mut actor = actor();
    actor
        .enqueue(plan(&actor, 1, device(1), &[key("b").into()], false))
        .unwrap();
    actor
        .enqueue(plan(&actor, 2, device(1), &[key("b").into()], false))
        .unwrap();
    assert_eq!(drain(&mut actor, 0), [(0, "b".into(), KeyEdge::Down)]);
    assert!(
        actor
            .cancel_batch(BatchId(1), MonoTime(1))
            .unwrap()
            .is_empty()
    );
    assert_eq!(actor.pressed_outputs(), [key("b")]);
    assert_eq!(
        actor.cancel_batch(BatchId(2), MonoTime(2)).unwrap(),
        [OutputIntent {
            at_ns: MonoTime(2),
            key: key("b"),
            edge: KeyEdge::Up
        }]
    );
}
#[test]
fn unplug_cancels_unexecuted_down_and_releases_only_executed_owners() {
    let mut actor = actor();
    actor.observe_device(device(2), false, MonoTime(0)).unwrap();
    actor
        .enqueue(plan(
            &actor,
            1,
            device(1),
            &[
                Action {
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("c").into(),
            ],
            false,
        ))
        .unwrap();
    actor
        .enqueue(plan(&actor, 2, device(2), &[key("d").into()], false))
        .unwrap();
    drain(&mut actor, 0);
    assert_eq!(
        actor.observe_device(device(1), true, MonoTime(10)).unwrap(),
        [OutputIntent {
            at_ns: MonoTime(10),
            key: key("b"),
            edge: KeyEdge::Up
        }]
    );
    assert_eq!(drain(&mut actor, 50), [(50, "d".into(), KeyEdge::Down)]);
    assert_eq!(actor.pressed_outputs(), [key("d")]);
    assert_eq!(actor.pending_batches(), 1);
    assert!(matches!(
        actor.enqueue(plan(&actor, 3, device(1), &[key("e").into()], false)),
        Err(ActorError::Timeline(_))
    ));
}
#[test]
fn reset_clears_slots_plans_and_reservations_without_reusing_batch_identity() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
            1,
            device(1),
            &[Action {
                hold_down_ns: 50,
                ..key("b").into()
            }],
            true,
        ))
        .unwrap();
    drain(&mut actor, 0);
    assert_eq!(
        actor.reset(MonoTime(10)).unwrap(),
        [OutputIntent {
            at_ns: MonoTime(10),
            key: key("b"),
            edge: KeyEdge::Up
        }]
    );
    assert_eq!(actor.queued_events(), 0);
    assert_eq!(actor.live_slots(), 0);
    assert_eq!(actor.reserved_slots(), 0);
    assert!(drain(&mut actor, 50).is_empty());
    assert_eq!(
        actor.enqueue(plan(&actor, 1, device(1), &[key("c").into()], false)),
        Err(ActorError::InvalidBatch)
    );
}
#[test]
fn reload_does_not_change_up_target_or_old_variable_up_value() {
    let variable = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "mode".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(0)),
            },
        }),
        ..key("b").into()
    };
    let mut config = (*cfg()).clone();
    config.complex = vec![Mapping {
        id: "v".into(),
        from: key("a"),
        to: vec![variable.clone()],
        allow_extra_modifiers: true,
        extras: Default::default(),
    }];
    let mut actor = TerminalActor::new(Arc::new(config)).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor
        .enqueue(plan(&actor, 1, device(1), &[key("b").into()], false))
        .unwrap();
    actor
        .enqueue(plan(&actor, 2, device(1), &[variable], false))
        .unwrap();
    drain(&mut actor, 0);
    assert_eq!(actor.variable_values()["mode"], VariableValue::Integer(1));
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor.input_up(BatchId(1), MonoTime(10)).unwrap();
    assert_eq!(drain(&mut actor, 10), [(10, "b".into(), KeyEdge::Up)]);
    actor.input_up(BatchId(2), MonoTime(10)).unwrap();
    assert!(drain(&mut actor, 10).is_empty());
    assert_eq!(actor.variable_values()["mode"], VariableValue::Integer(0));
}
#[test]
fn late_delivery_uses_actual_time_and_halt_does_not_stop_current_batch() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
            1,
            device(1),
            &[
                Action {
                    halt: true,
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("c").into(),
            ],
            true,
        ))
        .unwrap();
    let step = actor.poll(MonoTime(20)).unwrap().unwrap();
    assert!(step.halt_requested);
    assert_eq!(step.outputs[0].at_ns, MonoTime(20));
    assert_eq!(
        drain(&mut actor, 60),
        [
            (60, "b".into(), KeyEdge::Up),
            (60, "c".into(), KeyEdge::Down),
            (60, "c".into(), KeyEdge::Up)
        ]
    );
}
#[test]
fn unrelated_connect_and_stale_batch_cancel_do_not_flush_lazy_modifier() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
            1,
            device(1),
            &[Action {
                lazy: true,
                ..key("left_control").into()
            }],
            false,
        ))
        .unwrap();
    assert!(drain(&mut actor, 0).is_empty());
    assert!(
        actor
            .observe_device(device(2), false, MonoTime(1))
            .unwrap()
            .is_empty()
    );
    assert!(
        actor
            .cancel_batch(BatchId(99), MonoTime(2))
            .unwrap()
            .is_empty()
    );
    assert!(actor.pressed_outputs().is_empty());
    assert!(actor.reset(MonoTime(3)).unwrap().is_empty());
}
#[test]
fn admission_rejects_nonterminal_or_foreign_snapshot_without_allocating_slots() {
    let mut actor = actor();
    let mut origin = PlanOrigin {
        batch: BatchId(1),
        stage: Stage::Complex,
        start: MonoTime(0),
        device: device(1),
        source: SourceToken(1),
        config: actor.snapshot().clone(),
    };
    assert_eq!(
        actor.enqueue(plan_actions(&[key("b").into()], origin.clone(), false).unwrap()),
        Err(ActorError::UnsupportedStage)
    );
    origin.stage = Stage::Output;
    origin.config = cfg();
    assert_eq!(
        actor.enqueue(plan_actions(&[key("b").into()], origin, false).unwrap()),
        Err(ActorError::InvalidSnapshot)
    );
    assert_eq!(actor.pending_batches(), 0);
    assert_eq!(actor.reserved_slots(), 0);
    assert_eq!(actor.queued_events(), 0);
}

#[test]
fn future_slot_capacity_is_reserved_and_rejected_batch_does_not_consume_identity() {
    let mut actor = actor();
    let actions = vec![Action::from(key("b")); 512];
    for id in 1..=32 {
        actor
            .enqueue(plan(&actor, id, device(1), &actions, false))
            .unwrap();
        drain(&mut actor, 0);
    }
    assert_eq!(actor.reserved_slots(), MAX_OUTPUT_OWNERS);
    assert_eq!(actor.live_slots(), 32);
    assert_eq!(
        actor.enqueue(plan(&actor, 33, device(1), &[key("b").into()], false)),
        Err(ActorError::Capacity)
    );
    assert_eq!(actor.pending_batches(), 32);
    assert!(
        actor
            .cancel_batch(BatchId(1), MonoTime(0))
            .unwrap()
            .is_empty()
    );
    actor
        .enqueue(plan(&actor, 33, device(1), &[key("b").into()], false))
        .unwrap();
    drain(&mut actor, 0);
    assert_eq!(actor.reset(MonoTime(0)).unwrap().len(), 1);
}
#[test]
fn stage_queue_full_is_atomic_and_cancel_reclaims_unexecuted_work() {
    let mut actor = actor();
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 1..=8 {
        actor
            .enqueue(plan(&actor, id, device(1), &actions, true))
            .unwrap();
    }
    assert_eq!(actor.queued_events(), 8192);
    assert!(matches!(
        actor.enqueue(plan(&actor, 9, device(1), &actions, true)),
        Err(ActorError::Timeline(_))
    ));
    assert_eq!(actor.reserved_slots(), 4096);
    assert!(
        actor
            .cancel_batch(BatchId(1), MonoTime(0))
            .unwrap()
            .is_empty()
    );
    actor
        .enqueue(plan(&actor, 9, device(1), &actions, true))
        .unwrap();
    assert_eq!(actor.queued_events(), 8192);
    actor.reset(MonoTime(0)).unwrap();
    assert_eq!(actor.reserved_slots(), 0);
}

#[test]
fn key_budget_includes_lazily_unreleased_emitted_keys_of_a_completed_batch() {
    let mut actor = actor();
    actor
        .enqueue(plan(
            &actor,
            1,
            device(1),
            &[Action {
                lazy: true,
                ..key("left_control").into()
            }],
            false,
        ))
        .unwrap();
    actor
        .enqueue(plan(&actor, 2, device(1), &[key("b").into()], false))
        .unwrap();
    drain(&mut actor, 0);
    actor.input_up(BatchId(1), MonoTime(10)).unwrap();
    assert!(drain(&mut actor, 10).is_empty());
    assert_eq!(actor.pressed_outputs().len(), 2);
    let actions: Vec<_> = (1000..1511)
        .map(|usage| Action::from(KeyId::new(7, usage)))
        .collect();
    assert_eq!(
        actor.enqueue(plan(&actor, 3, device(1), &actions, false)),
        Err(ActorError::Capacity)
    );
    assert_eq!(actor.queued_events(), 0);
    assert_eq!(actor.reserved_slots(), 1);
}
