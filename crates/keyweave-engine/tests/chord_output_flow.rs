#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, modifier_scope::ScopedActionPlan, output_stage::*, rule_selection::*,
    simultaneous::*, stage_bindings::*, terminal_actor::TerminalActor, timeline::*, *,
};
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};
struct Payload(u64);
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn cfg(when: KeyUpWhen, timed: bool) -> Arc<ConfigSnapshot> {
    let mut spec = Simultaneous::new(vec![key("a"), key("b")]);
    spec.threshold_ns = 50;
    spec.key_up_when = when;
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "actual chord output".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "chord".into(),
            from: key("a"),
            to: if timed {
                vec![
                    Action {
                        hold_down_ns: 50,
                        ..key("c").into()
                    },
                    key("d").into(),
                ]
            } else {
                vec![key("c").into()]
            },
            allow_extra_modifiers: true,
            extras: MappingExtras {
                simultaneous: Some(spec),
                ..Default::default()
            },
        }],
    })
}
fn origin(cfg: &Arc<ConfigSnapshot>, batch: u64, stage: Stage, dev: u64, at: u64) -> PlanOrigin {
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
fn raw(
    cfg: &Arc<ConfigSnapshot>,
    batch: u64,
    dev: u64,
    name: &str,
    edge: KeyEdge,
    at: u64,
) -> DueEvent<PlannedAction> {
    DueEvent {
        event: StageEvent {
            stage: Stage::Complex,
            at_ns: MonoTime(at),
            device: DeviceInstanceId {
                id: dev,
                generation: 1,
            },
            source: SourceToken(dev),
            config: cfg.clone(),
            payload: PlannedAction {
                slot: ActionSlot {
                    batch: BatchId(batch),
                    index: 0,
                },
                target: ActionTarget::Key(key(name)),
                edge,
                lazy: false,
                halt: false,
            },
        },
        delivered_at: MonoTime(at),
    }
}
fn matched(cfg: &Arc<ConfigSnapshot>) -> (RuleSelection, ChordMatch) {
    let mut arbiter = ChordArbiter::new(raw(cfg, 1, 1, "a", KeyEdge::Down, 100)).unwrap();
    arbiter
        .observe(raw(cfg, 2, 2, "b", KeyEdge::Down, 110))
        .unwrap();
    let ArbitrationChoice::Matched {
        selection,
        group,
        tail,
    } = arbiter
        .advance(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True)
        .unwrap()
        .choice
    else {
        panic!("chord")
    };
    assert!(tail.is_empty());
    (selection, group)
}
fn setup(cfg: &Arc<ConfigSnapshot>) -> (BindingArena<Payload>, OutputStageActor) {
    let mut arena = BindingArena::default();
    let mut output = OutputStageActor::new(cfg.clone()).unwrap();
    for batch in [1, 2] {
        arena
            .prepare(origin(cfg, batch, Stage::Complex, batch, 100), 1, None)
            .unwrap()
            .commit();
        output
            .observe_device(
                DeviceInstanceId {
                    id: batch,
                    generation: 1,
                },
                false,
                MonoTime(0),
            )
            .unwrap();
    }
    (arena, output)
}
fn admit(
    cfg: &Arc<ConfigSnapshot>,
    arena: &mut BindingArena<Payload>,
    output: &mut OutputStageActor,
) -> (ActiveChord, SharedRetention) {
    let (selection, group) = matched(cfg);
    let (active, receipts) = group.into_active();
    let plan = ScopedActionPlan::main(
        &selection,
        BatchId(10),
        MonoTime(110),
        DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        SourceToken(1),
    )
    .unwrap();
    let slots = plan
        .actions()
        .events()
        .iter()
        .filter(|event| event.payload.edge == KeyEdge::Down)
        .count();
    let inputs = receipts
        .into_iter()
        .map(|receipt| {
            let id = receipt.event.payload.slot.batch.0;
            (receipt, Payload(id))
        })
        .collect();
    let registration = arena
        .prepare_shared_child_with_down(origin(cfg, 10, Stage::Output, 1, 110), slots, inputs)
        .unwrap();
    let handle = output
        .prepare_shared_child_scoped(plan, registration)
        .unwrap()
        .commit();
    (active, handle)
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
    panic!("bounded fixture")
}
#[test]
fn actual_matched_chord_any_all_drives_saved_output_up_and_shared_participant_barrier() {
    for when in [KeyUpWhen::Any, KeyUpWhen::All] {
        let cfg = cfg(when, false);
        let (mut arena, mut output) = setup(&cfg);
        let (mut active, handle) = admit(&cfg, &mut arena, &mut output);
        assert_eq!(drain(&mut output, 110).0, [(key("c"), KeyEdge::Down)]);
        let first = active.up(&raw(&cfg, 1, 1, "a", KeyEdge::Up, 120)).unwrap();
        let mut completed = false;
        if first.release_output {
            output.input_up(BatchId(10), MonoTime(120)).unwrap();
            let (edges, done) = drain(&mut output, 120);
            assert_eq!(edges, [(key("c"), KeyEdge::Up)]);
            assert_eq!(done, [BatchId(10)]);
            completed = true;
        } else {
            assert!(drain(&mut output, 120).0.is_empty());
        }
        assert!(matches!(
            arena.take(ActionSlot {
                batch: BatchId(1),
                index: 0
            }),
            Err(BindingError::ChildPending)
        ));
        let last = active.up(&raw(&cfg, 2, 2, "b", KeyEdge::Up, 130)).unwrap();
        assert!(last.finished && last.after_group_up);
        if last.release_output {
            output.input_up(BatchId(10), MonoTime(130)).unwrap();
            let (_, done) = drain(&mut output, 130);
            assert_eq!(done, [BatchId(10)]);
            completed = true;
        }
        assert!(completed);
        arena.cancel(BatchId(10)).unwrap();
        arena.release_shared_retention(handle).unwrap();
        for id in [1, 2] {
            assert_eq!(
                arena
                    .take(ActionSlot {
                        batch: BatchId(id),
                        index: 0
                    })
                    .unwrap()
                    .0,
                id
            );
            arena.retire(BatchId(id)).unwrap();
        }
    }
}
#[test]
fn actual_chord_early_any_up_keeps_future_macro_and_retention_until_completion() {
    let cfg = cfg(KeyUpWhen::Any, true);
    let (mut arena, mut output) = setup(&cfg);
    let (mut active, handle) = admit(&cfg, &mut arena, &mut output);
    assert_eq!(drain(&mut output, 110).0, [(key("c"), KeyEdge::Down)]);
    assert!(
        active
            .up(&raw(&cfg, 1, 1, "a", KeyEdge::Up, 120))
            .unwrap()
            .release_output
    );
    output.input_up(BatchId(10), MonoTime(120)).unwrap();
    assert!(
        active
            .up(&raw(&cfg, 2, 2, "b", KeyEdge::Up, 130))
            .unwrap()
            .finished
    );
    assert!(drain(&mut output, 159).0.is_empty());
    assert_eq!(arena.shared_retention_references(), 2);
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
}
#[test]
fn full_old_chord_snapshot_survives_output_destination_reload_without_sanitizing_arc() {
    let cfg = cfg(KeyUpWhen::All, false);
    let (mut arena, mut output) = setup(&cfg);
    output
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let (mut active, handle) = admit(&cfg, &mut arena, &mut output);
    assert_eq!(drain(&mut output, 110).0, [(key("c"), KeyEdge::Down)]);
    active.up(&raw(&cfg, 1, 1, "a", KeyEdge::Up, 120)).unwrap();
    assert!(
        active
            .up(&raw(&cfg, 2, 2, "b", KeyEdge::Up, 130))
            .unwrap()
            .release_output
    );
    output.input_up(BatchId(10), MonoTime(130)).unwrap();
    assert_eq!(drain(&mut output, 130).0, [(key("c"), KeyEdge::Up)]);
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
    assert!(cfg.complex[0].extras.simultaneous.is_some());
}
#[test]
fn output_metadata_permission_does_not_enable_input_execution_or_normal_chord_selection() {
    let cfg = cfg(KeyUpWhen::Any, false);
    assert!(OutputStageActor::new(cfg.clone()).is_ok());
    assert!(TerminalActor::new(cfg.clone()).is_ok());
    assert!(matches!(
        Engine::new(cfg.clone()),
        Err(EngineError::UnsupportedSimultaneous)
    ));
    assert!(complex_actor::ComplexStageActor::new(cfg.clone()).is_err());
    assert!(simple_actor::SimpleStageActor::new(cfg.clone()).is_err());
    assert!(physical_controller::PhysicalStageController::new(cfg.clone()).is_err());
    assert!(generated_stage::GeneratedStageQueue::new(Stage::Complex, cfg.clone()).is_err());
    assert!(
        select_rule(
            cfg,
            MappingStage::Complex,
            key("a"),
            BTreeSet::new(),
            |_| Truth::True
        )
        .is_none()
    );
}
#[test]
fn malformed_chord_metadata_is_rejected_even_for_output_only_model() {
    let mut malformed = (*cfg(KeyUpWhen::Any, false)).clone();
    malformed.complex[0]
        .extras
        .simultaneous
        .as_mut()
        .unwrap()
        .keys
        .clear();
    let malformed = Arc::new(malformed);
    assert!(OutputStageActor::new(malformed.clone()).is_err());
    assert!(TerminalActor::new(malformed).is_err());
}

#[test]
fn output_target_equal_to_chord_member_executes_leaf_without_reentering_input_selector() {
    let mut snapshot = (*cfg(KeyUpWhen::Any, false)).clone();
    snapshot.complex[0].to = vec![key("a").into()];
    let cfg = Arc::new(snapshot);
    let (mut arena, mut output) = setup(&cfg);
    let (mut active, handle) = admit(&cfg, &mut arena, &mut output);
    assert_eq!(drain(&mut output, 110).0, [(key("a"), KeyEdge::Down)]);
    active.up(&raw(&cfg, 1, 1, "a", KeyEdge::Up, 120)).unwrap();
    output.input_up(BatchId(10), MonoTime(120)).unwrap();
    assert_eq!(drain(&mut output, 120).0, [(key("a"), KeyEdge::Up)]);
    active.up(&raw(&cfg, 2, 2, "b", KeyEdge::Up, 130)).unwrap();
    arena.cancel(BatchId(10)).unwrap();
    arena.release_shared_retention(handle).unwrap();
}
