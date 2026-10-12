#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, complex_actor::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn mapping(from: &str, to: Vec<Action>) -> Mapping {
    Mapping {
        id: from.into(),
        from: key(from),
        to,
        allow_extra_modifiers: true,
        extras: Default::default(),
    }
}
fn config(to: Vec<Action>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "owned-complex".into(),
        simple: vec![],
        complex: vec![mapping("a", to)],
    })
}
fn actor(config: Arc<ConfigSnapshot>) -> ComplexStageActor {
    let mut actor = ComplexStageActor::new(config).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor.observe_device(device(2), false, MonoTime(0)).unwrap();
    actor
}
fn input(actor: &mut ComplexStageActor, actions: &[Action], device_id: u64, at: u64) -> ActionPlan {
    let id = actor.next_batch_id().unwrap();
    let plan = plan_actions(
        actions,
        PlanOrigin {
            batch: id,
            stage: Stage::Complex,
            start: MonoTime(at),
            device: device(device_id),
            source: SourceToken(id.0),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    actor.register_input(plan.clone()).unwrap();
    plan
}
fn down(plan: &ActionPlan, index: u16, at: u64) -> DueEvent<PlannedAction> {
    DueEvent {
        event: plan
            .events()
            .iter()
            .find(|e| e.payload.edge == KeyEdge::Down && e.payload.slot.index == index)
            .unwrap()
            .clone(),
        delivered_at: MonoTime(at),
    }
}
fn up(plan: &ActionPlan, index: u16, at: u64) -> DueEvent<PlannedAction> {
    let event = plan
        .input_up_events(MonoTime(at))
        .into_iter()
        .find(|e| e.payload.slot.index == index)
        .or_else(|| {
            plan.events()
                .iter()
                .find(|e| e.payload.edge == KeyEdge::Up && e.payload.slot.index == index)
                .cloned()
        })
        .unwrap();
    DueEvent {
        event,
        delivered_at: MonoTime(at),
    }
}
fn drain(actor: &mut ComplexStageActor, at: u64) -> (Vec<(String, KeyEdge)>, Vec<ActionSlot>) {
    let mut outputs = vec![];
    let mut released = vec![];
    while let Some(step) = actor.poll(MonoTime(at)).unwrap() {
        outputs.extend(step.outputs.into_iter().map(|o| (o.key.name(), o.edge)));
        released.extend(step.released_producers);
    }
    (outputs, released)
}
#[test]
fn owns_down_saved_up_output_completion_and_root_retirement_across_reload() {
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let receipt = down(&plan, 0, 125);
    let slot = receipt.event.payload.slot;
    actor.admit(&receipt, &ContextSnapshot::default()).unwrap();
    assert_eq!(actor.reserved_sources(), 2);
    assert_eq!(drain(&mut actor, 125).0, [("b".into(), KeyEdge::Down)]);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor
        .admit(&up(&plan, 0, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        actor.producer(slot).unwrap().up_delivered_at(),
        Some(MonoTime(150))
    );
    let (outputs, released) = drain(&mut actor, 150);
    assert_eq!(outputs, [("b".into(), KeyEdge::Up)]);
    assert_eq!(released, [slot]);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn early_original_up_waits_array_duration_and_does_not_release_future_producer_work() {
    let mut actor = actor(config(vec![
        Action {
            hold_down_ns: 50,
            ..key("b").into()
        },
        key("c").into(),
    ]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let receipt = down(&plan, 0, 100);
    let slot = receipt.event.payload.slot;
    actor.admit(&receipt, &ContextSnapshot::default()).unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Down)]);
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert!(actor.producer(slot).is_some());
    assert!(actor.poll(MonoTime(149)).unwrap().is_none());
    let (outputs, released) = drain(&mut actor, 150);
    assert_eq!(
        outputs,
        [
            ("b".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(released, [slot]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn matched_empty_main_preserves_parent_until_original_up_and_never_emits_fallback() {
    let mut actor = actor(config(vec![]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let receipt = down(&plan, 0, 100);
    let slot = receipt.event.payload.slot;
    actor.admit(&receipt, &ContextSnapshot::default()).unwrap();
    assert_eq!(actor.reserved_sources(), 1);
    assert!(actor.poll(MonoTime(100)).unwrap().is_none());
    let step = actor
        .admit(&up(&plan, 0, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(step.released_producers, [slot]);
    assert!(step.outputs.is_empty());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn two_sources_share_terminal_key_and_device_cleanup_preserves_the_other_owner() {
    let mut actor = actor(config(vec![key("b").into()]));
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Down)]);
    let second = input(&mut actor, &[key("a").into()], 2, 100);
    actor
        .admit(&down(&second, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    let step = actor
        .observe_device(device(1), true, MonoTime(110))
        .unwrap();
    assert!(step.outputs.is_empty());
    assert_eq!(
        step.discarded_input_batches,
        [first.events()[0].payload.slot.batch]
    );
    assert_eq!(actor.reserved_sources(), 2);
    actor
        .admit(&up(&second, 0, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 150).0, [("b".into(), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn pending_output_requires_poll_before_receipt_retry_without_selecting_or_failing_input() {
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    let receipt = up(&plan, 0, 100);
    assert!(matches!(
        actor.admit(&receipt, &ContextSnapshot::default()),
        Err(ComplexActorError::DeadlinePending)
    ));
    assert!(!actor.is_failed());
    assert_eq!(
        actor
            .producer(receipt.event.payload.slot)
            .unwrap()
            .up_delivered_at(),
        None
    );
    drain(&mut actor, 100);
    actor.admit(&receipt, &ContextSnapshot::default()).unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Up)]);
    assert_eq!(actor.pending_inputs(), 0);
}
#[test]
fn invalid_popped_down_faults_once_and_reset_discards_input_without_fabricated_up() {
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let mut receipt = down(&plan, 0, 100);
    receipt.event.payload.target = ActionTarget::Key(key("x"));
    assert!(matches!(
        actor.admit(&receipt, &ContextSnapshot::default()),
        Err(ComplexActorError::Input)
    ));
    assert!(actor.is_failed());
    assert!(matches!(
        actor.admit(&down(&plan, 0, 100), &ContextSnapshot::default()),
        Err(ComplexActorError::Failed)
    ));
    let step = actor.reset(MonoTime(100)).unwrap();
    assert!(step.outputs.is_empty());
    assert_eq!(
        step.discarded_input_batches,
        [plan.events()[0].payload.slot.batch]
    );
    assert!(!actor.is_failed());
    assert_eq!(actor.reserved_sources(), 0);
    assert!(actor.next_batch_id().unwrap().0 > plan.events()[0].payload.slot.batch.0);
}
#[test]
fn halt_is_associated_with_exact_output_batch_and_incoming_halt_is_returned_to_upstream() {
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "halt".into(),
        simple: vec![],
        complex: vec![
            mapping(
                "a",
                vec![Action {
                    halt: true,
                    ..key("b").into()
                }],
            ),
            mapping("x", vec![key("c").into()]),
        ],
    });
    let mut actor = actor(cfg);
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    let slot_a = down(&first, 0, 100).event.payload.slot;
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    assert!(actor.producer_halted(slot_a));
    let second = input(
        &mut actor,
        &[Action {
            halt: true,
            ..key("x").into()
        }],
        1,
        100,
    );
    let slot_x = down(&second, 0, 100).event.payload.slot;
    let step = actor
        .admit(&down(&second, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(step.originating_halt, Some(slot_x));
    drain(&mut actor, 100);
    assert!(!actor.producer_halted(slot_x));
    let step = actor.reset(MonoTime(150)).unwrap();
    assert_eq!(step.outputs.len(), 2);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn advanced_branches_are_explicitly_rejected_and_activation_keeps_current_snapshot() {
    let old = config(vec![key("b").into()]);
    let mut actor = actor(old.clone());
    let mut candidate = (*old).clone();
    candidate.generation = ConfigGeneration(2);
    candidate.complex[0].extras.if_alone.push(key("c").into());
    assert!(matches!(
        ComplexStageActor::new(Arc::new(candidate.clone())),
        Err(ComplexActorError::UnsupportedBranches)
    ));
    assert!(matches!(
        actor.activate(Arc::new(candidate)),
        Err(ComplexActorError::UnsupportedBranches)
    ));
    assert!(Arc::ptr_eq(actor.snapshot(), &old));
    assert!(!actor.is_failed());
}
#[test]
fn registered_modifier_scope_preserves_nonlazy_down_lazy_up_through_passthrough_and_flushes_later()
{
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(
        &mut actor,
        &[Action {
            modifiers: vec![key("left_shift")],
            ..key("right_control").into()
        }],
        1,
        100,
    );
    assert!(!down(&plan, 0, 100).event.payload.lazy);
    assert!(up(&plan, 0, 150).event.payload.lazy);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 100).0,
        [("left_shift".into(), KeyEdge::Down)]
    );
    actor
        .admit(&down(&plan, 1, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 100).0,
        [("right_control".into(), KeyEdge::Down)]
    );
    // A different held input on the same device prevents the existing terminal
    // last-device-batch flush, so the attached modifier's lazy Up stays latent.
    let other = input(&mut actor, &[key("x").into()], 1, 100);
    actor
        .admit(&down(&other, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("x".into(), KeyEdge::Down)]);
    actor
        .admit(&up(&plan, 1, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 150).0,
        [("right_control".into(), KeyEdge::Up)]
    );
    actor
        .admit(&up(&plan, 0, 150), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 150).0.is_empty());
    assert_eq!(actor.reserved_sources(), 2);
    let next = input(&mut actor, &[key("a").into()], 1, 200);
    actor
        .admit(&down(&next, 0, 200), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 200).0,
        [
            ("left_shift".into(), KeyEdge::Up),
            ("b".into(), KeyEdge::Down)
        ]
    );
}
#[test]
fn variable_generation_changes_next_selection_but_saved_key_up_remains_frozen() {
    let assign = Action {
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
        name: "variables".into(),
        simple: vec![],
        complex: vec![
            mapping("a", vec![assign]),
            mapping(
                "x",
                vec![Action {
                    conditions: vec![Condition::Variable {
                        name: "v".into(),
                        value: VariableValue::Integer(1),
                        unless: false,
                    }],
                    ..key("d").into()
                }],
            ),
        ],
    });
    let mut actor = actor(cfg);
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    assert!(drain(&mut actor, 100).0.is_empty());
    let next = input(&mut actor, &[key("x").into()], 1, 100);
    actor
        .admit(&down(&next, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("d".into(), KeyEdge::Down)]);
    actor
        .admit(&up(&first, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    drain(&mut actor, 110);
    actor
        .admit(&up(&next, 0, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 150).0, [("d".into(), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn early_forged_up_cannot_bypass_registered_input_tap_duration_and_fault_recovery_uses_real_down() {
    let mut actor = actor(config(vec![key("b").into()]));
    let plan = input(
        &mut actor,
        &[Action {
            repeat: false,
            hold_down_ns: 50,
            ..key("a").into()
        }],
        1,
        100,
    );
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let mut too_early = up(&plan, 0, 150);
    too_early.event.at_ns = MonoTime(110);
    too_early.delivered_at = MonoTime(110);
    assert!(matches!(
        actor.admit(&too_early, &ContextSnapshot::default()),
        Err(ComplexActorError::Input)
    ));
    assert!(actor.is_failed());
    let recovery = actor.reset(MonoTime(110)).unwrap();
    assert_eq!(
        recovery
            .outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("b"), KeyEdge::Up)]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn complex_consumed_modifier_scope_restores_generated_visibility_and_actual_lazy_sync() {
    let mut rule = mapping("a", vec![key("right_control").into()]);
    rule.extras.from_modifiers.mandatory.insert(Modifier::Shift);
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "scope".into(),
        simple: vec![],
        complex: vec![rule],
    });
    let mut actor = actor(cfg);
    let shift = input(&mut actor, &[key("left_shift").into()], 1, 100);
    actor
        .admit(&down(&shift, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 100).0,
        [("left_shift".into(), KeyEdge::Down)]
    );
    let main = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&main, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(!actor.generated_modifiers().contains(&key("left_shift")));
    assert_eq!(
        drain(&mut actor, 100).0,
        [
            ("left_shift".into(), KeyEdge::Up),
            ("right_control".into(), KeyEdge::Down)
        ]
    );
    actor
        .admit(&up(&main, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert!(actor.generated_modifiers().contains(&key("left_shift")));
    assert_eq!(
        drain(&mut actor, 110).0,
        [("right_control".into(), KeyEdge::Up)]
    );
    assert_eq!(actor.reserved_sources(), 2);
    let next = input(&mut actor, &[key("z").into()], 1, 150);
    actor
        .admit(&down(&next, 0, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 150).0,
        [
            ("left_shift".into(), KeyEdge::Down),
            ("z".into(), KeyEdge::Down)
        ]
    );
    actor.reset(MonoTime(200)).unwrap();
    assert_eq!(actor.reserved_sources(), 0);
}
