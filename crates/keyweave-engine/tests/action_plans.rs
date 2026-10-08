#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn action(name: &str, hold: u64) -> Action {
    Action {
        hold_down_ns: hold,
        ..key(name).into()
    }
}
fn origin(start: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(1),
        stage: Stage::Complex,
        start: MonoTime(start),
        device: DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        source: SourceToken(100),
        config: Arc::new(ConfigSnapshot::empty(ConfigGeneration(7))),
    }
}
fn summary(plan: &ActionPlan) -> Vec<(u64, String, KeyEdge)> {
    plan.events()
        .iter()
        .map(|event| {
            let name = match &event.payload.target {
                ActionTarget::Key(key) => key.name(),
                ActionTarget::SetVariable(_) => "variable".into(),
            };
            (event.at_ns.0, name, event.payload.edge)
        })
        .collect()
}

#[test]
fn duration_advances_tapped_up_and_all_later_actions_but_held_last_waits_for_input_up() {
    let plan = plan_actions(&[action("b", 50), action("c", 200)], origin(10), false).unwrap();
    assert_eq!(
        summary(&plan),
        [
            (10, "b".into(), KeyEdge::Down),
            (60, "b".into(), KeyEdge::Up),
            (60, "c".into(), KeyEdge::Down)
        ]
    );
    assert_eq!(plan.ends_at(), MonoTime(60));
    assert_eq!(plan.releases().len(), 1);
    let up = plan.input_up_events(MonoTime(30));
    assert_eq!(up[0].at_ns, MonoTime(60));
    assert_eq!(up[0].payload.target, ActionTarget::Key(key("c")));
    assert_eq!(up[0].payload.edge, KeyEdge::Up);
    assert_eq!(plan.events()[2].payload.slot, up[0].payload.slot);
}
#[test]
fn all_extra_actions_are_taps_and_duration_sums_instead_of_running_in_parallel() {
    let plan = plan_actions(&[action("b", 50), action("c", 200)], origin(10), true).unwrap();
    assert_eq!(
        summary(&plan),
        [
            (10, "b".into(), KeyEdge::Down),
            (60, "b".into(), KeyEdge::Up),
            (60, "c".into(), KeyEdge::Down),
            (260, "c".into(), KeyEdge::Up)
        ]
    );
    assert!(plan.releases().is_empty());
    assert_eq!(plan.ends_at(), MonoTime(260));
}
#[test]
fn repeat_false_last_action_is_tapped_at_its_duration() {
    let mut a = action("b", 5);
    a.repeat = false;
    let plan = plan_actions(&[a], origin(0), false).unwrap();
    assert_eq!(
        summary(&plan),
        [(0, "b".into(), KeyEdge::Down), (5, "b".into(), KeyEdge::Up)]
    );
    assert!(plan.releases().is_empty());
}
#[test]
fn scope_up_waits_for_tapped_main_up_and_next_action_reuses_no_slots() {
    let mut a = action("b", 50);
    a.modifiers = vec![key("left_control"), key("left_shift")];
    let plan = plan_actions(&[a, action("c", 0)], origin(0), false).unwrap();
    assert_eq!(
        summary(&plan),
        [
            (0, "left_control".into(), KeyEdge::Down),
            (0, "left_shift".into(), KeyEdge::Down),
            (0, "b".into(), KeyEdge::Down),
            (50, "b".into(), KeyEdge::Up),
            (50, "left_control".into(), KeyEdge::Up),
            (50, "left_shift".into(), KeyEdge::Up),
            (50, "c".into(), KeyEdge::Down)
        ]
    );
    assert!(plan.events()[0].payload.lazy && plan.events()[1].payload.lazy);
    assert_eq!(plan.events()[0].payload.slot, plan.events()[4].payload.slot);
    assert_ne!(plan.events()[2].payload.slot, plan.events()[6].payload.slot);
}
#[test]
fn last_modifier_scopes_survive_repeat_false_main_tap_until_original_up() {
    let mut a = action("left_control", 50);
    a.repeat = false;
    a.modifiers = vec![key("left_shift")];
    let plan = plan_actions(&[a], origin(0), false).unwrap();
    assert_eq!(
        summary(&plan),
        [
            (0, "left_shift".into(), KeyEdge::Down),
            (0, "left_control".into(), KeyEdge::Down),
            (50, "left_control".into(), KeyEdge::Up)
        ]
    );
    assert!(!plan.events()[0].payload.lazy);
    assert_eq!(plan.releases().len(), 1);
    assert_eq!(
        plan.releases()[0].target,
        ActionTarget::Key(key("left_shift"))
    );
    assert!(plan.releases()[0].lazy);
}
#[test]
fn explicit_lazy_and_halt_are_main_down_options_and_do_not_truncate_current_array() {
    let mut a = action("left_control", 1);
    a.lazy = true;
    a.halt = true;
    a.modifiers = vec![key("left_shift")];
    let plan = plan_actions(&[a, action("b", 0)], origin(0), true).unwrap();
    assert_eq!(plan.events().len(), 6);
    assert!(plan.events()[0].payload.lazy);
    assert!(plan.events()[1].payload.lazy && plan.events()[1].payload.halt);
    assert!(plan.events()[2].payload.lazy && !plan.events()[2].payload.halt);
    assert_eq!(plan.events()[4].payload.target, ActionTarget::Key(key("b")));
}
#[test]
fn variable_down_and_up_share_slot_and_preserve_typed_up_value() {
    let mut a = action("b", 50);
    a.target = ActionTarget::SetVariable(VariableAction {
        name: "mode".into(),
        operation: VariableOperation::Set {
            value: Some(VariableValue::Boolean(true)),
            key_up_value: Some(VariableValue::Integer(0)),
        },
    });
    let plan = plan_actions(&[a.clone()], origin(10), true).unwrap();
    assert_eq!(plan.events()[0].payload.target, a.target);
    assert_eq!(plan.events()[1].payload.target, a.target);
    assert_eq!(plan.events()[0].payload.slot, plan.events()[1].payload.slot);
    assert_eq!(plan.events()[1].at_ns, MonoTime(60));
}
#[test]
fn plan_runs_through_timeline_at_each_scheduled_time_with_original_provenance() {
    let source = origin(0);
    let plan = plan_actions(&[action("b", 50), action("c", 0)], source.clone(), false).unwrap();
    let mut queue = StageTimeline::new();
    queue.observe_device(source.device, false).unwrap();
    queue.enqueue(plan.events().to_vec()).unwrap();
    let down = queue.pop_due(MonoTime(0)).unwrap().unwrap();
    assert_eq!(down.event.payload.target, ActionTarget::Key(key("b")));
    assert!(queue.pop_due(MonoTime(49)).unwrap().is_none());
    let up = queue.pop_due(MonoTime(50)).unwrap().unwrap();
    assert_eq!(up.event.payload.edge, KeyEdge::Up);
    let next = queue.pop_due(MonoTime(50)).unwrap().unwrap();
    assert_eq!(next.event.payload.target, ActionTarget::Key(key("c")));
    assert_eq!(next.event.source, source.source);
    assert!(Arc::ptr_eq(&next.event.config, &source.config));
    queue.enqueue(plan.input_up_events(MonoTime(60))).unwrap();
    assert_eq!(
        queue
            .pop_due(MonoTime(60))
            .unwrap()
            .unwrap()
            .event
            .payload
            .edge,
        KeyEdge::Up
    );
}
#[test]
fn invalid_time_capacity_and_scope_fail_before_any_plan_is_returned() {
    assert_eq!(
        plan_actions(&[action("b", MAX_ACTION_DURATION_NS + 1)], origin(0), false).unwrap_err(),
        PlanError::InvalidDuration
    );
    assert_eq!(
        plan_actions(&[action("b", 10)], origin(u64::MAX - 5), true).unwrap_err(),
        PlanError::TimeOverflow
    );
    assert_eq!(
        plan_actions(&vec![action("b", 0); 513], origin(0), true).unwrap_err(),
        PlanError::Capacity
    );
    let mut a = action("b", 0);
    a.modifiers = vec![key("a")];
    assert_eq!(
        plan_actions(&[a], origin(0), false).unwrap_err(),
        PlanError::InvalidScope
    );
    // Duration of a held-last action never advances time, including at MAX.
    assert!(plan_actions(&[action("b", 10)], origin(u64::MAX), false).is_ok());
}
#[test]
fn empty_plan_has_no_phantom_outputs_or_original_up() {
    let plan = plan_actions(&[], origin(5), true).unwrap();
    assert!(plan.events().is_empty() && plan.releases().is_empty());
    assert_eq!(plan.ends_at(), MonoTime(5));
}
#[test]
fn duration_ir_cannot_silently_execute_on_the_recursive_engine() {
    let mut cfg = ConfigSnapshot::empty(ConfigGeneration(1));
    cfg.complex.push(Mapping {
        id: "timed".into(),
        from: key("a"),
        to: vec![action("b", 50)],
        allow_extra_modifiers: true,
        extras: Default::default(),
    });
    assert!(matches!(
        Engine::new(Arc::new(cfg.clone())),
        Err(EngineError::UnsupportedActionTiming)
    ));
    let mut engine = Engine::new(Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))).unwrap();
    cfg.generation = ConfigGeneration(2);
    assert_eq!(
        engine.activate(Arc::new(cfg)),
        Err(EngineError::UnsupportedActionTiming)
    );
    assert_eq!(engine.state().bindings, 0);
}

#[test]
fn separate_batches_from_same_physical_source_have_distinct_saved_slots() {
    let a = plan_actions(&[action("b", 0)], origin(0), false).unwrap();
    let mut next = origin(0);
    next.batch = BatchId(2);
    let b = plan_actions(&[action("b", 0)], next, false).unwrap();
    assert_eq!(a.events()[0].source, b.events()[0].source);
    assert_ne!(a.events()[0].payload.slot, b.events()[0].payload.slot);
    assert_eq!(a.events()[0].payload.slot, a.releases()[0].slot);
}
