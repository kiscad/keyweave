#![allow(clippy::unwrap_used)]
use keyweave_engine::{rule_selection::*, *};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn rule(id: &str, from: &str, to: Vec<Action>) -> Mapping {
    Mapping {
        id: id.into(),
        from: key(from),
        to,
        allow_extra_modifiers: true,
        extras: MappingExtras::default(),
    }
}
fn condition(name: &str) -> Condition {
    Condition::Variable {
        name: name.into(),
        value: VariableValue::Integer(1),
        unless: false,
    }
}
fn snapshot(simple: Vec<Mapping>, complex: Vec<Mapping>, generation: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "selected".into(),
        simple,
        complex,
    })
}
#[test]
fn first_matching_rule_is_selected_only_from_the_requested_stage() {
    let cfg = snapshot(
        vec![
            rule("first", "a", vec![key("b").into()]),
            rule("later", "a", vec![key("c").into()]),
        ],
        vec![rule("complex", "a", vec![key("d").into()])],
        1,
    );
    let simple = select_rule(
        cfg.clone(),
        MappingStage::Simple,
        key("a"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    assert_eq!(simple.index(), 0);
    assert_eq!(simple.mapping().id, "first");
    assert_eq!(simple.stage(), MappingStage::Simple);
    assert_eq!(simple.actions().next().unwrap().key(), Some(key("b")));
    let complex = select_rule(
        cfg,
        MappingStage::Complex,
        key("a"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    assert_eq!(complex.mapping().id, "complex");
    assert_eq!(complex.actions().next().unwrap().key(), Some(key("d")));
}
#[test]
fn unknown_rule_condition_skips_rule_but_empty_filtered_actions_still_consume_match() {
    let mut first = rule("unknown", "a", vec![key("b").into()]);
    first.extras.conditions = vec![condition("unknown")];
    let action = Action {
        conditions: vec![condition("filtered")],
        ..key("c").into()
    };
    let cfg = snapshot(
        vec![],
        vec![
            first,
            rule("empty", "a", vec![action]),
            rule("not-fallback", "a", vec![key("d").into()]),
        ],
        1,
    );
    let selection = select_rule(
        cfg,
        MappingStage::Complex,
        key("a"),
        BTreeSet::new(),
        |_| Truth::Unknown,
    )
    .unwrap();
    assert_eq!(selection.index(), 1);
    assert_eq!(selection.actions().len(), 0);
}
#[test]
fn mandatory_modifier_decision_is_retained_with_actual_side_and_extra_policy() {
    let mut selected = rule("shift", "a", vec![key("b").into()]);
    selected.allow_extra_modifiers = false;
    selected
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    let cfg = snapshot(vec![selected], vec![], 1);
    let modifiers = BTreeSet::from([key("left_shift"), key("right_shift")]);
    let selection = select_rule(
        cfg.clone(),
        MappingStage::Simple,
        key("a"),
        modifiers.clone(),
        |_| Truth::True,
    )
    .unwrap();
    assert_eq!(selection.modifiers_at_down(), &modifiers);
    assert_eq!(
        selection.consumed_modifiers(),
        &BTreeSet::from([key("left_shift")])
    );
    assert!(
        select_rule(
            cfg,
            MappingStage::Simple,
            key("a"),
            BTreeSet::from([key("left_shift"), key("left_control")]),
            |_| Truth::True
        )
        .is_none()
    );
}
#[test]
fn saved_actions_are_evaluated_once_and_keep_original_snapshot_after_context_or_config_changes() {
    let actions = vec![
        Action {
            conditions: vec![condition("keep")],
            ..key("b").into()
        },
        Action {
            conditions: vec![condition("drop")],
            ..key("c").into()
        },
    ];
    let old = snapshot(vec![rule("old", "a", actions)], vec![], 7);
    let weak = Arc::downgrade(&old);
    let mut evaluations = 0;
    let selected = select_rule(
        old.clone(),
        MappingStage::Simple,
        key("a"),
        BTreeSet::new(),
        |predicate| {
            evaluations += 1;
            match predicate {
                Condition::Variable { name, .. } if name == "keep" => Truth::True,
                _ => Truth::False,
            }
        },
    )
    .unwrap();
    drop(old);
    let new = snapshot(vec![rule("new", "a", vec![key("d").into()])], vec![], 8);
    assert_eq!(evaluations, 2);
    assert_eq!(selected.snapshot().generation, ConfigGeneration(7));
    assert!(Arc::ptr_eq(selected.snapshot(), &weak.upgrade().unwrap()));
    assert_eq!(
        selected
            .actions()
            .map(|a| a.key().unwrap())
            .collect::<Vec<_>>(),
        [key("b")]
    );
    assert_eq!(
        selected.actions().next_back().unwrap().key(),
        Some(key("b"))
    );
    assert_eq!(evaluations, 2);
    let later = select_rule(new, MappingStage::Simple, key("a"), BTreeSet::new(), |_| {
        Truth::True
    })
    .unwrap();
    assert_eq!(later.actions().next().unwrap().key(), Some(key("d")));
}
#[test]
fn engine_filters_entire_to_batch_before_earlier_variable_action_mutates_state() {
    let assign = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "layer".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: None,
            },
        }),
        ..key("vk_none").into()
    };
    let cfg = snapshot(
        vec![],
        vec![rule(
            "freeze",
            "a",
            vec![
                assign,
                Action {
                    conditions: vec![condition("layer")],
                    ..key("b").into()
                },
                key("c").into(),
            ],
        )],
        1,
    );
    let mut engine = Engine::new(cfg).unwrap();
    let step = engine
        .handle(InputEnvelope {
            seq: 1,
            at_ns: MonoTime(1),
            device: DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: key("a"),
                edge: KeyEdge::Down,
            },
        })
        .unwrap();
    assert_eq!(engine.variable("layer"), VariableValue::Integer(1));
    assert_eq!(
        step.outputs
            .iter()
            .map(|o| (o.key, o.edge))
            .collect::<Vec<_>>(),
        [(key("c"), KeyEdge::Down)]
    );
}

