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
