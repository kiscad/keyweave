#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, modifier_scope::ScopedActionPlan, output_stage::*, rule_selection::*,
    stage_bindings::*, timeline::*, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
struct Payload(u64); // Deliberately no Clone or Debug.
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn slot(batch: u64) -> ActionSlot {
    ActionSlot {
        batch: BatchId(batch),
        index: 0,
    }
}
fn cfg(actions: Vec<Action>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "shared output".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "primary".into(),
            from: key("a"),
            to: actions,
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
    })
}
fn origin(cfg: &Arc<ConfigSnapshot>, batch: u64, dev: u64, stage: Stage, at: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(batch),
        stage,
        start: MonoTime(at),
        device: DeviceInstanceId {
            id: dev,
            generation: 1,
        },
        source: SourceToken(dev),
        config: cfg.clone(),
    }
}
fn root(
    arena: &mut BindingArena<Payload>,
    cfg: &Arc<ConfigSnapshot>,
    batch: u64,
    dev: u64,
    name: &str,
) -> DueEvent<PlannedAction> {
    let origin = origin(cfg, batch, dev, Stage::Complex, 100);
    arena.prepare(origin.clone(), 1, None).unwrap().commit();
    let plan = plan_actions(&[key(name).into()], origin, false).unwrap();
    DueEvent {
        event: plan.events()[0].clone(),
        delivered_at: MonoTime(100),
    }
}
fn inputs(
    arena: &mut BindingArena<Payload>,
    cfg: &Arc<ConfigSnapshot>,
) -> GroupDownInputs<Payload> {
    vec![
        (root(arena, cfg, 1, 1, "a"), Payload(10)),
        (root(arena, cfg, 2, 2, "b"), Payload(20)),
    ]
}
fn plan(cfg: &Arc<ConfigSnapshot>, id: u64, at: u64) -> ScopedActionPlan {
    let selected = select_rule(
        cfg.clone(),
        MappingStage::Complex,
        key("a"),
        BTreeSet::new(),
        |_| Truth::True,
    )
    .unwrap();
    ScopedActionPlan::main(
        &selected,
        BatchId(id),
        MonoTime(at),
        DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        SourceToken(1),
    )
    .unwrap()
}
fn output(cfg: &Arc<ConfigSnapshot>) -> OutputStageActor {
    let mut output = OutputStageActor::new(cfg.clone()).unwrap();
    output
        .observe_device(
            DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            false,
            MonoTime(0),
        )
        .unwrap();
    output
        .observe_device(
            DeviceInstanceId {
                id: 2,
                generation: 1,
            },
            false,
            MonoTime(0),
        )
        .unwrap();
    output
}
fn drain(output: &mut OutputStageActor, at: u64) -> (Vec<(KeyId, KeyEdge)>, Vec<BatchId>) {
    let mut edges = vec![];
    let mut done = vec![];
    for _ in 0..10000 {
        let Some(step) = output.poll(MonoTime(at)).unwrap() else {
            return (edges, done);
        };
        edges.extend(step.outputs.into_iter().map(|out| (out.key, out.edge)));
        if let Some(id) = step.completed_batch {
            done.push(id);
        }
    }
    panic!("bounded fixture did not drain")
}
#[test]
fn group_abort_returns_all_owned_inputs_without_activating_or_consuming_retention_id() {
    let cfg = cfg(vec![key("c").into()]);
    let mut arena = BindingArena::default();
    let owned = inputs(&mut arena, &cfg);
    let returned = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 1, owned)
        .unwrap()
        .abort();
    assert_eq!(returned[0].1.0, 10);
    assert_eq!(returned[1].1.0, 20);
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.shared_retention_groups(), 0);
    assert_eq!(arena.reserved_sources(), 2);
    let handle = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 1, returned)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 2);
    assert_eq!(arena.retained_members(&handle).unwrap(), [slot(1), slot(2)]);
    assert_eq!(arena.blocking_retentions(slot(2)), [handle.key()]);
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
}
#[test]
fn bad_second_source_returns_all_payloads_and_leaves_primary_reserved() {
    let cfg = cfg(vec![key("c").into()]);
    let mut arena = BindingArena::default();
    let mut owned = inputs(&mut arena, &cfg);
    owned[1].0.event.source = SourceToken(99);
    let rejected = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 1, owned)
        .err()
        .unwrap();
    assert_eq!(rejected.error, BindingError::Provenance);
    assert_eq!(rejected.inputs.len(), 2);
    assert_eq!(rejected.inputs[0].1.0, 10);
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_sources(), 2);
    assert_eq!(arena.shared_retention_references(), 0);
}
#[test]
fn shared_output_macro_early_up_keeps_duration_and_both_participants_until_completion() {
    let cfg = cfg(vec![
        Action {
            hold_down_ns: 50,
            ..key("c").into()
        },
        key("d").into(),
    ]);
    let mut arena = BindingArena::default();
    let owned = inputs(&mut arena, &cfg);
    let mut output = output(&cfg);
    let registration = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 2, owned)
        .unwrap();
    let handle = output
        .prepare_shared_child_scoped(plan(&cfg, 10, 110), registration)
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 2);
    assert_eq!(drain(&mut output, 110).0, [(key("c"), KeyEdge::Down)]);
    output.input_up(BatchId(10), MonoTime(120)).unwrap();
    assert!(drain(&mut output, 159).0.is_empty());
    assert!(matches!(
        arena.take(slot(1)),
        Err(BindingError::ChildPending)
    ));
    assert!(matches!(
        arena.take(slot(2)),
        Err(BindingError::ChildPending)
    ));
    let (edges, done) = drain(&mut output, 160);
    assert_eq!(
        edges,
        [
            (key("c"), KeyEdge::Up),
            (key("d"), KeyEdge::Down),
            (key("d"), KeyEdge::Up)
        ]
    );
    assert_eq!(done, [BatchId(10)]);
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
    assert_eq!(arena.take(slot(1)).unwrap().0, 10);
    assert_eq!(arena.take(slot(2)).unwrap().0, 20);
}
#[test]
fn output_guard_abort_and_drop_do_not_change_upstream_or_generated_variables() {
    let assign = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "flag".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: None,
            },
        }),
        ..key("vk_none").into()
    };
    let cfg = cfg(vec![assign, key("c").into()]);
    let mut arena = BindingArena::default();
    let owned = inputs(&mut arena, &cfg);
    let mut output = output(&cfg);
    let registration = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 2, owned)
        .unwrap();
    let returned = output
        .prepare_shared_child_scoped(plan(&cfg, 10, 110), registration)
        .unwrap()
        .abort();
    assert_eq!(returned.len(), 2);
    assert_eq!(arena.live_slots(), 0);
    assert!(output.generated_variables().is_empty());
    assert!(drain(&mut output, 110).0.is_empty());
    let registration = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 2, returned)
        .unwrap();
    drop(
        output
            .prepare_shared_child_scoped(plan(&cfg, 10, 110), registration)
            .unwrap(),
    );
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.shared_retention_references(), 0);
    assert!(output.generated_variables().is_empty());
}
#[test]
fn output_plan_slot_mismatch_rejects_before_any_participant_activation() {
    let cfg = cfg(vec![key("c").into()]);
    let mut arena = BindingArena::default();
    let owned = inputs(&mut arena, &cfg);
    let mut output = output(&cfg);
    let registration = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 2, owned)
        .unwrap();
    assert!(
        output
            .prepare_shared_child_scoped(plan(&cfg, 10, 110), registration)
            .is_err()
    );
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_sources(), 2);
    assert_eq!(arena.shared_retention_groups(), 0);
    assert!(drain(&mut output, 110).0.is_empty());
}
#[test]
fn shared_members_keep_old_new_arcs_and_primary_child_uses_only_primary_provenance() {
    let old = cfg(vec![key("c").into()]);
    let new = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    let mut arena = BindingArena::default();
    let first = root(&mut arena, &old, 1, 1, "a");
    let second = root(&mut arena, &new, 2, 2, "b");
    let handle = arena
        .prepare_shared_child_with_down(
            origin(&old, 10, 1, Stage::Output, 110),
            1,
            vec![(first, Payload(10)), (second, Payload(20))],
        )
        .unwrap()
        .commit();
    assert_eq!(arena.live_slots(), 2);
    assert!(matches!(
        arena.cancel_subtree(BatchId(2)),
        Err(BindingError::ChildPending)
    ));
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
    let second = arena.cancel_subtree(BatchId(2)).unwrap();
    assert!(Arc::ptr_eq(&second[0].origin.config, &new));
    assert_eq!(arena.take(slot(1)).unwrap().0, 10);
}
#[test]
fn duplicate_participant_and_late_peer_time_reject_without_partial_registration() {
    for duplicate in [false, true] {
        let cfg = cfg(vec![key("c").into()]);
        let mut arena = BindingArena::default();
        let mut owned = inputs(&mut arena, &cfg);
        if duplicate {
            owned[1].0.event = owned[0].0.event.clone();
        } else {
            owned[1].0.delivered_at = MonoTime(120);
        }
        let rejected = arena
            .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 1, owned)
            .err()
            .unwrap();
        assert_eq!(rejected.inputs.len(), 2);
        assert_eq!(arena.live_slots(), 0);
        assert_eq!(arena.reserved_sources(), 2);
        assert_eq!(arena.shared_retention_groups(), 0);
    }
}

