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
fn delayed_configuration_is_accepted_and_activation_uses_new_snapshot() {
    let old = config(vec![key("b").into()]);
    let mut actor = actor(old.clone());
    let mut candidate = (*old).clone();
    candidate.generation = ConfigGeneration(2);
    candidate.complex[0].extras.delayed = Some(DelayedAction {
        if_invoked: vec![key("c").into()],
        if_canceled: vec![],
        delay_ns: 100,
    });
    let new = Arc::new(candidate);
    ComplexStageActor::new(new.clone()).unwrap();
    actor.activate(new.clone()).unwrap();
    assert!(Arc::ptr_eq(actor.snapshot(), &new));
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
fn after_config(main: Vec<Action>, after: Vec<Action>) -> Arc<ConfigSnapshot> {
    let mut cfg = (*config(main)).clone();
    cfg.complex[0].extras.after_key_up = after;
    Arc::new(cfg)
}
#[test]
fn empty_main_after_up_branch_keeps_parent_until_its_actual_tap_finishes() {
    let mut actor = actor(after_config(
        vec![],
        vec![Action {
            hold_down_ns: 20,
            ..key("d").into()
        }],
    ));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert!(actor.producer(slot).is_some());
    assert_eq!(drain(&mut actor, 110).0, [("d".into(), KeyEdge::Down)]);
    assert!(actor.poll(MonoTime(129)).unwrap().is_none());
    let (outputs, released) = drain(&mut actor, 130);
    assert_eq!(outputs, [("d".into(), KeyEdge::Up)]);
    assert_eq!(released, [slot]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn after_up_filters_context_at_generation_and_fifo_waits_main_duration_and_scope_completion() {
    let condition = Condition::Application {
        bundle_identifiers: vec![],
        file_paths: vec![Pattern::compile("editor").unwrap()],
        unless: false,
    };
    let cfg = after_config(
        vec![
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            key("c").into(),
        ],
        vec![Action {
            hold_down_ns: 20,
            conditions: vec![condition],
            ..key("d").into()
        }],
    );
    let mut actor = actor(cfg);
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let context = ContextSnapshot {
        revision: 1,
        provider: "test".into(),
        observed_at_ns: MonoTime(100),
        valid_until_ns: MonoTime(130),
        application: Some(ApplicationContext {
            bundle_identifier: None,
            file_path: Some("editor".into()),
        }),
        ..Default::default()
    };
    actor.admit(&up(&plan, 0, 110), &context).unwrap();
    assert!(actor.poll(MonoTime(149)).unwrap().is_none());
    assert_eq!(
        drain(&mut actor, 150).0,
        [
            ("b".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Up),
            ("d".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.pending_inputs(), 1);
    assert_eq!(drain(&mut actor, 170).0, [("d".into(), KeyEdge::Up)]);
    assert_eq!(actor.pending_inputs(), 0);
}
#[test]
fn after_up_reads_main_key_up_variable_and_filters_whole_array_before_its_own_assignments() {
    let var = |value, up_value| Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(value)),
                key_up_value: Some(VariableValue::Integer(up_value)),
            },
        }),
        ..key("vk_none").into()
    };
    let cfg = after_config(
        vec![var(1, 2)],
        vec![
            var(3, 4),
            Action {
                hold_down_ns: 20,
                conditions: vec![Condition::Variable {
                    name: "v".into(),
                    value: VariableValue::Integer(2),
                    unless: false,
                }],
                ..key("d").into()
            },
        ],
    );
    let mut actor = actor(cfg);
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(4))
    );
    assert_eq!(drain(&mut actor, 110).0, [("d".into(), KeyEdge::Down)]);
    assert_eq!(drain(&mut actor, 130).0, [("d".into(), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn generated_future_halt_suppresses_after_up_but_incoming_halt_does_not_suppress_new_rule() {
    let cfg = after_config(
        vec![
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            Action {
                halt: true,
                ..key("c").into()
            },
        ],
        vec![key("d").into()],
    );
    let mut halted = actor(cfg);
    let plan = input(&mut halted, &[key("a").into()], 1, 100);
    let slot = down(&plan, 0, 100).event.payload.slot;
    halted
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(halted.producer_halted(slot));
    drain(&mut halted, 100);
    halted
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut halted, 150).0,
        [
            ("b".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(halted.reserved_sources(), 0);
    let mut normal = actor(after_config(vec![], vec![key("d").into()]));
    let plan = input(
        &mut normal,
        &[Action {
            halt: true,
            ..key("a").into()
        }],
        1,
        100,
    );
    let step = normal
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(step.originating_halt.is_some());
    normal
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut normal, 110).0,
        [("d".into(), KeyEdge::Down), ("d".into(), KeyEdge::Up)]
    );
}
#[test]
fn after_up_uses_retained_rule_after_reload_and_device_control_cleans_executed_branch() {
    let old = after_config(
        vec![key("b").into()],
        vec![Action {
            hold_down_ns: 20,
            ..key("d").into()
        }],
    );
    let mut actor = actor(old.clone());
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let mut new = (*old).clone();
    new.generation = ConfigGeneration(2);
    new.complex[0].extras.after_key_up = vec![key("e").into()];
    actor.activate(Arc::new(new)).unwrap();
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 110).0,
        [("b".into(), KeyEdge::Up), ("d".into(), KeyEdge::Down)]
    );
    let cleanup = actor
        .observe_device(device(1), true, MonoTime(120))
        .unwrap();
    assert_eq!(
        cleanup
            .outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("d"), KeyEdge::Up)]
    );
    assert_eq!(actor.reserved_sources(), 0);
    assert!(actor.poll(MonoTime(130)).unwrap().is_none());
}
#[test]
fn after_up_queue_rejection_after_main_up_commit_faults_without_reselecting_and_reset_recovers_real_keys()
 {
    let mut cfg = (*after_config(
        vec![key("b").into()],
        vec![
            Action {
                hold_down_ns: 1,
                ..key("d").into()
            };
            512
        ],
    ))
    .clone();
    cfg.complex.push(mapping(
        "x",
        vec![
            Action {
                repeat: false,
                hold_down_ns: 1,
                ..key("c").into()
            };
            512
        ],
    ));
    let mut actor = actor(Arc::new(cfg));
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    let slot = down(&first, 0, 100).event.payload.slot;
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    for i in 0..8 {
        let plan = input(&mut actor, &[key("x").into()], 1, 110);
        actor
            .admit(&down(&plan, 0, 110), &ContextSnapshot::default())
            .unwrap();
        if i == 0 {
            drain(&mut actor, 110);
        }
    }
    assert!(
        actor
            .admit(&up(&first, 0, 110), &ContextSnapshot::default())
            .is_err()
    );
    assert!(actor.is_failed());
    assert_eq!(
        actor.producer(slot).unwrap().up_delivered_at(),
        Some(MonoTime(110))
    );
    assert!(matches!(
        actor.poll(MonoTime(5000)),
        Err(ComplexActorError::Failed)
    ));
    let cleanup = actor.reset(MonoTime(110)).unwrap();
    assert_eq!(
        cleanup
            .outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("b"), KeyEdge::Up), (key("c"), KeyEdge::Up)]
    );
    assert_eq!(actor.reserved_sources(), 0);
    assert!(!actor.is_failed());
}
#[test]
fn reset_before_future_after_up_down_releases_only_real_main_and_discards_unexecuted_branch() {
    let cfg = after_config(
        vec![
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            key("c").into(),
        ],
        vec![Action {
            hold_down_ns: 20,
            ..key("d").into()
        }],
    );
    let mut actor = actor(cfg);
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Down)]);
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    let cleanup = actor.reset(MonoTime(120)).unwrap();
    assert_eq!(
        cleanup
            .outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("b"), KeyEdge::Up)]
    );
    assert_eq!(
        cleanup.discarded_input_batches,
        [plan.events()[0].payload.slot.batch]
    );
    assert_eq!(actor.reserved_sources(), 0);
    assert!(actor.poll(MonoTime(170)).unwrap().is_none());
}
#[test]
fn filtered_empty_after_up_needs_no_batch_id_even_when_identity_counter_is_exhausted() {
    let mut actor = actor(after_config(
        vec![],
        vec![Action {
            conditions: vec![Condition::Variable {
                name: "v".into(),
                value: VariableValue::Integer(1),
                unless: false,
            }],
            ..key("d").into()
        }],
    ));
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    let empty = plan_actions(
        &[],
        PlanOrigin {
            batch: BatchId(u64::MAX),
            stage: Stage::Complex,
            start: MonoTime(100),
            device: device(1),
            source: SourceToken(1),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    actor.register_input(empty).unwrap();
    let step = actor
        .admit(&up(&first, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(step.released_producers, [first.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
    assert!(!actor.is_failed());
    assert!(actor.poll(MonoTime(110)).unwrap().is_none());
}
fn alone_config(
    main: Vec<Action>,
    alone: Vec<Action>,
    after: Vec<Action>,
    timeout: u64,
) -> Arc<ConfigSnapshot> {
    let mut cfg = (*after_config(main, after)).clone();
    cfg.complex[0].extras.if_alone = alone;
    cfg.complex[0].extras.alone_timeout_ns = timeout;
    Arc::new(cfg)
}
#[test]
fn alone_timeout_is_strict_and_alone_tap_precedes_after_up_with_parent_completion_barrier() {
    for (at, eligible) in [(199, true), (200, false), (201, false)] {
        let mut actor = actor(alone_config(
            vec![],
            vec![Action {
                hold_down_ns: 20,
                ..key("c").into()
            }],
            vec![key("d").into()],
            100,
        ));
        let plan = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
            .unwrap();
        actor
            .admit(&up(&plan, 0, at), &ContextSnapshot::default())
            .unwrap();
        let (first, _) = drain(&mut actor, at);
        if eligible {
            assert_eq!(first, [("c".into(), KeyEdge::Down)]);
            assert_eq!(actor.pending_inputs(), 1);
            assert_eq!(
                drain(&mut actor, at + 20).0,
                [
                    ("c".into(), KeyEdge::Up),
                    ("d".into(), KeyEdge::Down),
                    ("d".into(), KeyEdge::Up)
                ]
            );
        } else {
            assert_eq!(
                first,
                [("d".into(), KeyEdge::Down), ("d".into(), KeyEdge::Up)]
            );
        }
        assert_eq!(actor.reserved_sources(), 0);
    }
}
#[test]
fn another_device_button_down_cancels_alone_and_its_up_does_not_restore_old_eligibility() {
    let mut actor = actor(alone_config(vec![], vec![key("c").into()], vec![], 1000));
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    let button = input(&mut actor, &[KeyId::new(BUTTON_PAGE, 1).into()], 2, 110);
    actor
        .admit(&down(&button, 0, 110), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 110);
    actor
        .admit(&up(&button, 0, 115), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 115);
    actor
        .admit(&up(&first, 0, 120), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 120).0.is_empty());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn nonzero_wheel_either_axis_cancels_alone_but_zero_wheel_does_not() {
    for (h, v, eligible) in [(0, 0, true), (1, 0, false), (0, -1, false)] {
        let mut actor = actor(alone_config(vec![], vec![key("c").into()], vec![], 1000));
        let plan = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
            .unwrap();
        actor.observe_wheel(h, v, MonoTime(110)).unwrap();
        actor
            .admit(&up(&plan, 0, 120), &ContextSnapshot::default())
            .unwrap();
        let outputs = drain(&mut actor, 120).0;
        assert_eq!(!outputs.is_empty(), eligible);
        assert_eq!(actor.reserved_sources(), 0);
    }
}
#[test]
fn variable_input_does_not_cancel_alone_and_branch_reads_its_generated_value_fresh() {
    let condition = Condition::Variable {
        name: "v".into(),
        value: VariableValue::Integer(1),
        unless: false,
    };
    let mut actor = actor(alone_config(
        vec![],
        vec![Action {
            conditions: vec![condition],
            ..key("c").into()
        }],
        vec![],
        1000,
    ));
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    let variable = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: None,
            },
        }),
        ..key("vk_none").into()
    };
    let other = input(&mut actor, &[variable], 1, 110);
    actor
        .admit(&down(&other, 0, 110), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 110);
    actor
        .admit(&up(&first, 0, 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 120).0,
        [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
    actor
        .admit(&up(&other, 0, 130), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 130);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn alone_recreates_down_snapshot_after_original_modifiers_up_and_charges_extra_tree_sources() {
    let mut cfg = (*alone_config(
        vec![],
        vec![Action {
            hold_down_ns: 20,
            ..key("c").into()
        }],
        vec![],
        1000,
    ))
    .clone();
    cfg.complex[0]
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    let mut actor = actor(Arc::new(cfg));
    let shift = input(&mut actor, &[key("left_shift").into()], 1, 100);
    actor
        .admit(&down(&shift, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let ctrl = input(&mut actor, &[key("right_control").into()], 1, 100);
    actor
        .admit(&down(&ctrl, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&shift, 0, 110), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 110);
    actor
        .admit(&up(&ctrl, 0, 111), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 111);
    assert_eq!(actor.reserved_sources(), 1);
    actor
        .admit(&up(&first, 0, 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.reserved_sources(), 4);
    assert_eq!(
        drain(&mut actor, 120).0,
        [
            ("right_control".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(
        drain(&mut actor, 140).0,
        [
            ("c".into(), KeyEdge::Up),
            ("right_control".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn alone_generated_variable_affects_after_up_selection_and_alone_halt_suppresses_it() {
    for halt in [false, true] {
        let variable = Action {
            halt,
            target: ActionTarget::SetVariable(VariableAction {
                name: "v".into(),
                operation: VariableOperation::Set {
                    value: Some(VariableValue::Integer(1)),
                    key_up_value: Some(VariableValue::Integer(2)),
                },
            }),
            ..key("vk_none").into()
        };
        let after = Action {
            conditions: vec![Condition::Variable {
                name: "v".into(),
                value: VariableValue::Integer(2),
                unless: false,
            }],
            ..key("d").into()
        };
        let mut actor = actor(alone_config(vec![], vec![variable], vec![after], 1000));
        let plan = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
            .unwrap();
        actor
            .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(
            actor.generated_variables().get("v"),
            Some(&VariableValue::Integer(2))
        );
        let outputs = drain(&mut actor, 110).0;
        if halt {
            assert!(outputs.is_empty());
        } else {
            assert_eq!(
                outputs,
                [("d".into(), KeyEdge::Down), ("d".into(), KeyEdge::Up)]
            );
        }
        assert_eq!(actor.reserved_sources(), 0);
    }
}
#[test]
fn alone_uses_retained_rule_after_reload_and_reset_does_not_fire_pending_alone() {
    let old = alone_config(vec![], vec![key("c").into()], vec![], 1000);
    let mut actor = actor(old.clone());
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    let mut new = (*old).clone();
    new.generation = ConfigGeneration(2);
    new.complex[0].extras.if_alone = vec![key("d").into()];
    actor.activate(Arc::new(new)).unwrap();
    actor
        .admit(&up(&first, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 110).0,
        [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
    let next = input(&mut actor, &[key("a").into()], 1, 120);
    actor
        .admit(&down(&next, 0, 120), &ContextSnapshot::default())
        .unwrap();
    let cleanup = actor.reset(MonoTime(130)).unwrap();
    assert!(cleanup.outputs.is_empty());
    assert!(actor.poll(MonoTime(140)).unwrap().is_none());
    assert_eq!(actor.reserved_sources(), 0);
}
fn held_config(main: Vec<Action>, held: Vec<Action>, threshold: u64) -> Arc<ConfigSnapshot> {
    let mut cfg = (*config(main)).clone();
    cfg.complex[0].extras.if_held_down = held;
    cfg.complex[0].extras.held_threshold_ns = threshold;
    Arc::new(cfg)
}
fn drain_context(
    actor: &mut ComplexStageActor,
    at: u64,
    context: &ContextSnapshot,
) -> Vec<(String, KeyEdge)> {
    let mut outputs = vec![];
    while let Some(step) = actor.poll_with_context(MonoTime(at), context).unwrap() {
        outputs.extend(step.outputs.into_iter().map(|o| (o.key.name(), o.edge)));
    }
    outputs
}
#[test]
fn held_deadline_replaces_main_once_then_original_up_releases_new_saved_child() {
    let mut actor = actor(held_config(
        vec![key("b").into()],
        vec![key("c").into()],
        50,
    ));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [("b".into(), KeyEdge::Down)]);
    assert_eq!(actor.next_deadline(), Some(MonoTime(150)));
    assert!(actor.poll(MonoTime(149)).unwrap().is_none());
    assert_eq!(
        drain(&mut actor, 150).0,
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
    assert_eq!(actor.next_deadline(), None);
    actor
        .admit(&up(&plan, 0, 180), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 180).0, [("c".into(), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn up_before_held_deadline_cancels_but_equal_deadline_requires_timer_poll_first() {
    for at in [149, 150] {
        let mut actor = actor(held_config(
            vec![key("b").into()],
            vec![key("c").into()],
            50,
        ));
        let plan = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 100);
        let receipt = up(&plan, 0, at);
        if at == 150 {
            assert!(matches!(
                actor.admit(&receipt, &ContextSnapshot::default()),
                Err(ComplexActorError::DeadlinePending)
            ));
            assert_eq!(
                drain(&mut actor, at).0,
                [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
            );
        }
        actor.admit(&receipt, &ContextSnapshot::default()).unwrap();
        assert_eq!(
            drain(&mut actor, at).0,
            [(if at == 149 { "b".into() } else { "c".into() }, KeyEdge::Up)]
        );
        assert!(actor.poll(MonoTime(200)).unwrap().is_none());
        assert_eq!(actor.reserved_sources(), 0);
    }
}
#[test]
fn any_new_down_including_variable_cancels_pending_held_task() {
    let condition = Condition::Variable {
        name: "v".into(),
        value: VariableValue::Integer(1),
        unless: false,
    };
    let mut actor = actor(held_config(
        vec![key("b").into()],
        vec![Action {
            conditions: vec![condition],
            ..key("c").into()
        }],
        50,
    ));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let variable = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: None,
            },
        }),
        ..key("vk_none").into()
    };
    let other = input(&mut actor, &[variable], 1, 110);
    actor
        .admit(&down(&other, 0, 110), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 110);
    assert_eq!(actor.next_deadline(), None);
    assert!(actor.poll(MonoTime(150)).unwrap().is_none());
    actor
        .admit(&up(&plan, 0, 180), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 180).0, [("b".into(), KeyEdge::Up)]);
    actor.reset(MonoTime(180)).unwrap();
}
#[test]
fn held_filters_after_main_variable_up_and_does_not_clear_alone_eligibility() {
    let var = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        ..key("vk_none").into()
    };
    let mut cfg = (*held_config(
        vec![var],
        vec![Action {
            conditions: vec![Condition::Variable {
                name: "v".into(),
                value: VariableValue::Integer(2),
                unless: false,
            }],
            ..key("c").into()
        }],
        50,
    ))
    .clone();
    cfg.complex[0].extras.if_alone = vec![key("d").into()];
    let mut actor = actor(Arc::new(cfg));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    assert_eq!(drain(&mut actor, 150).0, [("c".into(), KeyEdge::Down)]);
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    actor
        .admit(&up(&plan, 0, 180), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 180).0,
        [
            ("c".into(), KeyEdge::Up),
            ("d".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn held_generation_context_and_fifo_preserve_old_future_array_before_new_early_up() {
    let condition = Condition::Application {
        bundle_identifiers: vec![],
        file_paths: vec![Pattern::compile("editor").unwrap()],
        unless: false,
    };
    let mut actor = actor(held_config(
        vec![
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            key("c").into(),
        ],
        vec![Action {
            conditions: vec![condition],
            ..key("d").into()
        }],
        20,
    ));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let context = ContextSnapshot {
        revision: 1,
        provider: "test".into(),
        observed_at_ns: MonoTime(100),
        valid_until_ns: MonoTime(130),
        application: Some(ApplicationContext {
            bundle_identifier: None,
            file_path: Some("editor".into()),
        }),
        ..Default::default()
    };
    assert!(drain_context(&mut actor, 120, &context).is_empty());
    actor
        .admit(&up(&plan, 0, 140), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 150).0,
        [
            ("b".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Up),
            ("d".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn held_retains_old_rule_after_reload_and_device_reset_cancels_timer_without_fake_child_up() {
    let old = held_config(vec![key("b").into()], vec![key("c").into()], 50);
    let mut actor = actor(old.clone());
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let mut new = (*old).clone();
    new.generation = ConfigGeneration(2);
    new.complex[0].extras.if_held_down = vec![key("d").into()];
    actor.activate(Arc::new(new)).unwrap();
    assert_eq!(
        drain(&mut actor, 150).0,
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
    let cleanup = actor
        .observe_device(device(1), true, MonoTime(160))
        .unwrap();
    assert_eq!(
        cleanup
            .outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("c"), KeyEdge::Up)]
    );
    assert!(actor.poll(MonoTime(200)).unwrap().is_none());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn held_deadline_overflow_faults_before_parent_activation_or_output_generation() {
    let mut actor = actor(held_config(
        vec![key("b").into()],
        vec![key("c").into()],
        u64::MAX,
    ));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let receipt = down(&plan, 0, 100);
    assert!(matches!(
        actor.admit(&receipt, &ContextSnapshot::default()),
        Err(ComplexActorError::TimeOverflow)
    ));
    assert!(actor.is_failed());
    assert!(actor.producer(receipt.event.payload.slot).is_none());
    assert!(actor.reset(MonoTime(100)).unwrap().outputs.is_empty());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn empty_or_halt_skipped_held_releases_old_main_without_new_identity_or_child() {
    for halt in [false, true] {
        let held = Action {
            conditions: if halt {
                vec![]
            } else {
                vec![Condition::Variable {
                    name: "v".into(),
                    value: VariableValue::Integer(1),
                    unless: false,
                }]
            },
            ..key("c").into()
        };
        let mut actor = actor(held_config(
            vec![Action {
                halt,
                ..key("b").into()
            }],
            vec![held],
            50,
        ));
        let plan = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 100);
        let empty = plan_actions(
            &[],
            PlanOrigin {
                batch: BatchId(u64::MAX),
                stage: Stage::Complex,
                start: MonoTime(100),
                device: device(1),
                source: SourceToken(1),
                config: actor.snapshot().clone(),
            },
            false,
        )
        .unwrap();
        actor.register_input(empty).unwrap();
        assert_eq!(drain(&mut actor, 150).0, [("b".into(), KeyEdge::Up)]);
        assert_eq!(actor.reserved_sources(), 1);
        actor
            .admit(&up(&plan, 0, 180), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.reserved_sources(), 0);
        assert!(!actor.is_failed());
    }
}
#[test]
fn held_modifier_scope_replaces_old_scope_and_restores_consumed_owner_on_original_up() {
    let mut cfg = (*held_config(
        vec![key("right_control").into()],
        vec![key("right_alt").into()],
        50,
    ))
    .clone();
    cfg.complex[0]
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    let mut actor = actor(Arc::new(cfg));
    let shift = input(&mut actor, &[key("left_shift").into()], 1, 100);
    actor
        .admit(&down(&shift, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let main = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&main, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 100).0,
        [
            ("left_shift".into(), KeyEdge::Up),
            ("right_control".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(
        drain(&mut actor, 150).0,
        [
            ("right_control".into(), KeyEdge::Up),
            ("right_alt".into(), KeyEdge::Down)
        ]
    );
    assert!(!actor.generated_modifiers().contains(&key("left_shift")));
    actor
        .admit(&up(&main, 0, 180), &ContextSnapshot::default())
        .unwrap();
    assert!(actor.generated_modifiers().contains(&key("left_shift")));
    assert_eq!(
        drain(&mut actor, 180).0,
        [("right_alt".into(), KeyEdge::Up)]
    );
    let next = input(&mut actor, &[key("z").into()], 1, 200);
    actor
        .admit(&down(&next, 0, 200), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 200).0,
        [
            ("left_shift".into(), KeyEdge::Down),
            ("z".into(), KeyEdge::Down)
        ]
    );
    actor.reset(MonoTime(210)).unwrap();
    assert_eq!(actor.reserved_sources(), 0);
}
fn delayed_config(
    main: Vec<Action>,
    invoked: Vec<Action>,
    canceled: Vec<Action>,
    delay: u64,
) -> Arc<ConfigSnapshot> {
    let mut cfg = (*config(main)).clone();
    cfg.complex[0].extras.delayed = Some(DelayedAction {
        if_invoked: invoked,
        if_canceled: canceled,
        delay_ns: delay,
    });
    Arc::new(cfg)
}
#[test]
fn delayed_survives_original_up_and_parent_waits_invoked_tap_completion() {
    let mut actor = actor(delayed_config(
        vec![key("b").into()],
        vec![Action {
            hold_down_ns: 20,
            ..key("c").into()
        }],
        vec![key("d").into()],
        50,
    ));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 110).0, [("b".into(), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 1);
    assert_eq!(actor.pending_inputs(), 1);
    assert!(actor.poll(MonoTime(149)).unwrap().is_none());
    assert_eq!(drain(&mut actor, 150).0, [("c".into(), KeyEdge::Down)]);
    assert_eq!(drain(&mut actor, 170).0, [("c".into(), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
    assert_eq!(actor.pending_inputs(), 0);
}
#[test]
fn canceled_generation_changes_new_down_selection_before_new_producer_freezes() {
    let variable = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: None,
            },
        }),
        ..key("vk_none").into()
    };
    let mut cfg = (*delayed_config(vec![], vec![key("d").into()], vec![variable], 50)).clone();
    cfg.complex.push(mapping(
        "x",
        vec![Action {
            conditions: vec![Condition::Variable {
                name: "v".into(),
                value: VariableValue::Integer(1),
                unless: false,
            }],
            ..key("c").into()
        }],
    ));
    let mut actor = actor(Arc::new(cfg));
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&first, 0, 105), &ContextSnapshot::default())
        .unwrap();
    let next = input(&mut actor, &[key("x").into()], 2, 110);
    actor
        .admit(&down(&next, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    let (outputs, released) = drain(&mut actor, 110);
    assert_eq!(outputs, [("c".into(), KeyEdge::Down)]);
    assert_eq!(released, [first.events()[0].payload.slot]);
    assert_eq!(actor.next_deadline(), None);
    actor
        .admit(&up(&next, 0, 120), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 120);
    assert_eq!(actor.reserved_sources(), 0);
    assert!(actor.poll(MonoTime(150)).unwrap().is_none());
}
#[test]
fn delayed_context_is_fresh_at_poll_and_old_rule_survives_reload_after_up() {
    let condition = Condition::Application {
        bundle_identifiers: vec![],
        file_paths: vec![Pattern::compile("editor").unwrap()],
        unless: false,
    };
    let old = delayed_config(
        vec![],
        vec![Action {
            conditions: vec![condition],
            ..key("c").into()
        }],
        vec![],
        50,
    );
    let mut actor = actor(old.clone());
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    let mut new = (*old).clone();
    new.generation = ConfigGeneration(2);
    new.complex[0].extras.delayed.as_mut().unwrap().if_invoked = vec![key("d").into()];
    actor.activate(Arc::new(new)).unwrap();
    let context = ContextSnapshot {
        revision: 1,
        provider: "test".into(),
        observed_at_ns: MonoTime(140),
        valid_until_ns: MonoTime(160),
        application: Some(ApplicationContext {
            bundle_identifier: None,
            file_path: Some("editor".into()),
        }),
        ..Default::default()
    };
    assert_eq!(
        drain_context(&mut actor, 150, &context),
        [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn same_deadline_held_generation_precedes_delayed_and_generation_halt_skips_invoked() {
    for halt in [false, true] {
        let variable = Action {
            halt,
            target: ActionTarget::SetVariable(VariableAction {
                name: "v".into(),
                operation: VariableOperation::Set {
                    value: Some(VariableValue::Integer(2)),
                    key_up_value: None,
                },
            }),
            ..key("vk_none").into()
        };
        let mut cfg = (*delayed_config(
            vec![key("b").into()],
            vec![Action {
                conditions: vec![Condition::Variable {
                    name: "v".into(),
                    value: VariableValue::Integer(2),
                    unless: false,
                }],
                ..key("c").into()
            }],
            vec![],
            50,
        ))
        .clone();
        cfg.complex[0].extras.if_held_down = vec![variable];
        cfg.complex[0].extras.held_threshold_ns = 50;
        let mut actor = actor(Arc::new(cfg));
        let plan = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 100);
        let outputs = drain(&mut actor, 150).0;
        if halt {
            assert_eq!(outputs, [("b".into(), KeyEdge::Up)]);
        } else {
            assert_eq!(
                outputs,
                [
                    ("b".into(), KeyEdge::Up),
                    ("c".into(), KeyEdge::Down),
                    ("c".into(), KeyEdge::Up)
                ]
            );
        }
        actor
            .admit(&up(&plan, 0, 180), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 180);
        assert_eq!(actor.reserved_sources(), 0);
    }
}
#[test]
fn delayed_filtered_empty_after_up_finishes_without_identity_allocation_at_counter_max() {
    let invoked = Action {
        conditions: vec![Condition::Variable {
            name: "v".into(),
            value: VariableValue::Integer(1),
            unless: false,
        }],
        ..key("c").into()
    };
    let mut actor = actor(delayed_config(vec![], vec![invoked], vec![], 50));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    let empty = plan_actions(
        &[],
        PlanOrigin {
            batch: BatchId(u64::MAX),
            stage: Stage::Complex,
            start: MonoTime(110),
            device: device(1),
            source: SourceToken(1),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    actor.register_input(empty).unwrap();
    let (outputs, released) = drain(&mut actor, 150);
    assert!(outputs.is_empty());
    assert_eq!(released, [plan.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
    assert!(!actor.is_failed());
}
#[test]
fn device_cleanup_after_up_discards_delayed_without_firing_canceled_or_invoked() {
    for reset in [false, true] {
        let mut actor = actor(delayed_config(
            vec![],
            vec![key("c").into()],
            vec![key("d").into()],
            50,
        ));
        let plan = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
            .unwrap();
        actor
            .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
            .unwrap();
        let cleanup = if reset {
            actor.reset(MonoTime(120)).unwrap()
        } else {
            actor
                .observe_device(device(1), true, MonoTime(120))
                .unwrap()
        };
        assert!(cleanup.outputs.is_empty());
        assert_eq!(actor.reserved_sources(), 0);
        assert_eq!(actor.next_deadline(), None);
        assert!(actor.poll(MonoTime(150)).unwrap().is_none());
    }
}
#[test]
fn delayed_deadline_overflow_faults_before_new_producer_or_output_commit() {
    let mut actor = actor(delayed_config(
        vec![key("b").into()],
        vec![key("c").into()],
        vec![],
        u64::MAX,
    ));
    let plan = input(&mut actor, &[key("a").into()], 1, 100);
    let receipt = down(&plan, 0, 100);
    assert!(matches!(
        actor.admit(&receipt, &ContextSnapshot::default()),
        Err(ComplexActorError::TimeOverflow)
    ));
    assert!(actor.producer(receipt.event.payload.slot).is_none());
    assert!(actor.reset(MonoTime(100)).unwrap().outputs.is_empty());
}
#[test]
fn delayed_snapshot_survives_modifier_release_and_original_up_with_extra_source_budget() {
    let mut cfg = (*delayed_config(
        vec![],
        vec![Action {
            hold_down_ns: 20,
            ..key("c").into()
        }],
        vec![],
        50,
    ))
    .clone();
    cfg.complex[0]
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    let mut actor = actor(Arc::new(cfg));
    let shift = input(&mut actor, &[key("left_shift").into()], 1, 100);
    actor
        .admit(&down(&shift, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let ctrl = input(&mut actor, &[key("right_control").into()], 1, 100);
    actor
        .admit(&down(&ctrl, 0, 100), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let first = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&first, 0, 100), &ContextSnapshot::default())
        .unwrap();
    actor
        .admit(&up(&shift, 0, 110), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 110);
    actor
        .admit(&up(&ctrl, 0, 111), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 111);
    actor
        .admit(&up(&first, 0, 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.reserved_sources(), 1);
    assert_eq!(
        drain(&mut actor, 150).0,
        [
            ("right_control".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.reserved_sources(), 4);
    assert_eq!(
        drain(&mut actor, 170).0,
        [
            ("c".into(), KeyEdge::Up),
            ("right_control".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn variable_down_cancels_delayed_but_nonzero_wheel_does_not() {
    for variable_cancel in [false, true] {
        let var = Action {
            target: ActionTarget::SetVariable(VariableAction {
                name: "v".into(),
                operation: VariableOperation::Set {
                    value: Some(VariableValue::Integer(1)),
                    key_up_value: None,
                },
            }),
            ..key("vk_none").into()
        };
        let mut actor = actor(delayed_config(
            vec![],
            vec![key("c").into()],
            vec![var.clone(), key("d").into()],
            50,
        ));
        let first = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&first, 0, 100), &ContextSnapshot::default())
            .unwrap();
        actor
            .admit(&up(&first, 0, 105), &ContextSnapshot::default())
            .unwrap();
        if variable_cancel {
            let other = input(&mut actor, &[var], 1, 110);
            actor
                .admit(&down(&other, 0, 110), &ContextSnapshot::default())
                .unwrap();
            assert_eq!(
                drain(&mut actor, 110).0,
                [("d".into(), KeyEdge::Down), ("d".into(), KeyEdge::Up)]
            );
            assert!(actor.poll(MonoTime(150)).unwrap().is_none());
            actor.reset(MonoTime(150)).unwrap();
        } else {
            actor.observe_wheel(1, 0, MonoTime(110)).unwrap();
            assert_eq!(
                drain(&mut actor, 150).0,
                [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
            );
        }
        assert_eq!(actor.reserved_sources(), 0);
    }
}
#[test]
fn unknown_removed_stale_and_future_devices_cannot_register_empty_or_nonempty_input() {
    for empty in [false, true] {
        let mut actor = actor(config(vec![]));
        actor.observe_device(device(1), true, MonoTime(0)).unwrap();
        let id = actor.next_batch_id().unwrap();
        for source_device in [
            device(1),
            DeviceInstanceId {
                id: 1,
                generation: 0,
            },
            DeviceInstanceId {
                id: 1,
                generation: 2,
            },
            device(9),
        ] {
            let actions = if empty { vec![] } else { vec![key("a").into()] };
            let plan = plan_actions(
                &actions,
                PlanOrigin {
                    batch: id,
                    stage: Stage::Complex,
                    start: MonoTime(100),
                    device: source_device,
                    source: SourceToken(1),
                    config: actor.snapshot().clone(),
                },
                false,
            )
            .unwrap();
            assert!(matches!(
                actor.register_input(plan),
                Err(ComplexActorError::Output(
                    keyweave_engine::output_stage::OutputStageError::Generated(
                        keyweave_engine::generated_stage::GeneratedError::Timeline(
                            TimelineError::RetiredDevice
                        )
                    )
                ))
            ));
            assert_eq!(actor.next_batch_id().unwrap(), id);
            assert_eq!(actor.reserved_sources(), 0);
            assert_eq!(actor.pending_inputs(), 0);
            assert!(!actor.is_failed());
        }
    }
}
#[test]
fn rejected_empty_input_does_not_consume_identity_and_observed_next_generation_can_consume_empty_main()
 {
    let mut actor = actor(config(vec![]));
    actor.observe_device(device(1), true, MonoTime(0)).unwrap();
    let next_device = DeviceInstanceId {
        id: 1,
        generation: 2,
    };
    let id = actor.next_batch_id().unwrap();
    let make = |actor: &ComplexStageActor| {
        plan_actions(
            &[key("a").into()],
            PlanOrigin {
                batch: id,
                stage: Stage::Complex,
                start: MonoTime(100),
                device: next_device,
                source: SourceToken(1),
                config: actor.snapshot().clone(),
            },
            false,
        )
        .unwrap()
    };
    assert!(actor.register_input(make(&actor)).is_err());
    actor
        .observe_device(next_device, false, MonoTime(0))
        .unwrap();
    let plan = make(&actor);
    actor.register_input(plan.clone()).unwrap();
    actor
        .admit(&down(&plan, 0, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.reserved_sources(), 1);
    let step = actor
        .admit(&up(&plan, 0, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(step.released_producers, [plan.events()[0].payload.slot]);
    assert!(actor.poll(MonoTime(110)).unwrap().is_none());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn reset_keeps_removed_device_tombstone_and_requires_explicit_new_generation_observation() {
    let mut actor = actor(config(vec![]));
    actor.observe_device(device(1), true, MonoTime(10)).unwrap();
    actor.reset(MonoTime(20)).unwrap();
    let id = actor.next_batch_id().unwrap();
    let empty = plan_actions(
        &[],
        PlanOrigin {
            batch: id,
            stage: Stage::Complex,
            start: MonoTime(100),
            device: device(1),
            source: SourceToken(1),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    assert!(actor.register_input(empty).is_err());
    assert_eq!(actor.next_batch_id().unwrap(), id);
    assert!(
        actor
            .observe_device(device(1), false, MonoTime(20))
            .is_err()
    );
    actor.reset(MonoTime(20)).unwrap();
    let next = DeviceInstanceId {
        id: 1,
        generation: 2,
    };
    actor.observe_device(next, false, MonoTime(20)).unwrap();
    let empty = plan_actions(
        &[],
        PlanOrigin {
            batch: id,
            stage: Stage::Complex,
            start: MonoTime(100),
            device: next,
            source: SourceToken(1),
            config: actor.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    actor.register_input(empty).unwrap();
    assert_eq!(actor.next_batch_id().unwrap().0, id.0 + 1);
    assert_eq!(actor.pending_inputs(), 0);
}
