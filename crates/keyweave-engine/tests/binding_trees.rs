#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, stage_bindings::*, timeline::Stage, *};
use keyweave_types::*;
use std::sync::Arc;
struct Payload(&'static str); // Deliberately neither Clone nor Debug.
fn slot(batch: u64, index: u16) -> ActionSlot {
    ActionSlot {
        batch: BatchId(batch),
        index,
    }
}
fn origin(config: &Arc<ConfigSnapshot>, batch: u64, stage: Stage) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(batch),
        stage,
        start: MonoTime(50),
        device: DeviceInstanceId {
            id: 1,
            generation: 2,
        },
        source: SourceToken(90),
        config: config.clone(),
    }
}
fn reserve(
    arena: &mut BindingArena<Payload>,
    config: &Arc<ConfigSnapshot>,
    batch: u64,
    stage: Stage,
    count: usize,
    parent: Option<ActionSlot>,
) {
    arena
        .prepare(origin(config, batch, stage), count, parent)
        .unwrap()
        .commit();
}
fn tree() -> (BindingArena<Payload>, Arc<ConfigSnapshot>) {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::default();
    reserve(&mut arena, &config, 10, Stage::Simple, 3, None);
    arena.insert(slot(10, 0), Payload("root-0")).unwrap();
    arena.insert(slot(10, 1), Payload("root-1")).unwrap();
    reserve(
        &mut arena,
        &config,
        20,
        Stage::Complex,
        3,
        Some(slot(10, 0)),
    );
    arena.insert(slot(20, 0), Payload("complex-0")).unwrap();
    arena
        .insert(slot(20, 2), Payload("already-released"))
        .unwrap();
    assert_eq!(arena.take(slot(20, 2)).unwrap().0, "already-released");
    reserve(&mut arena, &config, 30, Stage::Output, 2, Some(slot(20, 0)));
    arena.insert(slot(30, 0), Payload("output-0")).unwrap();
    reserve(
        &mut arena,
        &config,
        21,
        Stage::Complex,
        1,
        Some(slot(10, 1)),
    );
    arena.insert(slot(21, 0), Payload("complex-1")).unwrap();
    reserve(&mut arena, &config, 31, Stage::Output, 1, Some(slot(21, 0)));
    arena.insert(slot(31, 0), Payload("output-1")).unwrap();
    (arena, config)
}
#[test]
fn subtree_inspection_separates_saved_live_reserved_and_released_nodes_without_cloning() {
    let (arena, config) = tree();
    let mut nodes = vec![];
    arena
        .visit_subtree(slot(10, 0), |node| {
            assert!(Arc::ptr_eq(&node.origin.config, &config));
            assert_eq!(
                node.origin.device,
                DeviceInstanceId {
                    id: 1,
                    generation: 2
                }
            );
            assert_eq!(node.origin.source, SourceToken(90));
            let state = match node.state {
                BindingState::Live(payload) => payload.0,
                BindingState::Reserved => "reserved",
                BindingState::Released => "released",
            };
            nodes.push((node.slot, node.origin.stage, node.parent, state));
        })
        .unwrap();
    assert_eq!(
        nodes,
        [
            (slot(10, 0), Stage::Simple, None, "root-0"),
            (slot(20, 0), Stage::Complex, Some(slot(10, 0)), "complex-0"),
            (slot(30, 0), Stage::Output, Some(slot(20, 0)), "output-0"),
            (slot(30, 1), Stage::Output, Some(slot(20, 0)), "reserved"),
            (slot(20, 1), Stage::Complex, Some(slot(10, 0)), "reserved"),
            (slot(20, 2), Stage::Complex, Some(slot(10, 0)), "released"),
        ]
    );
    assert_eq!(arena.live_slots(), 6);
    assert_eq!(arena.reserved_slots(), 10);
}
#[test]
fn tree_cancellation_returns_children_first_with_original_provenance_and_only_real_downs() {
    let (mut arena, config) = tree();
    let weak = Arc::downgrade(&config);
    drop(config);
    let records = arena.cancel_subtree(BatchId(10)).unwrap();
    assert_eq!(
        records
            .iter()
            .map(|record| record.origin.batch)
            .collect::<Vec<_>>(),
        [
            BatchId(31),
            BatchId(21),
            BatchId(30),
            BatchId(20),
            BatchId(10)
        ]
    );
    let payloads: Vec<_> = records
        .iter()
        .flat_map(|record| {
            assert_eq!(record.origin.config.generation, ConfigGeneration(1));
            assert_eq!(record.origin.source, SourceToken(90));
            assert_eq!(record.origin.device.generation, 2);
            record
                .payloads
                .iter()
                .map(|saved| (saved.slot, saved.payload.0))
        })
        .collect();
    assert_eq!(
        payloads,
        [
            (slot(31, 0), "output-1"),
            (slot(21, 0), "complex-1"),
            (slot(30, 0), "output-0"),
            (slot(20, 0), "complex-0"),
            (slot(10, 1), "root-1"),
            (slot(10, 0), "root-0")
        ]
    );
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_slots(), 0);
    assert!(weak.upgrade().is_some()); // Recipes, not current config, retain old Arc.
    drop(records);
    assert!(weak.upgrade().is_none());
}
#[test]
fn unknown_or_nonlive_root_rejects_before_callbacks_or_mutation() {
    let (mut arena, _) = tree();
    for root in [slot(999, 0), slot(10, 2), slot(20, 2), slot(10, u16::MAX)] {
        let mut calls = 0;
        assert_eq!(
            arena.visit_subtree(root, |_| calls += 1),
            Err(BindingError::Lifecycle)
        );
        assert_eq!(calls, 0);
    }
    assert!(matches!(
        arena.cancel_subtree(BatchId(999)),
        Err(BindingError::Lifecycle)
    ));
    assert_eq!(arena.live_slots(), 6);
    assert_eq!(arena.reserved_slots(), 10);
}
#[test]
fn canceling_one_child_subtree_detaches_its_parent_link_and_preserves_siblings() {
    let (mut arena, _) = tree();
    let records = arena.cancel_subtree(BatchId(20)).unwrap();
    assert_eq!(
        records
            .iter()
            .map(|record| record.origin.batch)
            .collect::<Vec<_>>(),
        [BatchId(30), BatchId(20)]
    );
    assert_eq!(records[1].parent, Some(slot(10, 0)));
    assert_eq!(arena.take(slot(10, 0)).unwrap().0, "root-0");
    assert!(matches!(
        arena.take(slot(10, 1)),
        Err(BindingError::ChildPending)
    ));
    let mut nodes = vec![];
    arena
        .visit_subtree(slot(10, 1), |node| nodes.push(node.slot))
        .unwrap();
    assert_eq!(nodes, [slot(10, 1), slot(21, 0), slot(31, 0)]);
    let remaining = arena.cancel_subtree(BatchId(10)).unwrap();
    assert_eq!(
        remaining
            .iter()
            .map(|record| record.origin.batch)
            .collect::<Vec<_>>(),
        [BatchId(31), BatchId(21), BatchId(10)]
    );
    assert_eq!(arena.reserved_slots(), 0);
}
#[test]
fn aborted_child_reservation_leaves_no_adjacency_or_cleanup_recipe() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::default();
    reserve(&mut arena, &config, 1, Stage::Simple, 1, None);
    arena.insert(slot(1, 0), Payload("root")).unwrap();
    drop(
        arena
            .prepare(origin(&config, 2, Stage::Complex), 1, Some(slot(1, 0)))
            .unwrap(),
    );
    let mut count = 0;
    arena.visit_subtree(slot(1, 0), |_| count += 1).unwrap();
    assert_eq!(count, 1);
    assert_eq!(arena.take(slot(1, 0)).unwrap().0, "root");
    let canceled = arena.cancel_subtree(BatchId(1)).unwrap();
    assert_eq!(canceled.len(), 1);
    assert!(canceled[0].payloads.is_empty());
    assert_eq!(arena.reserved_slots(), 0);
}
#[test]
fn maximum_batch_tree_is_bounded_and_cancellation_includes_unexecuted_child_batches() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::default();
    reserve(&mut arena, &config, 1, Stage::Simple, 1, None);
    arena.insert(slot(1, 0), Payload("root")).unwrap();
    for id in 2..=MAX_BINDINGS as u64 {
        reserve(&mut arena, &config, id, Stage::Complex, 1, Some(slot(1, 0)));
    }
    assert!(matches!(
        arena.prepare(
            origin(&config, MAX_BINDINGS as u64 + 1, Stage::Complex),
            1,
            Some(slot(1, 0))
        ),
        Err(BindingError::Capacity)
    ));
    let mut count = 0;
    arena.visit_subtree(slot(1, 0), |_| count += 1).unwrap();
    assert_eq!(count, MAX_BINDINGS);
    let records = arena.cancel_subtree(BatchId(1)).unwrap();
    assert_eq!(records.len(), MAX_BINDINGS);
    assert_eq!(records[0].origin.batch, BatchId(MAX_BINDINGS as u64));
    assert_eq!(records.last().unwrap().origin.batch, BatchId(1));
    assert_eq!(
        records
            .iter()
            .map(|record| record.payloads.len())
            .sum::<usize>(),
        1
    );
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_slots(), 0);
}
