#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, simple_actor::*, timeline::*, *};
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
fn config(simple: Vec<Mapping>, complex: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "owned-simple".into(),
        simple,
        complex,
    })
}
fn actor(config: Arc<ConfigSnapshot>) -> SimpleStageActor {
    let mut actor = SimpleStageActor::new(config).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor.observe_device(device(2), false, MonoTime(0)).unwrap();
    actor
}
fn input(actor: &mut SimpleStageActor, actions: &[Action], device_id: u64, at: u64) -> ActionPlan {
    let id = actor.next_batch_id().unwrap();
    let plan = plan_actions(
        actions,
        PlanOrigin {
            batch: id,
            stage: Stage::Simple,
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
fn down(plan: &ActionPlan, at: u64) -> DueEvent<PlannedAction> {
    DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(at),
    }
}
fn up(plan: &ActionPlan, at: u64) -> DueEvent<PlannedAction> {
    DueEvent {
        event: plan
            .input_up_events(MonoTime(at))
            .into_iter()
            .next()
            .unwrap(),
        delivered_at: MonoTime(at),
    }
}
fn drain(actor: &mut SimpleStageActor, at: u64) -> (Vec<(KeyId, KeyEdge)>, Vec<ActionSlot>) {
    let mut outputs = vec![];
    let mut released = vec![];
    for _ in 0..10000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return (outputs, released);
        };
        outputs.extend(step.outputs.into_iter().map(|o| (o.key, o.edge)));
        released.extend(step.released_producers);
    }
    panic!("poll failed to finish bounded fixture");
}
fn edges(outputs: Vec<OutputIntent>) -> Vec<(KeyId, KeyEdge)> {
    outputs.into_iter().map(|o| (o.key, o.edge)).collect()
}
#[test]
fn owns_all_three_stage_main_and_saved_up_across_reload_without_retiring_parent_early() {
    let cfg = config(
        vec![mapping("a", vec![key("b").into()])],
        vec![mapping("b", vec![key("c").into()])],
    );
    let mut actor = actor(cfg.clone());
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&root, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.pending_children(), 1);
    assert_eq!(drain(&mut actor, 100).0, vec![(key("c"), KeyEdge::Down)]);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert!(Arc::ptr_eq(
        &actor
            .producer(root.events()[0].payload.slot)
            .unwrap()
            .down()
            .config,
        &cfg
    ));
    let step = actor
        .admit(&up(&root, 150), &ContextSnapshot::default())
        .unwrap();
    assert!(step.released_producers.is_empty());
    assert_eq!(actor.pending_inputs(), 1);
    let (outputs, released) = drain(&mut actor, 150);
    assert_eq!(outputs, vec![(key("c"), KeyEdge::Up)]);
    assert_eq!(released, vec![root.events()[0].payload.slot]);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
    let new = input(&mut actor, &[key("a").into()], 1, 200);
    actor
        .admit(&down(&new, 200), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 200).0, vec![(key("a"), KeyEdge::Down)]);
}
#[test]
fn reserved_old_simple_down_after_reload_selects_old_rules_in_both_destinations() {
    let mut actor = actor(config(
        vec![mapping("a", vec![key("b").into()])],
        vec![mapping("b", vec![key("c").into()])],
    ));
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor
        .admit(&down(&root, 125), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 125).0, vec![(key("c"), KeyEdge::Down)]);
    actor
        .admit(&up(&root, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 150).0, vec![(key("c"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn early_original_up_preserves_future_simple_macro_duration_and_actual_child_barrier() {
    let mut actor = actor(config(
        vec![mapping(
            "a",
            vec![
                Action {
                    repeat: false,
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("d").into(),
            ],
        )],
        vec![mapping("b", vec![key("c").into()])],
    ));
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&root, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, vec![(key("c"), KeyEdge::Down)]);
    actor
        .admit(&up(&root, 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.pending_inputs(), 1);
    assert!(drain(&mut actor, 149).0.is_empty());
    let (outputs, released) = drain(&mut actor, 150);
    assert_eq!(
        outputs,
        vec![
            (key("c"), KeyEdge::Up),
            (key("d"), KeyEdge::Down),
            (key("d"), KeyEdge::Up)
        ]
    );
    assert_eq!(released, vec![root.events()[0].payload.slot]);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn complex_delayed_after_original_simple_up_keeps_parent_until_actual_tap_ends() {
    let mut complex = mapping("b", vec![]);
    complex.extras.delayed = Some(DelayedAction {
        delay_ns: 50,
        if_invoked: vec![Action {
            hold_down_ns: 10,
            ..key("h").into()
        }],
        if_canceled: vec![],
    });
    let mut actor = actor(config(
        vec![mapping("a", vec![key("b").into()])],
        vec![complex],
    ));
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&root, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    actor
        .admit(&up(&root, 120), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 120).0.is_empty());
    assert_eq!(actor.pending_inputs(), 1);
    assert_eq!(actor.next_deadline(), Some(MonoTime(150)));
    assert_eq!(drain(&mut actor, 150).0, vec![(key("h"), KeyEdge::Down)]);
    assert_eq!(actor.pending_inputs(), 1);
    let (outputs, released) = drain(&mut actor, 160);
    assert_eq!(outputs, vec![(key("h"), KeyEdge::Up)]);
    assert_eq!(released, vec![root.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn simple_generated_variable_up_and_empty_match_do_not_allocate_fake_output_binding() {
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
    let mut actor = actor(config(
        vec![mapping("a", vec![variable]), mapping("b", vec![])],
        vec![],
    ));
    let a = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&a, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    assert!(!actor.output_generated_variables().contains_key("v"));
    assert!(drain(&mut actor, 100).0.is_empty());
    assert_eq!(
        actor.output_generated_variables().get("v"),
        Some(&VariableValue::Integer(1))
    );
    actor
        .admit(&up(&a, 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        actor.generated_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
    assert!(drain(&mut actor, 120).0.is_empty());
    assert_eq!(actor.reserved_sources(), 0);
    let b = input(&mut actor, &[key("b").into()], 1, 150);
    actor
        .admit(&down(&b, 150), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 1);
    let step = actor
        .admit(&up(&b, 170), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(step.released_producers, vec![b.events()[0].payload.slot]);
    assert!(step.outputs.is_empty());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn future_simple_input_slots_defer_lazy_modifier_eof_until_real_next_key_down() {
    let mut actor = actor(config(vec![], vec![]));
    let root = input(
        &mut actor,
        &[
            Action {
                repeat: false,
                hold_down_ns: 50,
                modifiers: vec![key("left_shift")],
                ..key("right_control").into()
            },
            key("x").into(),
        ],
        1,
        100,
    );
    let mut outputs = vec![];
    for event in root.events() {
        let at = event.at_ns.0;
        let due = DueEvent {
            event: event.clone(),
            delivered_at: MonoTime(at),
        };
        outputs.extend(edges(
            actor
                .admit(&due, &ContextSnapshot::default())
                .unwrap()
                .outputs,
        ));
        outputs.extend(drain(&mut actor, at).0);
        if event.payload.edge == KeyEdge::Up
            && event.payload.target == ActionTarget::Key(key("left_shift"))
        {
            assert!(!outputs.contains(&(key("left_shift"), KeyEdge::Up)));
        }
    }
    assert_eq!(
        outputs,
        vec![
            (key("left_shift"), KeyEdge::Down),
            (key("right_control"), KeyEdge::Down),
            (key("right_control"), KeyEdge::Up),
            (key("left_shift"), KeyEdge::Up),
            (key("x"), KeyEdge::Down)
        ]
    );
    actor
        .admit(&up(&root, 200), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 200).0, vec![(key("x"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn remove_device_releases_only_actual_owned_outputs_and_reset_cancels_reserved_future_work() {
    let cfg = config(
        vec![mapping(
            "a",
            vec![
                Action {
                    repeat: false,
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("d").into(),
            ],
        )],
        vec![],
    );
    let mut actor = actor(cfg);
    let z = input(&mut actor, &[key("z").into()], 2, 90);
    actor
        .admit(&down(&z, 90), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 90).0, vec![(key("z"), KeyEdge::Down)]);
    let a = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&a, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, vec![(key("b"), KeyEdge::Down)]);
    let removed = actor
        .observe_device(device(1), true, MonoTime(120))
        .unwrap();
    assert_eq!(edges(removed.outputs), vec![(key("b"), KeyEdge::Up)]);
    assert_eq!(
        removed.discarded_input_batches,
        vec![a.events()[0].payload.slot.batch]
    );
    assert_eq!(actor.pending_inputs(), 1);
    let next = actor.next_batch_id().unwrap();
    let reset = actor.reset(MonoTime(130)).unwrap();
    assert_eq!(edges(reset.outputs), vec![(key("z"), KeyEdge::Up)]);
    assert_eq!(actor.next_batch_id().unwrap(), next);
    assert_eq!(actor.next_deadline(), None);
    assert!(drain(&mut actor, 200).0.is_empty());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn due_first_retries_without_selection_and_other_invalid_receipt_latches_failure_until_reset() {
    let mut actor = actor(config(vec![], vec![]));
    let a = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&a, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(matches!(
        actor.admit(&up(&a, 120), &ContextSnapshot::default()),
        Err(SimpleActorError::DeadlinePending)
    ));
    assert!(!actor.is_failed());
    assert_eq!(drain(&mut actor, 100).0, vec![(key("a"), KeyEdge::Down)]);
    let mut bad = up(&a, 120);
    bad.event.payload.target = ActionTarget::Key(key("b"));
    assert!(actor.admit(&bad, &ContextSnapshot::default()).is_err());
    assert!(actor.is_failed());
    assert!(matches!(
        actor.poll(MonoTime(120)),
        Err(SimpleActorError::Failed)
    ));
    assert_eq!(
        edges(actor.reset(MonoTime(120)).unwrap().outputs),
        vec![(key("a"), KeyEdge::Up)]
    );
    assert!(!actor.is_failed());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn unsupported_simple_branches_reject_but_consumed_modifiers_are_supported() {
    let mut mapping = mapping("a", vec![key("b").into()]);
    mapping.extras.if_alone.push(key("c").into());
    assert!(matches!(
        SimpleStageActor::new(config(vec![mapping], vec![])),
        Err(SimpleActorError::UnsupportedSimpleSemantics)
    ));
    let mut mapping = crate::mapping("a", vec![key("b").into()]);
    mapping
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::LeftShift);
    assert!(SimpleStageActor::new(config(vec![mapping], vec![])).is_ok());
}
#[test]
fn final_empty_simple_input_up_flushes_modifier_deferred_by_registered_physical_group() {
    let mut actor = actor(config(vec![mapping("x", vec![])], vec![]));
    let root = input(
        &mut actor,
        &[
            Action {
                repeat: false,
                hold_down_ns: 50,
                modifiers: vec![key("left_shift")],
                ..key("right_control").into()
            },
            key("x").into(),
        ],
        1,
        100,
    );
    let mut outputs = vec![];
    for event in root.events() {
        let due = DueEvent {
            event: event.clone(),
            delivered_at: event.at_ns,
        };
        outputs.extend(edges(
            actor
                .admit(&due, &ContextSnapshot::default())
                .unwrap()
                .outputs,
        ));
        outputs.extend(drain(&mut actor, event.at_ns.0).0);
    }
    assert_eq!(
        outputs,
        vec![
            (key("left_shift"), KeyEdge::Down),
            (key("right_control"), KeyEdge::Down),
            (key("right_control"), KeyEdge::Up)
        ]
    );
    let step = actor
        .admit(&up(&root, 200), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(edges(step.outputs), vec![(key("left_shift"), KeyEdge::Up)]);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.reserved_sources(), 0);
    assert!(drain(&mut actor, 200).0.is_empty());
}
#[test]
fn complex_held_deadline_at_original_simple_up_runs_before_up_then_releases_saved_replacement() {
    let mut complex = mapping("b", vec![key("c").into()]);
    complex.extras.held_threshold_ns = 50;
    complex.extras.if_held_down.push(key("h").into());
    let mut actor = actor(config(
        vec![mapping("a", vec![key("b").into()])],
        vec![complex],
    ));
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&root, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, vec![(key("c"), KeyEdge::Down)]);
    assert!(matches!(
        actor.admit(&up(&root, 150), &ContextSnapshot::default()),
        Err(SimpleActorError::DeadlinePending)
    ));
    assert!(!actor.is_failed());
    assert_eq!(
        drain(&mut actor, 150).0,
        vec![(key("c"), KeyEdge::Up), (key("h"), KeyEdge::Down)]
    );
    actor
        .admit(&up(&root, 150), &ContextSnapshot::default())
        .unwrap();
    let (outputs, released) = drain(&mut actor, 150);
    assert_eq!(outputs, vec![(key("h"), KeyEdge::Up)]);
    assert_eq!(released, vec![root.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn after_up_uses_old_simple_and_complex_rules_after_reload() {
    let mut simple = mapping("a", vec![key("b").into()]);
    simple.extras.after_key_up = vec![key("d").into()];
    let old = config(
        vec![simple],
        vec![
            mapping("b", vec![key("c").into()]),
            mapping("d", vec![key("e").into()]),
        ],
    );
    let mut actor = actor(old.clone());
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&root, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [(key("c"), KeyEdge::Down)]);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor
        .admit(&up(&root, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.pending_children(), 2);
    let (outputs, released) = drain(&mut actor, 110);
    assert_eq!(
        outputs,
        [
            (key("c"), KeyEdge::Up),
            (key("e"), KeyEdge::Down),
            (key("e"), KeyEdge::Up)
        ]
    );
    assert_eq!(released, [root.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
    assert!(Arc::ptr_eq(&root.events()[0].config, &old));
}

#[test]
fn empty_main_after_up_duration_retains_parent_until_actual_child_completes() {
    let mut simple = mapping("a", vec![]);
    simple.extras.after_key_up = vec![Action {
        hold_down_ns: 50,
        ..key("d").into()
    }];
    let mut actor = actor(config(vec![simple], vec![]));
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&root, 100), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    let step = actor
        .admit(&up(&root, 110), &ContextSnapshot::default())
        .unwrap();
    assert!(step.released_producers.is_empty());
    assert_eq!(drain(&mut actor, 110).0, [(key("d"), KeyEdge::Down)]);
    assert_eq!(actor.pending_inputs(), 1);
    assert_eq!(actor.next_deadline(), Some(MonoTime(160)));
    assert!(drain(&mut actor, 159).0.is_empty());
    let (outputs, released) = drain(&mut actor, 160);
    assert_eq!(outputs, [(key("d"), KeyEdge::Up)]);
    assert_eq!(released, [root.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn early_up_after_branch_preserves_main_fifo_and_multiple_child_retirement() {
    let mut simple = mapping(
        "a",
        vec![
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            key("c").into(),
        ],
    );
    simple.extras.after_key_up = vec![Action {
        hold_down_ns: 100,
        ..key("d").into()
    }];
    let mut actor = actor(config(vec![simple], vec![]));
    let root = input(&mut actor, &[key("a").into()], 1, 100);
    actor
        .admit(&down(&root, 100), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 100).0, [(key("b"), KeyEdge::Down)]);
    actor
        .admit(&up(&root, 110), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(actor.pending_children(), 2);
    assert!(drain(&mut actor, 149).0.is_empty());
    assert_eq!(
        drain(&mut actor, 150).0,
        [
            (key("b"), KeyEdge::Up),
            (key("c"), KeyEdge::Down),
            (key("c"), KeyEdge::Up),
            (key("d"), KeyEdge::Down)
        ]
    );
    assert_eq!(actor.pending_children(), 1);
    assert_eq!(actor.pending_inputs(), 1);
    // The whole branch shifts behind the main tail; its 100ns duration survives.
    assert_eq!(actor.next_deadline(), Some(MonoTime(250)));
    assert!(drain(&mut actor, 249).0.is_empty());
    let (outputs, released) = drain(&mut actor, 250);
    assert_eq!(outputs, [(key("d"), KeyEdge::Up)]);
    assert_eq!(released, [root.events()[0].payload.slot]);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn simple_main_halt_suppresses_after_up_but_incoming_halt_belongs_to_parent() {
    for halted in [false, true] {
        let mut simple = mapping(
            "a",
            if halted {
                vec![Action {
                    halt: true,
                    ..key("b").into()
                }]
            } else {
                vec![]
            },
        );
        simple.extras.after_key_up = vec![key("d").into()];
        let mut actor = actor(config(vec![simple], vec![]));
        let root = input(
            &mut actor,
            &[Action {
                halt: true,
                ..key("a").into()
            }],
            1,
            100,
        );
        let step = actor
            .admit(&down(&root, 100), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(step.originating_halt, Some(root.events()[0].payload.slot));
        drain(&mut actor, 100);
        actor
            .admit(&up(&root, 110), &ContextSnapshot::default())
            .unwrap();
        let outputs = drain(&mut actor, 110).0;
        if halted {
            assert_eq!(outputs, [(key("b"), KeyEdge::Up)]);
        } else {
            assert_eq!(
                outputs,
                [(key("d"), KeyEdge::Down), (key("d"), KeyEdge::Up)]
            );
        }
        assert_eq!(actor.reserved_sources(), 0);
    }
}

#[test]
fn after_up_actions_evaluate_current_stage_variables_once_without_new_rule_selection() {
    for value in [0, 1] {
        let assign = Action {
            target: ActionTarget::SetVariable(VariableAction {
                name: "flag".into(),
                operation: VariableOperation::Set {
                    value: Some(VariableValue::Integer(value)),
                    key_up_value: None,
                },
            }),
            ..key("x").into()
        };
        let mut simple = mapping("a", vec![assign]);
        simple.extras.after_key_up = vec![Action {
            conditions: vec![Condition::Variable {
                name: "flag".into(),
                value: VariableValue::Integer(1),
                unless: false,
            }],
            ..key("d").into()
        }];
        let mut actor = actor(config(vec![simple], vec![]));
        let root = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&root, 100), &ContextSnapshot::default())
            .unwrap();
        assert!(drain(&mut actor, 100).0.is_empty());
        actor
            .admit(&up(&root, 110), &ContextSnapshot::default())
            .unwrap();
        let outputs = drain(&mut actor, 110).0;
        if value == 1 {
            assert_eq!(
                outputs,
                [(key("d"), KeyEdge::Down), (key("d"), KeyEdge::Up)]
            );
        } else {
            assert!(outputs.is_empty());
        }
        assert_eq!(actor.reserved_sources(), 0);
    }
}

#[test]
fn after_up_device_removal_or_reset_cancels_future_edges_and_releases_actual_owner() {
    for reset in [false, true] {
        let mut simple = mapping("a", vec![]);
        simple.extras.after_key_up = vec![
            Action {
                hold_down_ns: 50,
                ..key("d").into()
            },
            key("e").into(),
        ];
        let mut actor = actor(config(vec![simple], vec![]));
        let root = input(&mut actor, &[key("a").into()], 1, 100);
        actor
            .admit(&down(&root, 100), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 100);
        actor
            .admit(&up(&root, 110), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 110).0, [(key("d"), KeyEdge::Down)]);
        let step = if reset {
            actor.reset(MonoTime(120)).unwrap()
        } else {
            actor
                .observe_device(device(1), true, MonoTime(120))
                .unwrap()
        };
        assert_eq!(edges(step.outputs), [(key("d"), KeyEdge::Up)]);
        assert_eq!(
            step.discarded_input_batches,
            [root.events()[0].payload.slot.batch]
        );
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
        assert!(drain(&mut actor, 200).0.is_empty());
    }
}
