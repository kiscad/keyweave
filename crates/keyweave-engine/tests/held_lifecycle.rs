#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;
fn key(value: &str) -> KeyId {
    value.parse().unwrap()
}
fn config(generation: u64, held: &str, threshold: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "held".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key("b").into()],
            allow_extra_modifiers: true,
            extras: MappingExtras {
                if_held_down: vec![key(held).into()],
                held_threshold_ns: threshold,
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
fn stroke(
    engine: &mut Engine,
    seq: u64,
    time: u64,
    name: &str,
    edge: KeyEdge,
) -> Vec<(String, KeyEdge)> {
    engine
        .handle(envelope(
            seq,
            time,
            InputEvent::Key {
                key: key(name),
                edge,
            },
        ))
        .unwrap()
        .outputs
        .into_iter()
        .map(|output| (output.key.name(), output.edge))
        .collect()
}
fn fire(engine: &mut Engine, seq: u64, time: u64, token: DeadlineToken) -> Vec<(String, KeyEdge)> {
    engine
        .handle(envelope(seq, time, InputEvent::TimerFired { token }))
        .unwrap()
        .outputs
        .into_iter()
        .map(|output| (output.key.name(), output.edge))
        .collect()
}
#[test]
fn threshold_releases_original_to_then_holds_new_output_until_up() {
    let mut engine = Engine::new(config(1, "c", 5)).unwrap();
    assert_eq!(
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down),
        [("b".into(), KeyEdge::Down)]
    );
    let (deadline, token) = engine.next_deadline().unwrap();
    assert_eq!(deadline, MonoTime(5));
    assert_eq!(engine.state().pending_deadlines, 1);
    assert_eq!(
        engine
            .handle(envelope(2, 4, InputEvent::TimerFired { token }))
            .unwrap_err(),
        EngineError::InvalidDeadline
    );
    assert_eq!(
        fire(&mut engine, 2, 5, token),
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
    assert_eq!(
        stroke(&mut engine, 3, 6, "a", KeyEdge::Up),
        [("c".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state().output_owners, 0);
    assert!(engine.next_deadline().is_none());
}
#[test]
fn early_up_cancels_and_stale_callback_is_idempotent() {
    let mut engine = Engine::new(config(1, "c", 5)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    assert_eq!(
        stroke(&mut engine, 2, 4, "a", KeyEdge::Up),
        [("b".into(), KeyEdge::Up)]
    );
    assert!(fire(&mut engine, 3, 5, token).is_empty());
    assert_eq!(engine.state().bindings, 0);
}
#[test]
fn due_timer_precedes_equal_time_up_and_zero_threshold_is_explicit() {
    for threshold in [0, 5] {
        let mut engine = Engine::new(config(1, "c", threshold)).unwrap();
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
        let (_, token) = engine.next_deadline().unwrap();
        let before = engine.state();
        assert_eq!(
            engine
                .handle(envelope(
                    2,
                    threshold,
                    InputEvent::Key {
                        key: key("a"),
                        edge: KeyEdge::Up
                    }
                ))
                .unwrap_err(),
            EngineError::DeadlinePending
        );
        assert_eq!(engine.state(), before);
        fire(&mut engine, 2, threshold, token);
        assert_eq!(
            stroke(&mut engine, 3, threshold, "a", KeyEdge::Up),
            [("c".into(), KeyEdge::Up)]
        );
    }
}
#[test]
fn other_down_and_button_cancel_but_repeat_duplicate_wheel_move_do_not() {
    for name in ["d", "button1"] {
        let mut engine = Engine::new(config(1, "c", 10)).unwrap();
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
        let (_, token) = engine.next_deadline().unwrap();
        stroke(&mut engine, 2, 1, name, KeyEdge::Down);
        assert!(engine.next_deadline().is_none());
        assert!(fire(&mut engine, 3, 10, token).is_empty());
        assert_eq!(
            stroke(&mut engine, 4, 11, "a", KeyEdge::Up),
            [("b".into(), KeyEdge::Up)]
        );
    }
    let mut engine = Engine::new(config(1, "c", 10)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let expected = engine.next_deadline();
    stroke(&mut engine, 2, 1, "a", KeyEdge::Repeat);
    stroke(&mut engine, 3, 2, "a", KeyEdge::Down);
    engine
        .handle(envelope(
            4,
            3,
            InputEvent::Wheel {
                horizontal: 0,
                vertical: 1,
            },
        ))
        .unwrap();
    engine
        .handle(envelope(5, 4, InputEvent::PointerMove { x: 1, y: 1 }))
        .unwrap();
    assert_eq!(engine.next_deadline(), expected);
    assert_eq!(
        fire(&mut engine, 6, 10, expected.unwrap().1),
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
}
#[test]
fn old_snapshot_timer_survives_reload_but_remove_and_reset_cancel() {
    for reset in [false, true] {
        let mut engine = Engine::new(config(1, "c", 5)).unwrap();
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
        let (_, token) = engine.next_deadline().unwrap();
        engine.activate(config(2, "d", 1)).unwrap();
        if reset {
            engine.reset(MonoTime(1)).unwrap();
        } else {
            engine
                .handle(envelope(2, 1, InputEvent::DeviceRemoved))
                .unwrap();
        }
        assert!(fire(&mut engine, 3, 5, token).is_empty());
        assert_eq!(engine.state().output_owners, 0);
    }
    let mut engine = Engine::new(config(1, "c", 5)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    engine.activate(config(2, "d", 1)).unwrap();
    assert_eq!(
        fire(&mut engine, 2, 5, token),
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
}
#[test]
fn held_does_not_cancel_alone_and_after_macros_still_finish() {
    let mut config = (*config(1, "c", 5)).clone();
    config.complex[0].extras.if_alone = vec![key("escape").into()];
    config.complex[0].extras.after_key_up = vec![key("d").into()];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    fire(&mut engine, 2, 5, token);
    assert_eq!(
        stroke(&mut engine, 3, 6, "a", KeyEdge::Up),
        [
            ("c".into(), KeyEdge::Up),
            ("escape".into(), KeyEdge::Down),
            ("escape".into(), KeyEdge::Up),
            ("d".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Up)
        ]
    );
}
#[test]
fn fired_action_conditions_use_current_original_device_context_not_timer_carrier() {
    let mut config = (*config(1, "c", 5)).clone();
    config.complex[0].extras.if_held_down[0].conditions = vec![Condition::Device {
        identifiers: vec![DeviceProperties {
            vendor_id: Some(42),
            ..Default::default()
        }],
        unless: false,
        exists: false,
    }];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    engine
        .handle(envelope(
            2,
            5,
            InputEvent::ContextChanged {
                context: ContextSnapshot {
                    revision: 1,
                    provider: "test".into(),
                    observed_at_ns: MonoTime(5),
                    valid_until_ns: MonoTime(6),
                    devices: Some(vec![DeviceContext {
                        device: DeviceInstanceId {
                            id: 1,
                            generation: 1,
                        },
                        properties: DeviceProperties {
                            vendor_id: Some(42),
                            ..Default::default()
                        },
                    }]),
                    ..Default::default()
                },
            },
        ))
        .unwrap();
    let mut timer = envelope(3, 5, InputEvent::TimerFired { token });
    timer.device.id = 99;
    let outputs = engine.handle(timer).unwrap().outputs;
    assert_eq!(
        outputs.iter().map(|output| output.key).collect::<Vec<_>>(),
        [key("b"), key("c")]
    );
}
#[test]
fn overflow_and_future_output_limit_fail_before_any_ownership_changes() {
    let mut engine = Engine::new(config(1, "c", 5)).unwrap();
    assert_eq!(
        engine
            .handle(envelope(
                1,
                u64::MAX - 4,
                InputEvent::Key {
                    key: key("a"),
                    edge: KeyEdge::Down
                }
            ))
            .unwrap_err(),
        EngineError::TimeOverflow
    );
    assert_eq!(engine.state().output_owners, 0);
    let mut config = (*config(1, "c", 5)).clone();
    config.complex[0].extras.if_held_down = (0..512).map(|_| key("c").into()).collect();
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        engine
            .handle(envelope(
                1,
                0,
                InputEvent::Key {
                    key: key("a"),
                    edge: KeyEdge::Down
                }
            ))
            .unwrap_err(),
        EngineError::ResourceLimit
    );
    assert_eq!(engine.state().pending_deadlines, 0);
}
#[test]
fn held_modifiers_consume_and_restore_scope_using_the_original_binding() {
    let mut config = (*config(1, "right_shift", 5)).clone();
    config.complex[0].extras.from_modifiers = ModifierFilter {
        mandatory: [Modifier::Control].into_iter().collect(),
        optional: Default::default(),
    };
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    stroke(&mut engine, 1, 0, "left_control", KeyEdge::Down);
    stroke(&mut engine, 2, 1, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    assert_eq!(
        fire(&mut engine, 3, 6, token),
        [
            ("b".into(), KeyEdge::Up),
            ("right_shift".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(engine.suppression_references(), 1);
    stroke(&mut engine, 4, 7, "a", KeyEdge::Up);
    assert_eq!(engine.suppression_references(), 0);
    assert_eq!(
        stroke(&mut engine, 5, 8, "d", KeyEdge::Down),
        [
            ("left_control".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Down)
        ]
    );
}

#[test]
fn complex_held_rule_from_simple_output_replaces_and_releases_correctly() {
    let mut cfg = (*config(1, "c", 5)).clone();
    cfg.complex[0].from = key("b");
    cfg.complex[0].to = vec![key("d").into()];
    cfg.simple = vec![Mapping {
        id: "simple".into(),
        from: key("a"),
        to: vec![key("b").into()],
        allow_extra_modifiers: true,
        extras: Default::default(),
    }];
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    assert_eq!(
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down),
        [("d".into(), KeyEdge::Down)]
    );
    let (_, token) = engine.next_deadline().unwrap();
    assert_eq!(
        fire(&mut engine, 2, 5, token),
        [("d".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
    assert_eq!(
        stroke(&mut engine, 3, 6, "a", KeyEdge::Up),
        [("c".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state().output_owners, 0);
}
#[test]
fn later_down_in_same_generated_complex_phase_cancels_earlier_held_task() {
    let mut cfg = (*config(1, "c", 5)).clone();
    cfg.complex[0].from = key("left_control");
    let mut second = cfg.complex[0].clone();
    second.from = key("left_shift");
    second.to = vec![key("d").into()];
    second.extras.if_held_down = vec![key("e").into()];
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
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    assert_eq!(engine.state().pending_deadlines, 1);
    let (_, token) = engine.next_deadline().unwrap();
    assert_eq!(
        fire(&mut engine, 2, 5, token),
        [("d".into(), KeyEdge::Up), ("e".into(), KeyEdge::Down)]
    );
    stroke(&mut engine, 3, 6, "a", KeyEdge::Up);
    assert_eq!(engine.state().output_owners, 0);
}
#[test]
fn replug_and_late_overflow_callbacks_cannot_resurrect_an_old_binding() {
    let mut engine = Engine::new(config(1, "c", 5)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, old) = engine.next_deadline().unwrap();
    let mut input = envelope(
        2,
        1,
        InputEvent::Key {
            key: key("a"),
            edge: KeyEdge::Down,
        },
    );
    input.device.generation = 2;
    engine.handle(input).unwrap();
    let (_, new) = engine.next_deadline().unwrap();
    assert_ne!(old, new);
    assert!(fire(&mut engine, 3, 5, old).is_empty());
    assert_eq!(engine.state().pending_deadlines, 1);
    assert_eq!(
        engine
            .handle(envelope(4, u64::MAX, InputEvent::TimerFired { token: new }))
            .unwrap_err(),
        EngineError::TimeOverflow
    );
    assert_eq!(
        fire(&mut engine, 4, 6, new),
        [("b".into(), KeyEdge::Up), ("c".into(), KeyEdge::Down)]
    );
    engine.reset(MonoTime(7)).unwrap();
    assert!(engine.next_deadline().is_none());
}

#[test]
fn reset_clears_pending_deadline_before_any_callback_arrives() {
    let mut engine = Engine::new(config(1, "c", 5)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    engine.reset(MonoTime(1)).unwrap();
    assert_eq!(engine.state().pending_deadlines, 0);
    assert!(engine.next_deadline().is_none());
    assert_eq!(
        stroke(&mut engine, 2, 6, "d", KeyEdge::Down),
        [("d".into(), KeyEdge::Down)]
    );
}
