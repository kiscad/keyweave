#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;
fn key(value: &str) -> KeyId {
    value.parse().unwrap()
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
fn config(rules: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "flags".into(),
        simple: vec![],
        complex: rules,
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
fn lazy_modifier_is_logical_until_next_down_and_neutral_releases_it() {
    let mut lazy: Action = key("left_control").into();
    lazy.lazy = true;
    let mut engine = Engine::new(config(vec![mapping("a", vec![lazy])])).unwrap();
    assert!(stroke(&mut engine, 1, 0, "a", KeyEdge::Down).is_empty());
    assert_eq!(engine.state().output_owners, 1);
    assert!(engine.state().pressed_outputs.is_empty());
    assert_eq!(
        stroke(&mut engine, 2, 1, "b", KeyEdge::Down),
        [
            ("left_control".into(), KeyEdge::Down),
            ("b".into(), KeyEdge::Down)
        ]
    );
    assert!(stroke(&mut engine, 3, 2, "a", KeyEdge::Up).is_empty());
    assert_eq!(
        stroke(&mut engine, 4, 3, "b", KeyEdge::Up),
        [
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up)
        ]
    );
    assert_eq!(engine.state().output_owners, 0);
}
#[test]
fn unused_lazy_tap_and_reset_do_not_invent_modifier_edges() {
    let mut lazy: Action = key("left_control").into();
    lazy.lazy = true;
    let mut engine = Engine::new(config(vec![mapping("a", vec![lazy.clone()])])).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    assert!(engine.reset(MonoTime(1)).unwrap().is_empty());
    assert_eq!(engine.state().output_owners, 0);
    lazy.repeat = false;
    let mut engine = Engine::new(config(vec![mapping("a", vec![lazy])])).unwrap();
    assert!(stroke(&mut engine, 1, 0, "a", KeyEdge::Down).is_empty());
    assert!(stroke(&mut engine, 2, 1, "a", KeyEdge::Up).is_empty());
}
#[test]
fn lazy_modifier_scope_and_next_phase_remapping_use_action_policies() {
    let mut lazy: Action = key("left_shift").into();
    lazy.lazy = true;
    lazy.modifiers = vec![key("left_control")];
    let mut cfg = (*config(vec![])).clone();
    cfg.simple = vec![mapping("a", vec![lazy])];
    cfg.complex = vec![mapping("left_shift", vec![key("b").into()])];
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    assert_eq!(
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down),
        [
            ("left_control".into(), KeyEdge::Down),
            ("b".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up),
        [
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up)
        ]
    );
}
#[test]
fn lazy_on_nonmodifier_does_not_delay_key_or_variable_and_variable_down_flushes_flags() {
    let mut plain: Action = key("b").into();
    plain.lazy = true;
    let mut variable: Action = key("vk_none").into();
    variable.target = ActionTarget::SetVariable(VariableAction {
        name: "mode".into(),
        operation: VariableOperation::Set {
            value: Some(VariableValue::Boolean(true)),
            key_up_value: None,
        },
    });
    variable.lazy = true;
    let mut rule = mapping("a", vec![variable]);
    rule.extras.from_modifiers = ModifierFilter {
        mandatory: [Modifier::Control].into_iter().collect(),
        optional: Default::default(),
    };
    let mut engine = Engine::new(config(vec![rule, mapping("c", vec![plain])])).unwrap();
    stroke(&mut engine, 1, 0, "left_control", KeyEdge::Down);
    assert_eq!(
        stroke(&mut engine, 2, 1, "a", KeyEdge::Down),
        [("left_control".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.variable("mode"), VariableValue::Boolean(true));
    assert_eq!(
        stroke(&mut engine, 3, 2, "c", KeyEdge::Down),
        [
            ("left_control".into(), KeyEdge::Down),
            ("b".into(), KeyEdge::Down)
        ]
    );
}
#[test]
fn halt_finishes_current_array_and_ups_but_skips_later_macros() {
    let mut halted: Action = key("b").into();
    halted.halt = true;
    let mut rule = mapping("a", vec![halted, key("c").into()]);
    rule.extras.if_alone = vec![key("escape").into()];
    rule.extras.after_key_up = vec![key("d").into()];
    let mut engine = Engine::new(config(vec![rule])).unwrap();
    assert_eq!(
        stroke(&mut engine, 1, 0, "a", KeyEdge::Down),
        [
            ("b".into(), KeyEdge::Down),
            ("b".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up),
        [("c".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state().output_owners, 0);
}
#[test]
fn halt_in_alone_batch_skips_after_and_delayed_after_original_up() {
    let mut halted: Action = key("escape").into();
    halted.halt = true;
    let mut rule = mapping("a", vec![key("b").into()]);
    rule.extras.if_alone = vec![halted, key("c").into()];
    rule.extras.after_key_up = vec![key("d").into()];
    rule.extras.delayed = Some(DelayedAction {
        if_invoked: vec![key("e").into()],
        if_canceled: vec![],
        delay_ns: 5,
    });
    let mut engine = Engine::new(config(vec![rule])).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let token = engine.next_delayed().unwrap();
    assert_eq!(
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up),
        [
            ("b".into(), KeyEdge::Up),
            ("escape".into(), KeyEdge::Down),
            ("escape".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Up)
        ]
    );
    assert!(event(&mut engine, 3, 5, InputEvent::TimerFired { token }).is_empty());
    assert_eq!(engine.state().pending_bindings, 0);
}
#[test]
fn halted_held_callback_releases_original_output_and_delayed_halt_skips_later_alone() {
    let mut halted: Action = key("b").into();
    halted.halt = true;
    let mut rule = mapping("a", vec![halted]);
    rule.extras.if_held_down = vec![key("c").into()];
    rule.extras.held_threshold_ns = 5;
    let mut engine = Engine::new(config(vec![rule])).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let (_, token) = engine.next_deadline().unwrap();
    assert_eq!(
        event(&mut engine, 2, 5, InputEvent::TimerFired { token }),
        [("b".into(), KeyEdge::Up)]
    );
    assert!(stroke(&mut engine, 3, 6, "a", KeyEdge::Up).is_empty());
    let mut rule = mapping("a", vec![key("b").into()]);
    let mut halted: Action = key("c").into();
    halted.halt = true;
    rule.extras.delayed = Some(DelayedAction {
        if_invoked: vec![halted],
        if_canceled: vec![],
        delay_ns: 5,
    });
    rule.extras.if_alone = vec![key("escape").into()];
    let mut engine = Engine::new(config(vec![rule])).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    let token = engine.next_delayed().unwrap();
    event(&mut engine, 2, 5, InputEvent::TimerFired { token });
    assert_eq!(
        stroke(&mut engine, 3, 6, "a", KeyEdge::Up),
        [("b".into(), KeyEdge::Up)]
    );
}
#[test]
fn skipped_halt_has_no_effect_and_parent_halt_does_not_spread_to_next_phase() {
    let mut skipped: Action = key("c").into();
    skipped.halt = true;
    skipped.conditions = vec![Condition::Variable {
        name: "missing".into(),
        value: VariableValue::Integer(1),
        unless: false,
    }];
    let mut rule = mapping("a", vec![key("b").into(), skipped]);
    rule.extras.after_key_up = vec![key("d").into()];
    let mut engine = Engine::new(config(vec![rule])).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    assert_eq!(
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up),
        [
            ("b".into(), KeyEdge::Up),
            ("d".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Up)
        ]
    );
    let mut halted: Action = key("b").into();
    halted.halt = true;
    let mut cfg = (*config(vec![mapping("b", vec![key("c").into()])])).clone();
    cfg.simple = vec![mapping("a", vec![halted])];
    cfg.complex[0].extras.after_key_up = vec![key("d").into()];
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    stroke(&mut engine, 1, 0, "a", KeyEdge::Down);
    assert_eq!(
        stroke(&mut engine, 2, 1, "a", KeyEdge::Up),
        [
            ("c".into(), KeyEdge::Up),
            ("d".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Up)
        ]
    );
}
#[test]
fn modifier_flush_margin_rejects_oversized_macro_without_changing_lazy_owners() {
    let names = [
        "left_control",
        "left_shift",
        "left_option",
        "left_command",
        "right_control",
        "right_shift",
        "right_option",
        "right_command",
    ];
    let mut cfg = (*config(vec![mapping("b", vec![key("c").into(), key("d").into()])])).clone();
    cfg.simple = vec![mapping("a", vec![key("b").into(); 256])];
    for (index, name) in names.iter().enumerate() {
        let mut lazy: Action = key(name).into();
        lazy.lazy = true;
        cfg.complex
            .push(mapping(&format!("f{}", index + 1), vec![lazy]));
    }
    assert_eq!(cfg.expansion_bounds()[&key("a")], 512);
    assert!(cfg.output_bounds()[&key("a")] > MAX_OUTPUTS_PER_EVENT);
    let mut engine = Engine::new(Arc::new(cfg)).unwrap();
    for index in 0..8 {
        assert!(
            stroke(
                &mut engine,
                index + 1,
                index,
                &format!("f{}", index + 1),
                KeyEdge::Down
            )
            .is_empty()
        );
    }
    let before = engine.state();
    assert_eq!(
        engine
            .handle(envelope(
                9,
                8,
                InputEvent::Key {
                    key: key("a"),
                    edge: KeyEdge::Down
                }
            ))
            .unwrap_err(),
        EngineError::ResourceLimit
    );
    assert_eq!(engine.state(), before);
}
#[test]
fn engine_can_move_to_single_execution_actor() {
    fn assert_send<T: Send>() {}
    assert_send::<Engine>();
}

#[test]
fn halted_variable_finishes_its_up_value_after_reload() {
    let action = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "mode".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(0)),
            },
        }),
        halt: true,
        hold_down_ns: 0,
        ..key("b").into()
    };
    let mut rule = mapping("a", vec![action]);
    rule.extras.after_key_up = vec![key("c").into()];
    let mut engine = Engine::new(config(vec![rule])).unwrap();
    assert!(stroke(&mut engine, 1, 0, "a", KeyEdge::Down).is_empty());
    let mut next = (*config(vec![mapping("a", vec![key("d").into()])])).clone();
    next.generation = ConfigGeneration(2);
    engine.activate(Arc::new(next)).unwrap();
    assert!(stroke(&mut engine, 2, 1, "a", KeyEdge::Up).is_empty());
    assert_eq!(
        engine.variable_values().get("mode"),
        Some(&VariableValue::Integer(0))
    );
    assert_eq!(engine.state().output_owners, 0);
    assert_eq!(
        stroke(&mut engine, 3, 2, "a", KeyEdge::Down),
        [("d".into(), KeyEdge::Down)]
    );
}
