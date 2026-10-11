#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::*, modifier_scope::ScopedActionPlan, rule_selection::*,
    terminal_actor::*, timeline::Stage, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn variable() -> Action {
    Action {
        target: ActionTarget::SetVariable(VariableAction {
            name: "v".into(),
            operation: VariableOperation::Set {
                value: Some(VariableValue::Integer(1)),
                key_up_value: Some(VariableValue::Integer(2)),
            },
        }),
        hold_down_ns: 50,
        ..key("b").into()
    }
}
fn config() -> Arc<ConfigSnapshot> {
    let mut extras = MappingExtras::default();
    extras.from_modifiers.mandatory.insert(Modifier::Shift);
    extras.if_alone = vec![variable()];
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "paired-preflight".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key("right_control").into()],
            allow_extra_modifiers: true,
            extras,
        }],
    })
}
fn pair() -> (GeneratedStageQueue, TerminalActor) {
    let config = config();
    let mut generated = GeneratedStageQueue::new(Stage::Output, config.clone()).unwrap();
    let mut terminal = TerminalActor::new(config).unwrap();
    for id in [1, 2] {
        generated.observe_device(device(id), false).unwrap();
        terminal
            .observe_device(device(id), false, MonoTime(0))
            .unwrap();
    }
    (generated, terminal)
}
fn plain(
    config: Arc<ConfigSnapshot>,
    id: u64,
    start: u64,
    device_id: u64,
    actions: &[Action],
    tap: bool,
) -> ActionPlan {
    plan_actions(
        actions,
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(start),
            device: device(device_id),
            source: SourceToken(100),
            config,
        },
        tap,
    )
    .unwrap()
}
fn shift(generated: &mut GeneratedStageQueue, terminal: &mut TerminalActor) {
    let plan = plain(
        generated.snapshot().clone(),
        1,
        0,
        2,
        &[key("left_shift").into()],
        false,
    );
    let g = generated.prepare_enqueue(plan.events().to_vec()).unwrap();
    let t = terminal.prepare(plan).unwrap();
    g.commit();
    t.commit();
}
fn rule(generated: &GeneratedStageQueue) -> RuleSelection {
    generated
        .select(
            key("a"),
            &ContextSnapshot::default(),
            device(1),
            MonoTime(0),
        )
        .unwrap()
        .unwrap()
}
fn main(rule: &RuleSelection, id: u64) -> ScopedActionPlan {
    ScopedActionPlan::main(
        rule,
        BatchId(id),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap()
}
fn drain(terminal: &mut TerminalActor, at: u64) -> Vec<(String, KeyEdge)> {
    let mut outputs = vec![];
    while let Some(step) = terminal.poll(MonoTime(at)).unwrap() {
        outputs.extend(step.outputs.into_iter().map(|o| (o.key.name(), o.edge)));
    }
    outputs
}
fn delivered(generated: &mut GeneratedStageQueue, at: u64) {
    while generated.poll(MonoTime(at)).unwrap().is_some() {}
}
#[test]
fn raw_abort_does_not_consume_batch_identity_reserve_slots_or_emit_output() {
    let (_, mut terminal) = pair();
    let canceled = plain(
        terminal.snapshot().clone(),
        100,
        100,
        1,
        &[variable()],
        true,
    );
    terminal.prepare(canceled).unwrap().abort();
    assert_eq!(terminal.queued_events(), 0);
    assert_eq!(terminal.reserved_slots(), 0);
    assert_eq!(terminal.pending_batches(), 0);
    assert!(terminal.variable_values().is_empty());
    assert!(drain(&mut terminal, 0).is_empty());
    let plan = plain(
        terminal.snapshot().clone(),
        1,
        100,
        1,
        &[key("b").into()],
        false,
    );
    terminal.prepare(plan).unwrap().commit();
    assert_eq!(terminal.live_slots(), 0);
    assert!(drain(&mut terminal, 99).is_empty());
    assert_eq!(drain(&mut terminal, 100), [("b".into(), KeyEdge::Down)]);
    terminal.input_up(BatchId(1), MonoTime(110)).unwrap();
    assert_eq!(drain(&mut terminal, 110), [("b".into(), KeyEdge::Up)]);
    assert_eq!(terminal.reserved_slots(), 0);
}
#[test]
fn terminal_device_rejection_does_not_commit_prepared_generated_scope() {
    let (mut generated, _) = pair();
    let mut terminal = TerminalActor::new(generated.snapshot().clone()).unwrap();
    terminal
        .observe_device(device(2), false, MonoTime(0))
        .unwrap();
    shift(&mut generated, &mut terminal);
    let selected = rule(&generated);
    let planned = main(&selected, 2);
    let g = generated.prepare_scoped(&planned).unwrap();
    assert!(matches!(
        terminal.prepare_scoped(planned),
        Err(ActorError::Timeline(timeline::TimelineError::RetiredDevice))
    ));
    drop(g);
    assert_eq!(generated.modifiers(), BTreeSet::from([key("left_shift")]));
    assert_eq!(generated.hidden_modifier_references(), 0);
    assert_eq!(generated.pending_batches(), 1);
    assert_eq!(terminal.pending_batches(), 1);
    assert_eq!(terminal.reserved_slots(), 1);
    terminal
        .observe_device(device(1), false, MonoTime(0))
        .unwrap();
    let planned = main(&selected, 2);
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let _saved = g.commit();
    t.commit();
    assert_eq!(generated.hidden_modifier_references(), 1);
    assert_eq!(terminal.hidden_modifier_references(), 0);
    assert_eq!(terminal.live_slots(), 0);
}
#[test]
fn generated_identity_rejection_aborts_prepared_terminal_without_variable_execution() {
    let (mut generated, mut terminal) = pair();
    let plan = plain(
        generated.snapshot().clone(),
        50,
        100,
        1,
        &[variable()],
        true,
    );
    generated.enqueue(plan.events().to_vec()).unwrap();
    let t = terminal.prepare(plan.clone()).unwrap();
    assert!(matches!(
        generated.prepare_enqueue(plan.events().to_vec()),
        Err(RejectedGeneratedBatch {
            error: GeneratedError::Lifecycle,
            ..
        })
    ));
    drop(t);
    assert_eq!(terminal.pending_batches(), 0);
    assert_eq!(terminal.reserved_slots(), 0);
    assert!(terminal.variable_values().is_empty());
    // A lower ID is accepted: the aborted reservation did not advance last_batch.
    let retry = plain(terminal.snapshot().clone(), 1, 100, 1, &[variable()], true);
    terminal.prepare(retry).unwrap().commit();
    assert!(terminal.variable_values().is_empty());
    assert!(drain(&mut terminal, 100).is_empty());
    assert_eq!(terminal.variable_values()["v"], VariableValue::Integer(1));
    assert!(drain(&mut terminal, 149).is_empty());
    drain(&mut terminal, 150);
    assert_eq!(terminal.variable_values()["v"], VariableValue::Integer(2));
    assert_eq!(terminal.pending_batches(), 0);
}
#[test]
fn paired_main_commits_generated_scope_then_executes_and_releases_saved_actual_owners() {
    let (mut generated, mut terminal) = pair();
    shift(&mut generated, &mut terminal);
    assert_eq!(
        drain(&mut terminal, 0),
        [("left_shift".into(), KeyEdge::Down)]
    );
    delivered(&mut generated, 0);
    let planned = main(&rule(&generated), 2);
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let mut saved = g.commit();
    t.commit();
    assert_eq!(
        generated.modifiers(),
        BTreeSet::from([key("right_control")])
    );
    assert_eq!(terminal.hidden_modifier_references(), 0);
    assert!(drain(&mut terminal, 99).is_empty());
    assert_eq!(
        drain(&mut terminal, 100),
        [
            ("left_shift".into(), KeyEdge::Up),
            ("right_control".into(), KeyEdge::Down)
        ]
    );
    delivered(&mut generated, 100);
    assert_eq!(terminal.hidden_modifier_references(), 1);
    generated
        .input_up_scoped(&mut saved, MonoTime(110))
        .unwrap();
    terminal.input_up(BatchId(2), MonoTime(110)).unwrap();
    assert_eq!(generated.modifiers(), BTreeSet::from([key("left_shift")]));
    assert_eq!(
        drain(&mut terminal, 110),
        [
            ("right_control".into(), KeyEdge::Up),
            ("left_shift".into(), KeyEdge::Down)
        ]
    );
    delivered(&mut generated, 110);
    generated.retire_batch(BatchId(2)).unwrap();
    assert_eq!(terminal.hidden_modifier_references(), 0);
    assert_eq!(terminal.live_slots(), 1);
    assert_eq!(terminal.pending_batches(), 1);
}
#[test]
fn paired_snapshot_variable_preserves_generation_view_and_delayed_actual_up_distinction() {
    let (mut generated, mut terminal) = pair();
    shift(&mut generated, &mut terminal);
    drain(&mut terminal, 0);
    delivered(&mut generated, 0);
    let branch = rule(&generated)
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    let planned = ScopedActionPlan::branch(
        &branch,
        BatchId(2),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap();
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let _saved = g.commit();
    t.commit();
    assert_eq!(generated.variable_values()["v"], VariableValue::Integer(2));
    assert!(terminal.variable_values().is_empty());
    assert_eq!(terminal.reserved_slots(), 3);
    drain(&mut terminal, 100);
    delivered(&mut generated, 100);
    assert_eq!(terminal.variable_values()["v"], VariableValue::Integer(1));
    assert_eq!(terminal.live_slots(), 2);
    drain(&mut terminal, 150);
    delivered(&mut generated, 150);
    assert_eq!(terminal.variable_values()["v"], VariableValue::Integer(2));
    generated.retire_batch(BatchId(2)).unwrap();
    assert_eq!(terminal.reserved_slots(), 1);
    assert_eq!(generated.reserved_slots(), 1);
}
#[test]
fn aborted_empty_plan_does_not_advance_identity_but_committed_empty_plan_does() {
    let (_, mut terminal) = pair();
    let empty = plain(terminal.snapshot().clone(), 100, 0, 1, &[], false);
    terminal.prepare(empty).unwrap().abort();
    let empty = plain(terminal.snapshot().clone(), 1, 0, 1, &[], false);
    terminal.prepare(empty).unwrap().commit();
    assert_eq!(terminal.pending_batches(), 0);
    assert_eq!(terminal.reserved_slots(), 0);
    let duplicate = plain(
        terminal.snapshot().clone(),
        1,
        0,
        1,
        &[key("b").into()],
        false,
    );
    assert!(matches!(
        terminal.prepare(duplicate),
        Err(ActorError::InvalidBatch)
    ));
}
#[test]
fn cancel_pending_paired_scope_does_not_invent_actual_key_up_or_open_snapshot() {
    let (mut generated, mut terminal) = pair();
    shift(&mut generated, &mut terminal);
    drain(&mut terminal, 0);
    delivered(&mut generated, 0);
    let planned = main(&rule(&generated), 2);
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let _saved = g.commit();
    t.commit();
    generated.cancel_batch(BatchId(2));
    assert!(
        terminal
            .cancel_batch(BatchId(2), MonoTime(10))
            .unwrap()
            .is_empty()
    );
    assert_eq!(generated.hidden_modifier_references(), 0);
    assert_eq!(terminal.hidden_modifier_references(), 0);
    assert_eq!(terminal.pressed_outputs(), [key("left_shift")]);
    assert_eq!(terminal.reserved_slots(), 1);
    assert_eq!(generated.reserved_slots(), 1);
    assert!(drain(&mut terminal, 100).is_empty());
}

#[test]
fn full_terminal_queue_rejection_does_not_assign_prepared_generated_variable() {
    let (mut generated, mut terminal) = pair();
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 1..=8 {
        let plan = plain(terminal.snapshot().clone(), id, 0, 1, &actions, true);
        terminal.enqueue(plan).unwrap();
    }
    assert_eq!(terminal.queued_events(), timeline::MAX_STAGE_EVENTS);
    let plan = plain(generated.snapshot().clone(), 9, 100, 1, &[variable()], true);
    let g = generated.prepare_enqueue(plan.events().to_vec()).unwrap();
    assert!(matches!(
        terminal.prepare(plan.clone()),
        Err(ActorError::Timeline(timeline::TimelineError::Capacity))
    ));
    drop(g);
    assert!(generated.variable_values().is_empty());
    assert_eq!(generated.pending_batches(), 0);
    assert_eq!(generated.reserved_slots(), 0);
    assert_eq!(terminal.pending_batches(), 8);
    assert_eq!(terminal.reserved_slots(), 4096);
    assert!(
        terminal
            .cancel_batch(BatchId(1), MonoTime(0))
            .unwrap()
            .is_empty()
    );
    let g = generated.prepare_enqueue(plan.events().to_vec()).unwrap();
    let t = terminal.prepare(plan).unwrap();
    g.commit();
    t.commit();
    assert_eq!(generated.variable_values()["v"], VariableValue::Integer(2));
    assert!(terminal.variable_values().is_empty());
    assert_eq!(terminal.pending_batches(), 8);
    assert_eq!(terminal.reserved_slots(), 3585);
}

fn active_main() -> (GeneratedStageQueue, TerminalActor, ScopedGeneration) {
    let (mut generated, mut terminal) = pair();
    shift(&mut generated, &mut terminal);
    drain(&mut terminal, 0);
    delivered(&mut generated, 0);
    let planned = main(&rule(&generated), 2);
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let saved = g.commit();
    t.commit();
    drain(&mut terminal, 100);
    delivered(&mut generated, 100);
    (generated, terminal, saved)
}
#[test]
fn aborted_paired_up_keeps_scope_recipe_and_clock_then_retries_at_earlier_time() {
    let (mut generated, mut terminal, mut saved) = active_main();
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(200))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(200))
        .unwrap()
        .unwrap();
    g.abort();
    t.abort();
    assert_eq!(generated.hidden_modifier_references(), 1);
    assert_eq!(terminal.hidden_modifier_references(), 1);
    assert_eq!(generated.queued_events(), 0);
    assert_eq!(terminal.queued_events(), 0);
    assert_eq!(
        generated
            .delivery_progress(BatchId(2))
            .unwrap()
            .up_generated,
        0
    );
    assert_eq!(terminal.live_slots(), 2);
    // 150 remains valid after preparing and aborting 200.
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(150))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(150))
        .unwrap()
        .unwrap();
    g.commit();
    t.commit();
    assert_eq!(generated.hidden_modifier_references(), 0);
    assert_eq!(terminal.hidden_modifier_references(), 1);
    assert_eq!(
        drain(&mut terminal, 150),
        [
            ("right_control".into(), KeyEdge::Up),
            ("left_shift".into(), KeyEdge::Down)
        ]
    );
    delivered(&mut generated, 150);
    generated.retire_batch(BatchId(2)).unwrap();
    assert!(
        generated
            .prepare_input_up_scoped(&mut saved, MonoTime(150))
            .unwrap()
            .is_none()
    );
    assert!(
        terminal
            .prepare_input_up(BatchId(2), MonoTime(150))
            .unwrap()
            .is_none()
    );
}
#[test]
fn terminal_due_rejection_does_not_close_prepared_generated_up_and_drain_allows_retry() {
    let (mut generated, mut terminal) = pair();
    shift(&mut generated, &mut terminal);
    drain(&mut terminal, 0);
    delivered(&mut generated, 0);
    let planned = main(&rule(&generated), 2);
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let mut saved = g.commit();
    t.commit();
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(100))
        .unwrap()
        .unwrap();
    assert!(matches!(
        terminal.prepare_input_up(BatchId(2), MonoTime(100)),
        Err(ActorError::Engine(EngineError::DeadlinePending))
    ));
    drop(g);
    assert_eq!(generated.hidden_modifier_references(), 1);
    assert_eq!(
        generated
            .delivery_progress(BatchId(2))
            .unwrap()
            .up_generated,
        0
    );
    assert_eq!(terminal.live_slots(), 1);
    drain(&mut terminal, 100);
    delivered(&mut generated, 100);
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(100))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(100))
        .unwrap()
        .unwrap();
    g.commit();
    t.commit();
    drain(&mut terminal, 100);
    delivered(&mut generated, 100);
    assert_eq!(terminal.hidden_modifier_references(), 0);
    generated.retire_batch(BatchId(2)).unwrap();
}
#[test]
fn terminal_full_up_rejection_preserves_generated_scope_and_saved_up_for_retry() {
    let (mut generated, mut terminal, mut saved) = active_main();
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 3..=10 {
        terminal
            .enqueue(plain(
                terminal.snapshot().clone(),
                id,
                200,
                1,
                &actions,
                true,
            ))
            .unwrap();
    }
    assert_eq!(terminal.queued_events(), 8192);
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(110))
        .unwrap()
        .unwrap();
    assert!(matches!(
        terminal.prepare_input_up(BatchId(2), MonoTime(110)),
        Err(ActorError::Timeline(timeline::TimelineError::Capacity))
    ));
    drop(g);
    assert_eq!(generated.hidden_modifier_references(), 1);
    assert_eq!(
        generated
            .delivery_progress(BatchId(2))
            .unwrap()
            .up_generated,
        0
    );
    assert_eq!(terminal.hidden_modifier_references(), 1);
    terminal.cancel_batch(BatchId(3), MonoTime(105)).unwrap();
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(110))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(110))
        .unwrap()
        .unwrap();
    g.commit();
    t.commit();
    assert_eq!(generated.hidden_modifier_references(), 0);
    // Actor FIFO puts this actual Up behind remaining future work.
    assert_eq!(terminal.hidden_modifier_references(), 1);
    assert!(drain(&mut terminal, 110).is_empty());
    delivered(&mut generated, 110);
    terminal.cancel_batch(BatchId(2), MonoTime(111)).unwrap();
    assert_eq!(terminal.hidden_modifier_references(), 0);
    generated.retire_batch(BatchId(2)).unwrap();
}
#[test]
fn generated_full_up_rejection_aborts_terminal_up_marker_and_clock() {
    let (mut generated, mut terminal, mut saved) = active_main();
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("b").into()
        };
        512
    ];
    for id in 3..=10 {
        let plan = plain(generated.snapshot().clone(), id, 200, 1, &actions, true);
        generated.enqueue(plan.events().to_vec()).unwrap();
    }
    assert_eq!(generated.queued_events(), 8192);
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(110))
        .unwrap()
        .unwrap();
    assert!(matches!(
        generated.prepare_input_up_scoped(&mut saved, MonoTime(110)),
        Err(GeneratedError::Timeline(timeline::TimelineError::Capacity))
    ));
    drop(t);
    assert_eq!(terminal.queued_events(), 0);
    assert_eq!(terminal.hidden_modifier_references(), 1);
    assert_eq!(generated.hidden_modifier_references(), 1);
    generated.cancel_batch(BatchId(3));
    // 105 confirms that aborted preparation at 110 advanced neither clock.
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(105))
        .unwrap()
        .unwrap();
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(105))
        .unwrap()
        .unwrap();
    t.commit();
    g.commit();
    drain(&mut terminal, 105);
    assert_eq!(terminal.hidden_modifier_references(), 0);
    assert_eq!(generated.hidden_modifier_references(), 0);
    assert_eq!(
        generated
            .delivery_progress(BatchId(2))
            .unwrap()
            .up_generated,
        1
    );
    // Generation FIFO can still delay delivery; it is not actual execution proof.
    assert_eq!(
        generated
            .delivery_progress(BatchId(2))
            .unwrap()
            .up_delivered,
        0
    );
}
#[test]
fn old_arc_variable_up_preparation_abort_does_not_assign_then_commit_keeps_timed_execution() {
    let mut configuration = (*config()).clone();
    configuration.complex[0].to = vec![variable()];
    configuration.complex[0].extras.from_modifiers = ModifierFilter::default();
    let configuration = Arc::new(configuration);
    let mut generated = GeneratedStageQueue::new(Stage::Output, configuration.clone()).unwrap();
    let mut terminal = TerminalActor::new(configuration.clone()).unwrap();
    generated.observe_device(device(1), false).unwrap();
    terminal
        .observe_device(device(1), false, MonoTime(0))
        .unwrap();
    let planned = main(&rule(&generated), 1);
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let mut saved = g.commit();
    t.commit();
    drain(&mut terminal, 100);
    delivered(&mut generated, 100);
    let next = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
    generated.activate(next.clone()).unwrap();
    terminal.activate(next).unwrap();
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(200))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(1), MonoTime(200))
        .unwrap()
        .unwrap();
    drop(g);
    drop(t);
    for values in [generated.variable_values(), terminal.variable_values()] {
        assert_eq!(values["v"], VariableValue::Integer(1));
    }
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(110))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(1), MonoTime(110))
        .unwrap()
        .unwrap();
    g.commit();
    t.commit();
    assert_eq!(generated.variable_values()["v"], VariableValue::Integer(2));
    assert_eq!(terminal.variable_values()["v"], VariableValue::Integer(1));
    drain(&mut terminal, 110);
    let event = generated.poll(MonoTime(110)).unwrap().unwrap();
    assert!(Arc::ptr_eq(&event.event.config, &configuration));
    assert_eq!(terminal.variable_values()["v"], VariableValue::Integer(2));
    generated.retire_batch(BatchId(1)).unwrap();
}
#[test]
fn tapped_last_modifier_prepares_only_actual_scope_marker_after_all_key_ups_delivered() {
    let mut configuration = (*config()).clone();
    configuration.complex[0].to[0].repeat = false;
    configuration.complex[0].to[0].hold_down_ns = 50;
    let configuration = Arc::new(configuration);
    let mut generated = GeneratedStageQueue::new(Stage::Output, configuration.clone()).unwrap();
    let mut terminal = TerminalActor::new(configuration).unwrap();
    for id in [1, 2] {
        generated.observe_device(device(id), false).unwrap();
        terminal
            .observe_device(device(id), false, MonoTime(0))
            .unwrap();
    }
    shift(&mut generated, &mut terminal);
    drain(&mut terminal, 0);
    delivered(&mut generated, 0);
    let planned = main(&rule(&generated), 2);
    assert!(planned.actions().releases().is_empty());
    let g = generated.prepare_scoped(&planned).unwrap();
    let t = terminal.prepare_scoped(planned).unwrap();
    let mut saved = g.commit();
    t.commit();
    drain(&mut terminal, 100);
    delivered(&mut generated, 100);
    drain(&mut terminal, 150);
    delivered(&mut generated, 150);
    assert_eq!(
        generated
            .delivery_progress(BatchId(2))
            .unwrap()
            .up_delivered,
        1
    );
    assert_eq!(terminal.live_slots(), 1);
    assert_eq!(terminal.hidden_modifier_references(), 1);
    assert!(matches!(
        generated.retire_batch(BatchId(2)),
        Err(GeneratedError::Lifecycle)
    ));
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(200))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(200))
        .unwrap()
        .unwrap();
    drop(g);
    drop(t);
    assert_eq!(generated.hidden_modifier_references(), 1);
    assert_eq!(terminal.queued_events(), 0);
    let g = generated
        .prepare_input_up_scoped(&mut saved, MonoTime(160))
        .unwrap()
        .unwrap();
    let t = terminal
        .prepare_input_up(BatchId(2), MonoTime(160))
        .unwrap()
        .unwrap();
    g.commit();
    t.commit();
    assert_eq!(generated.queued_events(), 0);
    assert_eq!(terminal.queued_events(), 1);
    assert_eq!(
        drain(&mut terminal, 160),
        [("left_shift".into(), KeyEdge::Down)]
    );
    assert_eq!(terminal.hidden_modifier_references(), 0);
    generated.retire_batch(BatchId(2)).unwrap();
    assert_eq!(terminal.pending_batches(), 1);
}
