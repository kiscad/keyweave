#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, rule_selection::*, timeline::Stage, *};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn gated(name: &str, target: &str) -> Action {
    let mut action: Action = key(target).into();
    action.conditions.push(Condition::Variable {
        name: name.into(),
        value: VariableValue::Integer(1),
        unless: false,
    });
    action.hold_down_ns = 50;
    action
}
fn config() -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "branches".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "original".into(),
            from: key("a"),
            to: vec![gated("main", "b")],
            allow_extra_modifiers: true,
            extras: MappingExtras {
                if_held_down: vec![gated("held", "c")],
                if_alone: vec![gated("alone", "d")],
                after_key_up: vec![gated("after", "e")],
                delayed: Some(DelayedAction {
                    if_invoked: vec![gated("invoked", "f")],
                    if_canceled: vec![gated("canceled", "g")],
                    delay_ns: 100,
                }),
                ..Default::default()
            },
        }],
    })
}
fn selected(config: Arc<ConfigSnapshot>) -> RuleSelection {
    select_rule(
        config,
        MappingStage::Complex,
        key("a"),
        BTreeSet::from([key("left_shift")]),
        |_| Truth::False,
    )
    .unwrap()
}
fn planned(selection: &BranchSelection) -> ActionPlan {
    selection
        .plan(
            BatchId(7),
            MonoTime(100),
            DeviceInstanceId {
                id: 9,
                generation: 2,
            },
            SourceToken(80),
        )
        .unwrap()
}
#[test]
fn each_branch_filters_fresh_and_keeps_its_decision_after_rule_is_dropped() {
    let config = config();
    let rule = selected(config.clone());
    assert_eq!(rule.actions().len(), 0);
    let cases = [
        (RuleBranch::Held, "held", "c"),
        (RuleBranch::Alone, "alone", "d"),
        (RuleBranch::AfterKeyUp, "after", "e"),
        (RuleBranch::DelayedInvoked, "invoked", "f"),
        (RuleBranch::DelayedCanceled, "canceled", "g"),
    ];
    let branches: Vec<_> = cases
        .iter()
        .map(|(branch, name, _)| {
            let mut calls = vec![];
            let result = rule
                .select_branch(*branch, false, |condition| {
                    if let Condition::Variable { name, .. } = condition {
                        calls.push(name.clone());
                    }
                    Truth::True
                })
                .unwrap();
            assert_eq!(calls, [name.to_string()]);
            result
        })
        .collect();
    drop(rule);
    for (selection, (branch, _, target)) in branches.iter().zip(cases) {
        assert_eq!(selection.branch(), branch);
        assert_eq!(selection.actions().next().unwrap().key(), Some(key(target)));
        assert_eq!(
            selection.modifiers_at_down(),
            &BTreeSet::from([key("left_shift")])
        );
        let plan = planned(selection);
        for event in plan.events() {
            assert!(Arc::ptr_eq(&event.config, &config));
            assert_eq!(event.stage, Stage::Output);
            assert_eq!(event.source, SourceToken(80));
            assert_eq!(event.device.generation, 2);
        }
    }
}
#[test]
fn held_waits_for_input_up_but_other_branches_tap_with_duration() {
    let rule = selected(config());
    for branch in [
        RuleBranch::Held,
        RuleBranch::Alone,
        RuleBranch::AfterKeyUp,
        RuleBranch::DelayedInvoked,
        RuleBranch::DelayedCanceled,
    ] {
        let branch = rule.select_branch(branch, false, |_| Truth::True).unwrap();
        let plan = planned(&branch);
        if branch.branch() == RuleBranch::Held {
            assert_eq!(plan.events().len(), 1);
            assert_eq!(plan.releases().len(), 1);
            assert_eq!(plan.input_up_events(MonoTime(120))[0].at_ns, MonoTime(120));
        } else {
            assert_eq!(
                plan.events()
                    .iter()
                    .map(|event| (event.payload.edge, event.at_ns))
                    .collect::<Vec<_>>(),
                [(KeyEdge::Down, MonoTime(100)), (KeyEdge::Up, MonoTime(150))]
            );
            assert!(plan.releases().is_empty());
        }
        let expected = if matches!(branch.branch(), RuleBranch::Held | RuleBranch::AfterKeyUp) {
            BranchModifierPolicy::CurrentMinusConsumed
        } else {
            BranchModifierPolicy::DownSnapshotMinusConsumed
        };
        assert_eq!(branch.modifier_policy(), expected);
    }
}
#[test]
fn unknown_and_halt_do_not_generate_actions_or_evaluate_again_during_planning() {
    let rule = selected(config());
    let mut calls = 0;
    let empty = rule
        .select_branch(RuleBranch::Alone, false, |_| {
            calls += 1;
            Truth::Unknown
        })
        .unwrap();
    assert_eq!(calls, 1);
    assert!(planned(&empty).events().is_empty());
    let halted = rule
        .select_branch(RuleBranch::Held, true, |_| {
            panic!("halted branch must not evaluate actions")
        })
        .unwrap();
    assert!(planned(&halted).events().is_empty());
    assert!(planned(&halted).releases().is_empty());
    assert_eq!(calls, 1);
}
#[test]
fn absent_delayed_branch_is_distinct_from_filtered_empty_branch() {
    let mut no_delayed = (*config()).clone();
    no_delayed.complex[0].extras.delayed = None;
    let rule = selected(Arc::new(no_delayed));
    for branch in [RuleBranch::DelayedInvoked, RuleBranch::DelayedCanceled] {
        assert!(
            rule.select_branch(branch, false, |_| panic!("absent branch"))
                .is_none()
        );
    }
    assert_eq!(
        rule.select_branch(RuleBranch::Alone, false, |_| Truth::False)
            .unwrap()
            .actions()
            .len(),
        0
    );
}
#[test]
fn simple_branch_plans_complex_work_and_retains_concrete_consumed_side() {
    let mut config = (*config()).clone();
    let mut mapping = config.complex.remove(0);
    mapping
        .extras
        .from_modifiers
        .mandatory
        .insert(Modifier::Shift);
    config.simple.push(mapping);
    let config = Arc::new(config);
    let rule = select_rule(
        config.clone(),
        MappingStage::Simple,
        key("a"),
        BTreeSet::from([key("right_shift")]),
        |_| Truth::False,
    )
    .unwrap();
    let branch = rule
        .select_branch(RuleBranch::AfterKeyUp, false, |_| Truth::True)
        .unwrap();
    assert_eq!(
        branch.consumed_modifiers(),
        &BTreeSet::from([key("right_shift")])
    );
    assert_eq!(
        branch.modifier_policy(),
        BranchModifierPolicy::CurrentMinusConsumed
    );
    let plan = planned(&branch);
    assert!(
        plan.events()
            .iter()
            .all(|event| event.stage == Stage::Complex && Arc::ptr_eq(&event.config, &config))
    );
    assert_eq!(plan.events().len(), 2);
}
#[test]
fn real_engine_held_branch_filters_after_main_but_before_its_own_assignments() {
    let variable = |value| Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "held".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(value)),
                key_up_value: None,
            },
        }),
        ..key("b").into()
    };
    let mut last = gated("held", "right_shift");
    last.hold_down_ns = 0;
    let config = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "live-held".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![variable(1)],
            allow_extra_modifiers: true,
            extras: MappingExtras {
                if_held_down: vec![variable(0), last],
                held_threshold_ns: 10,
                ..Default::default()
            },
        }],
    });
    let mut engine = Engine::new(config).unwrap();
    let envelope = |seq, at, event| InputEnvelope {
        seq,
        at_ns: MonoTime(at),
        device: DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        provenance: Provenance::Physical,
        event,
    };
    assert!(
        engine
            .handle(envelope(
                1,
                0,
                InputEvent::Key {
                    key: key("a"),
                    edge: KeyEdge::Down
                }
            ))
            .unwrap()
            .outputs
            .is_empty()
    );
    let (at, token) = engine.next_deadline().unwrap();
    assert_eq!(at, MonoTime(10));
    let held = engine
        .handle(envelope(2, 10, InputEvent::TimerFired { token }))
        .unwrap();
    assert_eq!(
        held.outputs
            .iter()
            .map(|event| (event.key, event.edge))
            .collect::<Vec<_>>(),
        [(key("right_shift"), KeyEdge::Down)]
    );
    assert_eq!(engine.variable("held"), VariableValue::Integer(0));
    let up = engine
        .handle(envelope(
            3,
            11,
            InputEvent::Key {
                key: key("a"),
                edge: KeyEdge::Up,
            },
        ))
        .unwrap();
    assert_eq!(
        up.outputs
            .iter()
            .map(|event| (event.key, event.edge))
            .collect::<Vec<_>>(),
        [(key("right_shift"), KeyEdge::Up)]
    );
    assert_eq!(engine.state().output_owners, 0);
}
