#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, complex_actor::*, generated_stage::*, modifier_scope::ScopedActionPlan,
    rule_selection::*, stage_bindings::BindingError, timeline::*, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn config() -> Arc<ConfigSnapshot> {
    let action = Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        ..key("vk_none").into()
    };
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "prepare-input".into(),
        simple: vec![Mapping {
            id: "simple".into(),
            from: key("a"),
            to: vec![action],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
        complex: vec![],
    })
}
fn actor(cfg: Arc<ConfigSnapshot>) -> ComplexStageActor {
    let mut actor = ComplexStageActor::new(cfg).unwrap();
    actor.observe_device(device(1), false, MonoTime(0)).unwrap();
    actor
}
fn queue(cfg: Arc<ConfigSnapshot>) -> GeneratedStageQueue {
    let mut queue = GeneratedStageQueue::new(Stage::Complex, cfg).unwrap();
    queue.observe_device(device(1), false).unwrap();
    queue.observe_device(device(9), false).unwrap();
    queue
}
fn scoped(cfg: Arc<ConfigSnapshot>, id: u64, source_device: u64) -> ScopedActionPlan {
    let rule = select_rule(cfg, MappingStage::Simple, key("a"), BTreeSet::new(), |_| {
        Truth::True
    })
    .unwrap();
    ScopedActionPlan::main(
        &rule,
        BatchId(id),
        MonoTime(100),
        device(source_device),
        SourceToken(100),
    )
    .unwrap()
}
fn raw(cfg: Arc<ConfigSnapshot>, id: u64, actions: &[Action], tap: bool) -> ActionPlan {
    plan_actions(
        actions,
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Complex,
            start: MonoTime(0),
            device: device(1),
            source: SourceToken(100),
            config: cfg,
        },
        tap,
    )
    .unwrap()
}
#[test]
fn abort_returns_original_plan_and_drop_preserves_sources_inputs_identity_and_snapshot() {
    let cfg = config();
    let mut actor = actor(cfg.clone());
    let original = scoped(cfg.clone(), 1, 1).actions().clone();
    let returned = actor.prepare_input(original.clone()).unwrap().abort();
    assert_eq!(returned.events()[0].at_ns, MonoTime(100));
    assert!(Arc::ptr_eq(&returned.events()[0].config, &cfg));
    assert_eq!(
        returned.events()[0].payload.target,
        original.events()[0].payload.target
    );
    {
        let _drop = actor.prepare_input(returned).unwrap();
    }
    assert_eq!(actor.reserved_sources(), 0);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.next_batch_id().unwrap(), BatchId(1));
    assert_eq!(actor.register_input(original).unwrap(), BatchId(1));
    assert_eq!(actor.reserved_sources(), 1);
}
#[test]
fn destination_device_rejection_drops_upstream_prepared_variable_and_same_id_stays_available() {
    let cfg = config();
    let mut source = queue(cfg.clone());
    let mut dest = actor(cfg.clone());
    let plan = scoped(cfg.clone(), 1, 9);
    {
        let _generated = source.prepare_scoped(&plan).unwrap();
        assert!(dest.prepare_input(plan.actions().clone()).is_err());
    }
    assert!(source.variable_values().is_empty());
    assert_eq!(source.queued_events(), 0);
    assert_eq!(source.reserved_slots(), 0);
    assert_eq!(dest.next_batch_id().unwrap(), BatchId(1));
    assert_eq!(dest.reserved_sources(), 0);
    let plan = scoped(cfg, 1, 1);
    let generated = source.prepare_scoped(&plan).unwrap();
    let registration = dest.prepare_input(plan.actions().clone()).unwrap();
    registration.commit();
    generated.commit();
    assert_eq!(
        source.variable_values().get("v"),
        Some(&VariableValue::Integer(1))
    );
    assert_eq!(dest.reserved_sources(), 1);
}
#[test]
fn source_timeline_rejection_aborts_already_prepared_downstream_registration() {
    let cfg = config();
    let mut source = queue(cfg.clone());
    let mut dest = actor(cfg.clone());
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 10..18 {
        source
            .enqueue(raw(cfg.clone(), id, &actions, true).events().to_vec())
            .unwrap();
    }
    let plan = scoped(cfg, 1, 1);
    {
        let _registration = dest.prepare_input(plan.actions().clone()).unwrap();
        assert!(matches!(
            source.prepare_scoped(&plan),
            Err(GeneratedError::Timeline(TimelineError::Capacity))
        ));
    }
    assert_eq!(dest.reserved_sources(), 0);
    assert_eq!(dest.pending_inputs(), 0);
    assert_eq!(dest.next_batch_id().unwrap(), BatchId(1));
    assert!(source.variable_values().is_empty());
    assert_eq!(source.queued_events(), MAX_STAGE_EVENTS);
}
#[test]
fn destination_source_budget_rejection_drops_upstream_generation_without_ghost_variables() {
    let cfg = config();
    let mut source = queue(cfg.clone());
    let mut dest = actor(cfg.clone());
    let actions = vec![key("b").into(); 512];
    for id in 1..=32 {
        dest.register_input(raw(cfg.clone(), id, &actions, false))
            .unwrap();
    }
    assert_eq!(dest.reserved_sources(), MAX_OUTPUT_OWNERS);
    let plan = scoped(cfg, 33, 1);
    {
        let _generated = source.prepare_scoped(&plan).unwrap();
        assert!(matches!(
            dest.prepare_input(plan.actions().clone()),
            Err(ComplexActorError::Binding(BindingError::Capacity))
        ));
    }
    assert!(source.variable_values().is_empty());
    assert_eq!(source.reserved_slots(), 0);
    assert_eq!(dest.next_batch_id().unwrap(), BatchId(33));
    assert_eq!(dest.pending_inputs(), 32);
}
#[test]
fn successful_two_queue_registration_drives_real_complex_down_and_original_up_to_output_completion()
{
    let cfg = config();
    let mut source = queue(cfg.clone());
    let mut dest = actor(cfg.clone());
    let plan = scoped(cfg, 1, 1);
    let generated = source.prepare_scoped(&plan).unwrap();
    let registration = dest.prepare_input(plan.actions().clone()).unwrap();
    registration.commit();
    let mut saved = generated.commit();
    let down = source.poll(MonoTime(100)).unwrap().unwrap();
    dest.admit(&down, &ContextSnapshot::default()).unwrap();
    while dest.poll(MonoTime(100)).unwrap().is_some() {}
    source.input_up_scoped(&mut saved, MonoTime(110)).unwrap();
    let up = source.poll(MonoTime(110)).unwrap().unwrap();
    dest.admit(&up, &ContextSnapshot::default()).unwrap();
    while dest.poll(MonoTime(110)).unwrap().is_some() {}
    source.retire_batch(BatchId(1)).unwrap();
    assert_eq!(source.reserved_slots(), 0);
    assert_eq!(dest.reserved_sources(), 0);
    assert_eq!(dest.pending_inputs(), 0);
    assert_eq!(
        source.variable_values().get("v"),
        Some(&VariableValue::Integer(2))
    );
    assert_eq!(
        dest.generated_variables().get("v"),
        Some(&VariableValue::Integer(2))
    );
}
#[test]
fn empty_registration_abort_does_not_consume_identity_but_commit_does_without_fake_root() {
    let cfg = config();
    let mut dest = actor(cfg.clone());
    {
        let _drop = dest.prepare_input(raw(cfg.clone(), 1, &[], false)).unwrap();
    }
    assert_eq!(dest.next_batch_id().unwrap(), BatchId(1));
    dest.prepare_input(raw(cfg, 1, &[], false))
        .unwrap()
        .commit();
    assert_eq!(dest.next_batch_id().unwrap(), BatchId(2));
    assert_eq!(dest.pending_inputs(), 0);
    assert_eq!(dest.reserved_sources(), 0);
}
