#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, stage_bindings::*, timeline::Stage, *};
use keyweave_types::*;
use std::sync::Arc;
fn origin(id: u64, stage: Stage, config: &Arc<ConfigSnapshot>) -> PlanOrigin {
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
fn slot(id: u64, index: u16) -> ActionSlot {
    ActionSlot {
        batch: BatchId(id),
        index,
    }
}
#[test]
fn aborted_admission_and_duplicate_edges_do_not_change_saved_bindings() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::<u64>::default();
    drop(
        arena
            .prepare(origin(1, Stage::Output, &config), 2, None)
            .unwrap(),
    );
    assert_eq!(arena.reserved_slots(), 0);
    arena
        .prepare(origin(1, Stage::Output, &config), 2, None)
        .unwrap()
        .commit();
    assert_eq!(arena.take(slot(1, 0)), Err(BindingError::Lifecycle));
    arena.insert(slot(1, 0), 42).unwrap();
    assert_eq!(arena.insert(slot(1, 0), 99), Err(BindingError::Lifecycle));
    assert_eq!(arena.take(slot(1, 0)).unwrap(), 42);
    assert_eq!(arena.check_down(slot(1, 0)), Err(BindingError::Lifecycle));
    assert_eq!(arena.retire(BatchId(1)), Err(BindingError::Lifecycle));
    assert!(arena.cancel(BatchId(1)).unwrap().is_empty());
    assert_eq!(arena.reserved_slots(), 0);
}
#[test]
fn children_retain_old_provenance_and_block_parent_release_until_retired() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(1, Stage::Simple, &config), 1, None)
        .unwrap()
        .commit();
    arena.insert(slot(1, 0), "parent").unwrap();
    let mut wrong = origin(2, Stage::Complex, &config);
    wrong.source = SourceToken(11);
    assert!(matches!(
        arena.prepare(wrong, 1, Some(slot(1, 0))),
        Err(BindingError::Provenance)
    ));
    let other = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    assert!(matches!(
        arena.prepare(origin(2, Stage::Complex, &other), 1, Some(slot(1, 0))),
        Err(BindingError::Provenance)
    ));
    let mut wrong = origin(2, Stage::Complex, &config);
    wrong.device.generation = 2;
    assert!(matches!(
        arena.prepare(wrong, 1, Some(slot(1, 0))),
        Err(BindingError::Provenance)
    ));
    assert!(matches!(
        arena.prepare(origin(2, Stage::Output, &config), 1, Some(slot(1, 0))),
        Err(BindingError::Provenance)
    ));
    arena
        .prepare(origin(2, Stage::Complex, &config), 1, Some(slot(1, 0)))
        .unwrap()
        .commit();
    arena.insert(slot(2, 0), "child").unwrap();
    arena
        .prepare(origin(3, Stage::Output, &config), 1, Some(slot(2, 0)))
        .unwrap()
        .commit();
    arena.insert(slot(3, 0), "leaf").unwrap();
    assert_eq!(arena.take(slot(1, 0)), Err(BindingError::ChildPending));
    assert_eq!(arena.cancel(BatchId(2)), Err(BindingError::ChildPending));
    assert!(matches!(
        arena.prepare(origin(4, Stage::Output, &config), 1, Some(slot(3, 0))),
        Err(BindingError::Provenance)
    ));
    assert_eq!(arena.take(slot(3, 0)).unwrap(), "leaf");
    arena.retire(BatchId(3)).unwrap();
    assert_eq!(arena.take(slot(2, 0)).unwrap(), "child");
    arena.retire(BatchId(2)).unwrap();
    assert_eq!(arena.take(slot(1, 0)).unwrap(), "parent");
    arena.retire(BatchId(1)).unwrap();
    assert_eq!(arena.live_slots(), 0);
    assert_eq!(arena.reserved_slots(), 0);
}
#[test]
fn cancellation_returns_only_real_downs_in_reverse_order_and_reclaims_capacity() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(1, Stage::Output, &config), MAX_OUTPUT_OWNERS, None)
        .unwrap()
        .commit();
    assert!(matches!(
        arena.prepare(origin(2, Stage::Output, &config), 1, None),
        Err(BindingError::Capacity)
    ));
    assert_eq!(
        arena.insert(slot(1, u16::MAX), 9),
        Err(BindingError::Lifecycle)
    );
    arena.insert(slot(1, 0), 1).unwrap();
    arena.insert(slot(1, 2), 2).unwrap();
    assert_eq!(arena.cancel(BatchId(1)).unwrap(), vec![2, 1]);
    assert_eq!(arena.live_slots(), 0);
    arena
        .prepare(origin(2, Stage::Output, &config), 1, None)
        .unwrap()
        .commit();
    assert_eq!(arena.reserved_slots(), 1);
}

#[test]
fn live_read_is_source_slot_specific_and_never_exposes_reserved_or_released_payloads() {
    struct Payload(u64);
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut arena = BindingArena::default();
    arena
        .prepare(origin(1, Stage::Output, &config), 2, None)
        .unwrap()
        .commit();
    assert!(arena.live(slot(1, 0)).is_none());
    assert!(arena.live(slot(1, 2)).is_none());
    assert!(arena.live(slot(2, 0)).is_none());
    arena.insert(slot(1, 0), Payload(42)).unwrap();
    for _ in 0..1000 {
        assert_eq!(arena.live(slot(1, 0)).unwrap().0, 42);
    }
    assert_eq!(arena.reserved_slots(), 2);
    assert_eq!(arena.live_slots(), 1);
    assert_eq!(arena.take(slot(1, 0)).unwrap().0, 42);
    assert!(arena.live(slot(1, 0)).is_none());
    arena.cancel(BatchId(1)).unwrap();
    assert!(arena.live(slot(1, 1)).is_none());
    assert_eq!(arena.reserved_slots(), 0);
}
