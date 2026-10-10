#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;
fn key(value: &str) -> KeyId {
    value.parse().unwrap()
}
fn config(generation: u64, invoked: &str, canceled: &str) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "delayed".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key("b").into()],
            allow_extra_modifiers: true,
            extras: MappingExtras {
                delayed: Some(DelayedAction {
                    if_invoked: vec![key(invoked).into()],
                    if_canceled: vec![key(canceled).into()],
                    delay_ns: 5,
                }),
                ..Default::default()
            },
        }],
    })
}
fn envelope(seq: u64, time: u64, event: InputEvent) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(time),
        device: DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        provenance: Provenance::Physical,
        event,
    }
}
fn event(engine: &mut Engine, seq: u64, time: u64, event: InputEvent) -> Vec<(String, KeyEdge)> {
    engine
        .handle(envelope(seq, time, event))
        .unwrap()
        .outputs
        .into_iter()
        .map(|output| (output.key.name(), output.edge))
        .collect()
}
fn stroke(
    engine: &mut Engine,
    seq: u64,
    time: u64,
    name: &str,
    edge: KeyEdge,
) -> Vec<(String, KeyEdge)> {
    event(
        engine,
        seq,
        time,
        InputEvent::Key {
            key: key(name),
            edge,
        },
    )
}
#[test]
fn original_up_keeps_task_and_reserved_binding_until_invoked_macro_finishes() {
    let mut engine = Engine::new(config(1, "c", "d")).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (at, token) = engine.next_deadline().unwrap();
    assert_eq!(at, MonoTime(5));
    assert_eq!(
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up),
        [("b".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state().bindings, 0);
    assert_eq!(engine.state().pending_bindings, 1);
    assert_eq!(
        event(&mut engine, 3, 5, InputEvent::TimerFired { token }),
        [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state().pending_bindings, 0);
    assert_eq!(engine.state().pending_deadlines, 0);
    assert_eq!(engine.state().output_owners, 0);
    assert!(event(&mut engine, 4, 6, InputEvent::TimerFired { token }).is_empty());
}
#[test]
fn canceled_macro_runs_before_other_key_down_without_releasing_original_to() {
    for name in ["e", "button1"] {
        let mut engine = Engine::new(config(1, "c", "d")).unwrap();
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
        let (_, token) = engine.next_deadline().unwrap();
        assert_eq!(
            stroke(&mut engine, 2, 1, name, KeyEdge::Down),
            [
                ("d".into(), KeyEdge::Down),
                ("d".into(), KeyEdge::Up),
                (name.into(), KeyEdge::Down)
            ]
        );
        assert!(event(&mut engine, 3, 5, InputEvent::TimerFired { token }).is_empty());
        assert_eq!(
            stroke(&mut engine, 4, 6, "a", KeyEdge::Up),
            [("b".into(), KeyEdge::Up)]
        );
    }
}
#[test]
fn repeat_duplicate_pointer_and_unknown_up_do_not_cancel() {
    let mut engine = Engine::new(config(1, "c", "d")).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let deadline = engine.next_deadline();
    stroke(&mut engine, 2, 1, "a", KeyEdge::Repeat);
    stroke(&mut engine, 3, 2, "a", KeyEdge::Down);
    event(
        &mut engine,
        4,
        3,
        InputEvent::Wheel {
            horizontal: 1,
            vertical: 1,
        },
    );
    event(&mut engine, 5, 4, InputEvent::PointerMove { x: 1, y: 0 });
    stroke(&mut engine, 6, 4, "e", KeyEdge::Up);
    assert_eq!(engine.next_deadline(), deadline);
    assert_eq!(
        event(
            &mut engine,
            7,
            5,
            InputEvent::TimerFired {
                token: deadline.unwrap().1
            }
        ),
        [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
}
#[test]
fn old_config_and_cursor_survive_up_reload_and_variable_macro() {
    let mut cfg = (*config(1, "c", "d")).clone();
    cfg.complex[0].extras.delayed.as_mut().unwrap().if_invoked = vec![Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "old".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Boolean(true)),
                key_up_value: Some(VariableValue::String("done".into())),
            },
        }),
        repeat: true,
        lazy: false,
        halt: false,
        hold_down_ns: 0,
        modifiers: vec![],
        conditions: vec![],
    }];
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    stroke(&mut engine, 2, 1, "a", KeyEdge::Up);
    engine.activate(config(2, "e", "f")).unwrap();
    assert!(event(&mut engine, 3, 5, InputEvent::TimerFired { token }).is_empty());
    assert_eq!(engine.variable("old"), VariableValue::String("done".into()));
    assert_eq!(engine.state().pending_bindings, 0);
}
#[test]
fn remove_reset_and_new_generation_discard_pending_macros() {
    for cause in [InputEvent::Reset, InputEvent::DeviceRemoved] {
        let mut engine = Engine::new(config(1, "c", "d")).unwrap();
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
        let (_, token) = engine.next_deadline().unwrap();
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up);
        assert!(event(&mut engine, 3, 2, cause).is_empty());
        assert_eq!(engine.state().pending_bindings, 0);
        assert_eq!(engine.state().pending_deadlines, 0);
        assert!(event(&mut engine, 4, 5, InputEvent::TimerFired { token }).is_empty());
    }
    let mut engine = Engine::new(config(1, "c", "d")).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, old) = engine.next_deadline().unwrap();
    stroke(&mut engine, 2, 1, "a", KeyEdge::Up);
    let mut down = envelope(
        3,
        2,
        InputEvent::Key {
            key: key("a"),
            edge: KeyEdge::Down,
        },
    );
    down.device.generation = 2;
    assert_eq!(
        engine
            .handle(down)
            .unwrap()
            .outputs
            .iter()
            .map(|output| output.key)
            .collect::<Vec<_>>(),
        [key("b")]
    );
    assert!(event(&mut engine, 4, 5, InputEvent::TimerFired { token: old }).is_empty());
}
#[test]
fn held_and_delayed_tasks_share_reserved_range_and_independent_lifetimes() {
    let mut cfg = (*config(1, "c", "d")).clone();
    cfg.complex[0].extras.if_held_down = vec![key("e").into()];
    cfg.complex[0].extras.held_threshold_ns = 3;
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, held) = engine.next_deadline().unwrap();
    assert_eq!(engine.state().pending_deadlines, 2);
    assert_eq!(
        event(&mut engine, 2, 3, InputEvent::TimerFired { token: held }),
        [("b".into(), KeyEdge::Up), ("e".into(), KeyEdge::Down)]
    );
    let (_, delayed) = engine.next_deadline().unwrap();
    assert_ne!(held, delayed);
    assert_eq!(
        stroke(&mut engine, 3, 4, "a", KeyEdge::Up),
        [("e".into(), KeyEdge::Up)]
    );
    assert_eq!(
        event(&mut engine, 4, 5, InputEvent::TimerFired { token: delayed }),
        [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
}
#[test]
fn delayed_restores_down_modifier_snapshot_and_flushes_idle_device_scope() {
    let mut cfg = (*config(1, "c", "d")).clone();
    cfg.complex[0].extras.from_modifiers = ModifierFilter {
        mandatory: [Modifier::Control].into_iter().collect(),
        optional: [Modifier::Shift].into_iter().collect(),
    };
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    stroke(&mut engine, 1, 0, "left_control", KeyEdge::Down);
    stroke(&mut engine, 2, 0, "left_shift", KeyEdge::Down);
    stroke(&mut engine, 3, 0, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    stroke(&mut engine, 4, 1, "a", KeyEdge::Up);
    stroke(&mut engine, 5, 2, "left_shift", KeyEdge::Up);
    stroke(&mut engine, 6, 3, "left_control", KeyEdge::Up);
    assert_eq!(
        event(&mut engine, 7, 5, InputEvent::TimerFired { token }),
        [
            ("left_shift".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Up),
            ("left_shift".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(engine.state().output_owners, 0);
    assert!(engine.state().pressed_outputs.is_empty());
    assert_eq!(engine.suppression_references(), 0);
}
#[test]
fn cancellation_budget_can_be_drained_without_exceeding_output_limit() {
    let mut cfg = (*config(1, "c", "d")).clone();
    cfg.complex[0].extras.delayed.as_mut().unwrap().if_canceled = (0..249)
        .map(|index| {
            let mut action: Action = key("d").into();
            action.modifiers = vec![key(if index % 2 == 0 {
                "left_control"
            } else {
                "left_shift"
            })];
            action
        })
        .collect();
    let mut other = cfg.complex[0].clone();
    other.from = key("e");
    other.to = (0..249)
        .map(|index| {
            let mut action: Action = key("f").into();
            action.modifiers = vec![key(if index % 2 == 0 {
                "left_control"
            } else {
                "left_shift"
            })];
            action
        })
        .collect();
    other.extras.delayed = None;
    cfg.complex.push(other);
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    stroke(&mut engine, 2, 1, "a", KeyEdge::Up);
    let token = engine.next_delayed().unwrap();
    let before = engine.state();
    assert_eq!(
        engine
            .handle(envelope(
                3,
                2,
                InputEvent::Key {
                    key: key("e"),
                    edge: KeyEdge::Down
                }
            ))
            .unwrap_err(),
        EngineError::CancellationPending
    );
    assert_eq!(engine.state(), before);
    assert_eq!(
        event(&mut engine, 3, 2, InputEvent::CancelDelayed { token }).len(),
        996
    );
    assert_eq!(engine.state().pending_bindings, 0);
    assert_eq!(stroke(&mut engine, 4, 2, "e", KeyEdge::Down).len(), 994);
}

#[test]
fn due_timer_wins_over_cancellation_and_each_branch_is_one_shot() {
    let mut engine = Engine::new(config(1, "c", "d")).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let token = engine.next_delayed().unwrap();
    assert_eq!(
        engine
            .handle(envelope(2, 5, InputEvent::CancelDelayed { token }))
            .unwrap_err(),
        EngineError::DeadlinePending
    );
    assert_eq!(
        engine
            .handle(envelope(
                2,
                5,
                InputEvent::Key {
                    key: key("e"),
                    edge: KeyEdge::Down
                }
            ))
            .unwrap_err(),
        EngineError::DeadlinePending
    );
    assert_eq!(
        event(&mut engine, 2, 5, InputEvent::TimerFired { token }),
        [("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
    assert!(event(&mut engine, 3, 5, InputEvent::CancelDelayed { token }).is_empty());
    assert_eq!(
        stroke(&mut engine, 4, 5, "e", KeyEdge::Down),
        [("e".into(), KeyEdge::Down)]
    );
}
#[test]
fn generated_down_cancels_task_while_root_binding_is_still_being_built() {
    let mut cfg = (*config(1, "c", "d")).clone();
    cfg.complex[0].from = key("left_control");
    let mut second = cfg.complex[0].clone();
    second.from = key("left_shift");
    second.to = vec![key("e").into()];
    second.extras.delayed.as_mut().unwrap().if_invoked = vec![key("f").into()];
    cfg.complex.push(second);
    let mut action: Action = key("left_shift").into();
    action.modifiers = vec![key("left_control")];
    cfg.simple = vec![Mapping {
        id: "simple".into(),
        from: key("a"),
        to: vec![action],
        allow_extra_modifiers: true,
        extras: Default::default(),
    }];
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    assert_eq!(
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down),
        [
            ("b".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Up),
            ("e".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(engine.state().pending_deadlines, 1);
    let token = engine.next_delayed().unwrap();
    assert_eq!(
        event(&mut engine, 2, 5, InputEvent::TimerFired { token }),
        [("f".into(), KeyEdge::Down), ("f".into(), KeyEdge::Up)]
    );
    assert_eq!(
        stroke(&mut engine, 3, 6, "a", KeyEdge::Up),
        [("e".into(), KeyEdge::Up), ("b".into(), KeyEdge::Up)]
    );
}
#[test]
fn repeated_pending_bindings_reclaim_budget_instead_of_exhausting_reservations() {
    let mut engine = Engine::new(config(1, "c", "d")).unwrap();
    for index in 0..2000 {
        stroke(&mut engine, index * 2 + 1, 0, "a", KeyEdge::Down);
        stroke(&mut engine, index * 2 + 2, 0, "a", KeyEdge::Up);
        assert_eq!(engine.state().pending_bindings, 1);
        assert_eq!(engine.state().pending_deadlines, 1);
        assert_eq!(engine.state().output_owners, 0);
    }
    engine.reset(MonoTime(1)).unwrap();
    assert_eq!(engine.state().pending_bindings, 0);
    assert_eq!(engine.state().pending_deadlines, 0);
}
#[test]
fn pending_binding_capacity_is_recovered_by_explicit_cancellation() {
    let mut engine = Engine::new(config(1, "c", "d")).unwrap();
    for id in 1..4096 {
        let mut input = envelope(
            id,
            0,
            InputEvent::Key {
                key: key("e"),
                edge: KeyEdge::Down,
            },
        );
        input.device.id = id;
        engine.handle(input).unwrap();
    }
    stroke(&mut engine, 4096, 0, "a", KeyEdge::Down);
    stroke(&mut engine, 4097, 0, "a", KeyEdge::Up);
    let token = engine.next_delayed().unwrap();
    assert_eq!(
        engine
            .handle(envelope(
                4098,
                1,
                InputEvent::Key {
                    key: key("f"),
                    edge: KeyEdge::Down
                }
            ))
            .unwrap_err(),
        EngineError::CancellationPending
    );
    event(&mut engine, 4098, 1, InputEvent::CancelDelayed { token });
    stroke(&mut engine, 4099, 1, "f", KeyEdge::Down);
    assert_eq!(engine.state().bindings, 4096);
    assert_eq!(engine.state().pending_bindings, 0);
}
#[test]
fn timed_features_in_simple_ir_are_rejected_instead_of_creating_recursive_callbacks() {
    let mut cfg = (*config(1, "c", "d")).clone();
    cfg.simple = std::mem::take(&mut cfg.complex);
    assert!(matches!(
        Engine::new(Arc::new(cfg)),
        Err(EngineError::InvalidMapping)
    ));
}
#[test]
fn delayed_conditions_use_live_original_device_context_and_unknown_unless_stays_inactive() {
    for (vendor, until, matched) in [(None, 6, false), (Some(7), 5, false), (Some(7), 6, true)] {
        let mut cfg = (*config(1, "c", "d")).clone();
        cfg.complex[0].extras.delayed.as_mut().unwrap().if_invoked[0].conditions =
            vec![Condition::Device {
                identifiers: vec![DeviceProperties {
                    vendor_id: Some(42),
                    ..Default::default()
                }],
                unless: true,
                exists: false,
            }];
        let mut engine = Engine::new(Arc::new(cfg)).unwrap();
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
        let token = engine.next_delayed().unwrap();
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up);
        engine.activate(config(2, "e", "f")).unwrap();
        event(
            &mut engine,
            3,
            4,
            InputEvent::ContextChanged {
                context: ContextSnapshot {
                    device_inventory_scope: DeviceInventoryScope::Complete,
                    revision: 1,
                    provider: "synthetic".into(),
                    observed_at_ns: MonoTime(4),
                    valid_until_ns: MonoTime(until),
                    devices: Some(vec![DeviceContext {
                        device: DeviceInstanceId {
                            id: 1,
                            generation: 1,
                        },
                        properties: DeviceProperties {
                            vendor_id: vendor,
                            ..Default::default()
                        },
                    }]),
                    ..Default::default()
                },
            },
        );
        let mut timer = envelope(4, 5, InputEvent::TimerFired { token });
        timer.device.id = 99;
        let step = engine.handle(timer).unwrap();
        assert_eq!(step.outputs.len(), usize::from(matched) * 2);
        if matched {
            assert_eq!(step.outputs[0].key, key("c"));
        }
        assert_eq!(engine.state().pending_bindings, 0);
    }
}