#[test]
fn shared_output_variable_array_tracks_generated_and_actual_values_until_saved_up() {
    let assign = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "flag".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        hold_down_ns: 50,
        ..key("vk_none").into()
    };
    let cfg = cfg(vec![assign, key("c").into()]);
    let mut arena = BindingArena::default();
    let owned = inputs(&mut arena, &cfg);
    let mut output = output(&cfg);
    let registration = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 2, owned)
        .unwrap();
    let handle = output
        .prepare_shared_child_scoped(plan(&cfg, 10, 110), registration)
        .unwrap()
        .commit();
    assert_eq!(
        output.generated_variables()["flag"],
        VariableValue::Integer(2)
    );
    assert!(drain(&mut output, 110).0.is_empty());
    assert_eq!(
        output.executed_variables()["flag"],
        VariableValue::Integer(1)
    );
    output.input_up(BatchId(10), MonoTime(120)).unwrap();
    let (edges, done) = drain(&mut output, 160);
    assert_eq!(edges, [(key("c"), KeyEdge::Down), (key("c"), KeyEdge::Up)]);
    assert_eq!(
        output.executed_variables()["flag"],
        VariableValue::Integer(2)
    );
    assert_eq!(done, [BatchId(10)]);
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
}

#[test]
fn shared_old_output_plan_can_be_prepared_after_destination_reload_without_losing_arcs() {
    let old = cfg(vec![key("c").into()]);
    let mut arena = BindingArena::default();
    let owned = inputs(&mut arena, &old);
    let mut output = output(&old);
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let registration = arena
        .prepare_shared_child_with_down(origin(&old, 10, 1, Stage::Output, 110), 1, owned)
        .unwrap();
    let handle = output
        .prepare_shared_child_scoped(plan(&old, 10, 110), registration)
        .unwrap()
        .commit();
    assert_eq!(drain(&mut output, 110).0, [(key("c"), KeyEdge::Down)]);
    output.input_up(BatchId(10), MonoTime(120)).unwrap();
    assert_eq!(drain(&mut output, 120).0, [(key("c"), KeyEdge::Up)]);
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
}

#[test]
fn child_source_capacity_rejection_returns_participants_and_does_not_install_retention() {
    let cfg = cfg(vec![key("c").into()]);
    let mut arena = BindingArena::default();
    let owned = inputs(&mut arena, &cfg);
    arena
        .prepare(
            origin(&cfg, 3, 3, Stage::Complex, 100),
            MAX_OUTPUT_OWNERS - 2,
            None,
        )
        .unwrap()
        .commit();
    let rejected = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 1, owned)
        .err()
        .unwrap();
    assert_eq!(rejected.error, BindingError::Capacity);
    assert_eq!(rejected.inputs.len(), 2);
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.shared_retention_groups(), 0);
    arena.cancel(BatchId(3)).unwrap();
    let handle = arena
        .prepare_shared_child_with_down(origin(&cfg, 10, 1, Stage::Output, 110), 1, rejected.inputs)
        .unwrap()
        .commit();
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
}
