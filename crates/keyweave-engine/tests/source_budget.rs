#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, stage_bindings::*, timeline::Stage, *};
use keyweave_types::*;
use std::sync::Arc;
fn origin(config: &Arc<ConfigSnapshot>, id: u64, stage: Stage) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(id),
        stage,
        start: MonoTime(0),
        device: DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        source: SourceToken(10),
        config: config.clone(),
    }
}
fn slot(id: u64) -> ActionSlot {
    ActionSlot {
        batch: BatchId(id),
        index: 0,
    }
}
#[test]
fn exact_source_budget_includes_extra_units_and_slot_up_does_not_release_them() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::<u64>::default();
    arena
        .prepare(origin(&config, 1, Stage::Simple), 1, None)
        .unwrap()
        .with_extra_sources(MAX_OUTPUT_OWNERS - 1)
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_slots(), 1);
    assert_eq!(arena.extra_source_reservations(), MAX_OUTPUT_OWNERS - 1);
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS);
    assert!(matches!(
        arena.prepare(origin(&config, 2, Stage::Simple), 1, None),
        Err(BindingError::Capacity)
    ));
    arena.insert(slot(1), 42).unwrap();
    assert_eq!(arena.take(slot(1)).unwrap(), 42);
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS);
    assert!(matches!(
        arena.prepare(origin(&config, 2, Stage::Simple), 1, None),
        Err(BindingError::Capacity)
    ));
    arena.retire(BatchId(1)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
    assert_eq!(arena.extra_source_reservations(), 0);
}
#[test]
fn aborted_additional_reservation_and_usize_overflow_leave_no_source_budget_ghosts() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::<u64>::default();
    drop(
        arena
            .prepare(origin(&config, 1, Stage::Simple), 2, None)
            .unwrap()
            .with_extra_sources(5)
            .unwrap()
            .with_extra_sources(3)
            .unwrap(),
    );
    assert_eq!(arena.reserved_sources(), 0);
    assert_eq!(arena.reserved_slots(), 0);
    assert!(matches!(
        arena
            .prepare(origin(&config, 1, Stage::Simple), 2, None)
            .unwrap()
            .with_extra_sources(usize::MAX),
        Err(BindingError::Capacity)
    ));
    assert_eq!(arena.reserved_sources(), 0);
    arena
        .prepare(origin(&config, 1, Stage::Simple), 2, None)
        .unwrap()
        .with_extra_sources(8)
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_sources(), 10);
    let cleanup = arena.cancel_subtree(BatchId(1)).unwrap();
    assert_eq!(cleanup[0].extra_sources, 8);
    assert!(cleanup[0].payloads.is_empty());
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn subtree_cancellation_returns_each_original_extra_budget_and_preserves_surviving_parent() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(&config, 1, Stage::Simple), 1, None)
        .unwrap()
        .with_extra_sources(2)
        .unwrap()
        .commit();
    arena.insert(slot(1), 10).unwrap();
    arena
        .prepare(origin(&config, 2, Stage::Complex), 1, Some(slot(1)))
        .unwrap()
        .with_extra_sources(3)
        .unwrap()
        .commit();
    arena.insert(slot(2), 20).unwrap();
    arena
        .prepare(origin(&config, 3, Stage::Output), 1, Some(slot(2)))
        .unwrap()
        .with_extra_sources(4)
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_sources(), 12);
    assert_eq!(arena.reserved_slots(), 3);
    assert_eq!(arena.extra_source_reservations(), 9);
    let cleanup = arena.cancel_subtree(BatchId(2)).unwrap();
    assert_eq!(
        cleanup
            .iter()
            .map(|record| (record.origin.batch, record.extra_sources))
            .collect::<Vec<_>>(),
        [(BatchId(3), 4), (BatchId(2), 3)]
    );
    assert!(cleanup[0].payloads.is_empty());
    assert_eq!(cleanup[1].payloads[0].payload, 20);
    assert_eq!(arena.reserved_sources(), 3);
    assert_eq!(arena.extra_source_reservations(), 2);
    assert_eq!(arena.take(slot(1)).unwrap(), 10);
    assert_eq!(arena.reserved_sources(), 3);
    arena.retire(BatchId(1)).unwrap();
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn later_slot_admission_accounts_for_other_batches_extra_and_cancellation_reclaims_exact_units() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::<u64>::default();
    arena
        .prepare(origin(&config, 1, Stage::Simple), 1, None)
        .unwrap()
        .with_extra_sources(MAX_OUTPUT_OWNERS - 2)
        .unwrap()
        .commit();
    assert!(matches!(
        arena.prepare(origin(&config, 2, Stage::Simple), 2, None),
        Err(BindingError::Capacity)
    ));
    arena
        .prepare(origin(&config, 2, Stage::Simple), 1, None)
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS);
    assert!(arena.cancel(BatchId(1)).unwrap().is_empty());
    assert_eq!(arena.reserved_slots(), 1);
    assert_eq!(arena.extra_source_reservations(), 0);
    assert_eq!(arena.reserved_sources(), 1);
    arena
        .prepare(
            origin(&config, 3, Stage::Simple),
            MAX_OUTPUT_OWNERS - 1,
            None,
        )
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_sources(), MAX_OUTPUT_OWNERS);
}
