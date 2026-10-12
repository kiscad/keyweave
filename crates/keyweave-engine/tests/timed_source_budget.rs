#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
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
fn cfg(simple: Vec<Mapping>, complex: Vec<Mapping>) -> ConfigSnapshot {
    ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "budget".into(),
        simple,
        complex,
    }
}
#[test]
fn bound_counts_variable_and_each_intermediate_identity_instead_of_only_leaf_keys() {
    let variable = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Unset,
        }),
        modifiers: vec![key("left_shift")],
        ..key("vk_none").into()
    };
    let bounds = cfg(vec![mapping("a", vec![variable])], vec![]).timed_stage_source_bounds();
    assert_eq!(bounds[&key("a")], 5); // root + two Complex + two Output slots
}
#[test]
fn bound_includes_all_complex_branches_and_each_independent_down_snapshot_scope() {
    let mut rule = mapping("b", vec![key("c").into()]);
    rule.extras.if_held_down.push(key("h").into());
    rule.extras.if_alone.push(key("x").into());
    rule.extras.after_key_up.push(key("y").into());
    rule.extras.delayed = Some(DelayedAction {
        delay_ns: 10,
        if_invoked: vec![key("i").into()],
        if_canceled: vec![key("j").into()],
    });
    let bounds =
        cfg(vec![mapping("a", vec![key("b").into()])], vec![rule]).timed_stage_source_bounds();
    assert_eq!(bounds[&key("a")], 32); // root+Complex + 6 outputs + 3*8 snapshots
}
#[test]
fn same_from_candidates_take_max_and_empty_matches_keep_possible_pass_through_ceiling() {
    let bounds = cfg(
        vec![
            mapping("a", vec![]),
            mapping("a", vec![key("b").into(), key("c").into()]),
        ],
        vec![],
    )
    .timed_stage_source_bounds();
    assert_eq!(bounds[&key("a")], 5);
    let empty = cfg(vec![mapping("a", vec![])], vec![]).timed_stage_source_bounds();
    assert_eq!(empty[&key("a")], 3);
}
#[test]
fn modifier_targets_use_their_own_complex_expansion_and_complex_is_not_recursively_remapped() {
    let action = Action {
        modifiers: vec![key("left_shift")],
        ..key("b").into()
    };
    let bounds = cfg(
        vec![mapping("a", vec![action])],
        vec![
            mapping("left_shift", vec![key("x").into(), key("y").into()]),
            mapping("x", vec![key("z").into(); 10]),
        ],
    )
    .timed_stage_source_bounds();
    assert_eq!(bounds[&key("a")], 6); // root + b pass-through(2) + shift(1+2)
}