#[test]
fn saved_rule_generates_timed_next_stage_work_with_original_snapshot_and_input_up_binding() {
    use keyweave_engine::{action_plan::BatchId, timeline::Stage};
    let old = snapshot(
        vec![rule(
            "simple",
            "a",
            vec![
                Action {
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("c").into(),
            ],
        )],
        vec![rule("complex", "b", vec![key("d").into()])],
        7,
    );
    let simple = select_rule(
        old.clone(),
        MappingStage::Simple,
        key("a"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    let plan = simple
        .plan_main(
            BatchId(1),
            MonoTime(10),
            DeviceInstanceId {
                id: 17,
                generation: 3,
            },
            SourceToken(99),
        )
        .unwrap();
    assert_eq!(
        plan.events()
            .iter()
            .map(|e| (e.stage, e.at_ns.0, e.payload.target.clone(), e.payload.edge))
            .collect::<Vec<_>>(),
        [
            (
                Stage::Complex,
                10,
                ActionTarget::Key(key("b")),
                KeyEdge::Down
            ),
            (Stage::Complex, 60, ActionTarget::Key(key("b")), KeyEdge::Up),
            (
                Stage::Complex,
                60,
                ActionTarget::Key(key("c")),
                KeyEdge::Down
            ),
        ]
    );
    for event in plan.events() {
        assert!(Arc::ptr_eq(&event.config, &old));
        assert_eq!(event.source, SourceToken(99));
        assert_eq!(
            event.device,
            DeviceInstanceId {
                id: 17,
                generation: 3
            }
        );
    }
    assert_eq!(plan.input_up_events(MonoTime(20))[0].at_ns, MonoTime(60));
    let complex = select_rule(
        old.clone(),
        MappingStage::Complex,
        key("b"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    let next = complex
        .plan_main(
            BatchId(2),
            MonoTime(10),
            DeviceInstanceId {
                id: 17,
                generation: 3,
            },
            SourceToken(99),
        )
        .unwrap();
    assert_eq!(next.events()[0].stage, Stage::Output);
    assert_eq!(next.events()[0].payload.target, ActionTarget::Key(key("d")));
    assert!(Arc::ptr_eq(&next.events()[0].config, &old));
}
#[test]
fn saved_timing_plan_rejects_overflow_without_executing_or_reselecting_actions() {
    use keyweave_engine::action_plan::{BatchId, PlanError};
    let cfg = snapshot(
        vec![rule(
            "timed",
            "a",
            vec![Action {
                hold_down_ns: 50,
                repeat: false,
                ..key("b").into()
            }],
        )],
        vec![],
        1,
    );
    let selection = select_rule(cfg, MappingStage::Simple, key("a"), BTreeSet::new(), |_| {
        Truth::True
    })
    .unwrap();
    let result = selection.plan_main(
        BatchId(1),
        MonoTime(u64::MAX - 1),
        DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        SourceToken(1),
    );
    assert!(matches!(result, Err(PlanError::TimeOverflow)));
    let plan = selection
        .plan_main(
            BatchId(1),
            MonoTime(0),
            DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            SourceToken(1),
        )
        .unwrap();
    assert_eq!(plan.ends_at(), MonoTime(50));
    assert_eq!(plan.events().len(), 2);
}
