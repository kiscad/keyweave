#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;
fn key(value: &str) -> KeyId {
    value.parse().unwrap()
}
fn set(name: &str, value: Option<VariableValue>, up: Option<VariableValue>) -> Action {
    Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: name.into(),
            operation: VariableOperation::Set {
                value,
                key_up_value: up,
            },
        }),
        repeat: true,
        lazy: false,
        halt: false,
        hold_down_ns: 0,
        modifiers: vec![],
        conditions: vec![],
    }
}
fn variable(value: VariableValue, unless: bool) -> Condition {
    Condition::Variable {
        name: "layer".into(),
        value,
        unless,
    }
}
fn mapping(from: &str, to: Vec<Action>, conditions: Vec<Condition>) -> Mapping {
    Mapping {
        id: from.into(),
        from: key(from),
        to,
        allow_extra_modifiers: true,
        extras: MappingExtras {
            conditions,
            ..Default::default()
        },
    }
}
fn config(generation: u64, rules: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "vars".into(),
        simple: vec![],
        complex: rules,
    })
}
fn input(
    engine: &mut Engine,
    seq: u64,
    device: u64,
    name: &str,
    edge: KeyEdge,
) -> Vec<(String, KeyEdge)> {
    engine
        .handle(InputEnvelope {
            seq,
            at_ns: MonoTime(seq),
            device: DeviceInstanceId {
                id: device,
                generation: 1,
            },
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: key(name),
                edge,
            },
        })
        .unwrap()
        .outputs
        .into_iter()
        .map(|event| (event.key.name(), event.edge))
        .collect()
}
#[test]
fn layer_works_without_context_and_up_keeps_its_original_key_binding() {
    let mut engine = Engine::new(config(
        1,
        vec![
            mapping(
                "caps_lock",
                vec![set(
                    "layer",
                    Some(VariableValue::Integer(1)),
                    Some(VariableValue::Integer(0)),
                )],
                vec![],
            ),
            mapping(
                "a",
                vec![key("b").into()],
                vec![variable(VariableValue::Integer(1), false)],
            ),
        ],
    ))
    .unwrap();
    assert!(input(&mut engine, 1, 1, "caps_lock", KeyEdge::Down).is_empty());
    assert_eq!(engine.variable("layer"), VariableValue::Integer(1));
    assert_eq!(
        input(&mut engine, 2, 1, "a", KeyEdge::Down),
        [("b".into(), KeyEdge::Down)]
    );
    assert!(input(&mut engine, 3, 1, "caps_lock", KeyEdge::Up).is_empty());
    assert_eq!(engine.variable("layer"), VariableValue::Integer(0));
    assert_eq!(
        input(&mut engine, 4, 1, "a", KeyEdge::Up),
        [("b".into(), KeyEdge::Up)]
    );
    assert_eq!(
        input(&mut engine, 5, 1, "a", KeyEdge::Down),
        [("a".into(), KeyEdge::Down)]
    );
}
#[test]
fn absent_zero_unset_and_distinct_value_types_match_strictly() {
    for value in [
        VariableValue::Integer(0),
        VariableValue::Boolean(false),
        VariableValue::String("0".into()),
    ] {
        let mut engine = Engine::new(config(
            1,
            vec![
                mapping(
                    "a",
                    vec![key("b").into()],
                    vec![variable(value.clone(), false)],
                ),
                mapping("c", vec![set("layer", Some(value.clone()), None)], vec![]),
                mapping(
                    "d",
                    vec![Action {
                        target: ActionTarget::SetVariable(VariableAction {
                            name: "layer".into(),
                            operation: VariableOperation::Unset,
                        }),
                        ..key("a").into()
                    }],
                    vec![],
                ),
            ],
        ))
        .unwrap();
        assert_eq!(
            input(&mut engine, 1, 1, "a", KeyEdge::Down)[0].0,
            if value == VariableValue::Integer(0) {
                "b"
            } else {
                "a"
            }
        );
        input(&mut engine, 2, 1, "a", KeyEdge::Up);
        input(&mut engine, 3, 1, "c", KeyEdge::Down);
        input(&mut engine, 4, 1, "c", KeyEdge::Up);
        assert_eq!(engine.variable("layer"), value);
        assert_eq!(input(&mut engine, 5, 1, "a", KeyEdge::Down)[0].0, "b");
        input(&mut engine, 6, 1, "a", KeyEdge::Up);
        input(&mut engine, 7, 1, "d", KeyEdge::Down);
        input(&mut engine, 8, 1, "d", KeyEdge::Up);
        assert_eq!(engine.variable("layer"), VariableValue::Integer(0));
        assert!(engine.variable_values().is_empty());
    }
}
#[test]
fn all_actions_filter_before_assignment_and_filtered_last_key_remains_held() {
    let mut last: Action = key("b").into();
    last.conditions = vec![variable(VariableValue::Integer(1), false)];
    let mut engine = Engine::new(config(
        1,
        vec![mapping(
            "a",
            vec![set("layer", Some(VariableValue::Integer(1)), None), last],
            vec![],
        )],
    ))
    .unwrap();
    assert!(input(&mut engine, 1, 1, "a", KeyEdge::Down).is_empty());
    input(&mut engine, 2, 1, "a", KeyEdge::Up);
    assert_eq!(
        input(&mut engine, 3, 1, "a", KeyEdge::Down),
        [("b".into(), KeyEdge::Down)]
    );
    assert_eq!(
        input(&mut engine, 4, 1, "a", KeyEdge::Up),
        [("b".into(), KeyEdge::Up)]
    );
}
#[test]
fn variable_repeat_false_sequence_and_extra_actions_have_complete_up_lifetimes() {
    let mut pulse = set(
        "layer",
        Some(VariableValue::Integer(1)),
        Some(VariableValue::Integer(2)),
    );
    pulse.repeat = false;
    let mut rule = mapping("a", vec![pulse], vec![]);
    rule.extras.if_alone = vec![set(
        "layer",
        Some(VariableValue::Integer(3)),
        Some(VariableValue::Integer(4)),
    )];
    rule.extras.after_key_up = vec![set("layer", None, Some(VariableValue::Integer(5)))];
    let mut engine = Engine::new(config(1, vec![rule])).unwrap();
    input(&mut engine, 1, 1, "a", KeyEdge::Down);
    assert_eq!(engine.variable("layer"), VariableValue::Integer(2));
    input(&mut engine, 2, 1, "a", KeyEdge::Repeat);
    assert_eq!(engine.variable("layer"), VariableValue::Integer(2));
    input(&mut engine, 3, 1, "a", KeyEdge::Up);
    assert_eq!(engine.variable("layer"), VariableValue::Integer(5));
    assert_eq!(engine.state().output_owners, 0);
}
#[test]
fn reload_and_device_remove_finish_old_key_up_value_without_running_extra_macro() {
    let mut rule = mapping(
        "a",
        vec![set(
            "layer",
            Some(VariableValue::Integer(1)),
            Some(VariableValue::Integer(2)),
        )],
        vec![],
    );
    rule.extras.after_key_up = vec![set("layer", Some(VariableValue::Integer(99)), None)];
    let mut engine = Engine::new(config(1, vec![rule])).unwrap();
    input(&mut engine, 1, 1, "a", KeyEdge::Down);
    engine
        .activate(config(
            2,
            vec![mapping(
                "a",
                vec![set(
                    "layer",
                    Some(VariableValue::Integer(7)),
                    Some(VariableValue::Integer(8)),
                )],
                vec![],
            )],
        ))
        .unwrap();
    engine
        .handle(InputEnvelope {
            seq: 2,
            at_ns: MonoTime(2),
            device: DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            provenance: Provenance::Physical,
            event: InputEvent::DeviceRemoved,
        })
        .unwrap();
    assert_eq!(engine.variable("layer"), VariableValue::Integer(2));
    assert_eq!(engine.state().bindings, 0);
    input(&mut engine, 3, 2, "a", KeyEdge::Down);
    input(&mut engine, 4, 2, "a", KeyEdge::Up);
    assert_eq!(engine.variable("layer"), VariableValue::Integer(8));
    engine.reset(MonoTime(5)).unwrap();
    assert!(engine.variable_values().is_empty());
}
#[test]
fn shared_variables_are_ordered_global_assignments_not_key_owner_sets() {
    let mut engine = Engine::new(config(
        1,
        vec![mapping(
            "a",
            vec![set(
                "layer",
                Some(VariableValue::Integer(1)),
                Some(VariableValue::Integer(0)),
            )],
            vec![],
        )],
    ))
    .unwrap();
    input(&mut engine, 1, 1, "a", KeyEdge::Down);
    input(&mut engine, 2, 2, "a", KeyEdge::Down);
    input(&mut engine, 3, 1, "a", KeyEdge::Up);
    assert_eq!(engine.variable("layer"), VariableValue::Integer(0));
    assert_eq!(engine.state().bindings, 1);
    input(&mut engine, 4, 2, "a", KeyEdge::Up);
    assert_eq!(engine.state().bindings, 0);
}
#[test]
fn variable_capacity_is_reserved_for_old_bindings_and_reclaimed_on_reset() {
    let rules = (0..256)
        .map(|index| {
            mapping(
                "a",
                vec![set(
                    &format!("v{index}"),
                    Some(VariableValue::Integer(1)),
                    None,
                )],
                vec![],
            )
        })
        .collect();
    let mut engine = Engine::new(config(1, rules)).unwrap();
    input(&mut engine, 1, 1, "a", KeyEdge::Down);
    let before = engine.state();
    assert_eq!(
        engine
            .activate(config(
                2,
                vec![mapping(
                    "a",
                    vec![set("new", Some(VariableValue::Integer(3)), None)],
                    vec![]
                )]
            ))
            .unwrap_err(),
        EngineError::ResourceLimit
    );
    assert_eq!(engine.state(), before);
    input(&mut engine, 2, 1, "a", KeyEdge::Up);
    engine.activate(config(2, vec![])).unwrap();
    engine.reset(MonoTime(3)).unwrap();
    engine
        .activate(config(
            3,
            vec![mapping(
                "a",
                vec![set("new", Some(VariableValue::Integer(3)), None)],
                vec![],
            )],
        ))
        .unwrap();
    input(&mut engine, 4, 1, "a", KeyEdge::Down);
    assert_eq!(engine.variable("new"), VariableValue::Integer(3));
}
#[test]
fn invalid_direct_ir_is_rejected_before_engine_activation() {
    for action in [
        set("", Some(VariableValue::Integer(1)), None),
        set("v", Some(VariableValue::String("a".repeat(4097))), None),
        set("v", None, None),
    ] {
        assert!(matches!(
            Engine::new(config(1, vec![mapping("a", vec![action], vec![])])),
            Err(EngineError::InvalidVariable)
        ));
    }
    let rules = (0..257)
        .map(|index| {
            mapping(
                "a",
                vec![set(
                    &format!("v{index}"),
                    Some(VariableValue::Integer(1)),
                    None,
                )],
                vec![],
            )
        })
        .collect();
    assert!(matches!(
        Engine::new(config(1, rules)),
        Err(EngineError::ResourceLimit)
    ));
}

#[test]
fn variable_assignment_does_not_recompute_modifier_scope_selection() {
    let mut last: Action = key("right_shift").into();
    last.conditions = vec![variable(VariableValue::Integer(0), false)];
    let mut rule = mapping(
        "a",
        vec![set("layer", Some(VariableValue::Integer(1)), None), last],
        vec![],
    );
    rule.extras.from_modifiers = ModifierFilter {
        mandatory: [Modifier::Control].into_iter().collect(),
        optional: Default::default(),
    };
    let mut engine = Engine::new(config(1, vec![rule])).unwrap();
    input(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    assert_eq!(
        input(&mut engine, 2, 1, "a", KeyEdge::Down),
        [
            ("left_control".into(), KeyEdge::Up),
            ("right_shift".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(engine.suppression_references(), 1);
    assert_eq!(
        input(&mut engine, 3, 1, "a", KeyEdge::Up),
        [("right_shift".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.suppression_references(), 0);
    assert_eq!(
        input(&mut engine, 4, 1, "c", KeyEdge::Down),
        [
            ("left_control".into(), KeyEdge::Down),
            ("c".into(), KeyEdge::Down)
        ]
    );
}
