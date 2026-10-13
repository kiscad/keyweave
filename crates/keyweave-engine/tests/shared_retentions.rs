#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, stage_bindings::*, timeline::Stage, *};
use keyweave_types::*;
use std::sync::Arc;
struct Payload(u64); // Neither Clone nor Debug.
fn slot(batch: u64, index: u16) -> ActionSlot {
    ActionSlot {
        batch: BatchId(batch),
        index,
    }
}
fn cfg(generation: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot::empty(ConfigGeneration(generation)))
}
fn origin(cfg: &Arc<ConfigSnapshot>, batch: u64, device: u64, stage: Stage) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(batch),
        stage,
        start: MonoTime(100),
        device: DeviceInstanceId {
            id: device,
            generation: 1,
        },
        source: SourceToken(device),
        config: cfg.clone(),
    }
}
fn reserve(
    arena: &mut BindingArena<Payload>,
    cfg: &Arc<ConfigSnapshot>,
    batch: u64,
    device: u64,
    count: usize,
    stage: Stage,
    parent: Option<ActionSlot>,
) {
    arena
        .prepare(origin(cfg, batch, device, stage), count, parent)
        .unwrap()
        .commit();
}
#[test]
fn reserved_and_live_participants_stay_until_shared_handle_release() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 2, Stage::Complex, None);
    let handle = arena
        .prepare_shared_retention(vec![slot(1, 0), slot(1, 1)])
        .unwrap()
        .commit();
    assert_eq!(arena.shared_retention_references(), 2);
    assert_eq!(arena.live_slots(), 0);
    arena.insert(slot(1, 0), Payload(10)).unwrap();
    arena.insert(slot(1, 1), Payload(20)).unwrap();
    assert!(matches!(
        arena.take(slot(1, 0)),
        Err(BindingError::ChildPending)
    ));
    assert!(matches!(
        arena.cancel(BatchId(1)),
        Err(BindingError::ChildPending)
    ));
    assert_eq!(
        arena.release_shared_retention(handle).unwrap(),
        [slot(1, 0), slot(1, 1)]
    );
    assert_eq!(arena.take(slot(1, 0)).unwrap().0, 10);
    assert_eq!(arena.take(slot(1, 1)).unwrap().0, 20);
    arena.retire(BatchId(1)).unwrap();
}
#[test]
fn drop_and_abort_prepare_change_no_ids_references_or_payloads() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 2, Stage::Complex, None);
    drop(arena.prepare_shared_retention(vec![slot(1, 0)]).unwrap());
    assert_eq!(arena.shared_retention_groups(), 0);
    assert_eq!(
        arena
            .prepare_shared_retention(vec![slot(1, 1)])
            .unwrap()
            .abort(),
        [slot(1, 1)]
    );
    let handle = arena
        .prepare_shared_retention(vec![slot(1, 0)])
        .unwrap()
        .commit();
    let first = handle.key();
    arena.release_shared_retention(handle).unwrap();
    let other = arena
        .prepare_shared_retention(vec![slot(1, 1)])
        .unwrap()
        .commit();
    assert!(other.key() > first);
    arena.release_shared_retention(other).unwrap();
}
#[test]
fn overlapping_holds_require_last_release_and_preserve_unrelated_slot() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 3, Stage::Complex, None);
    for index in 0..3 {
        arena.insert(slot(1, index), Payload(index as u64)).unwrap();
    }
    let first = arena
        .prepare_shared_retention(vec![slot(1, 0), slot(1, 1)])
        .unwrap()
        .commit();
    let second = arena
        .prepare_shared_retention(vec![slot(1, 0)])
        .unwrap()
        .commit();
    assert_eq!(arena.take(slot(1, 2)).unwrap().0, 2);
    arena.release_shared_retention(first).unwrap();
    assert_eq!(arena.take(slot(1, 1)).unwrap().0, 1);
    assert!(matches!(
        arena.take(slot(1, 0)),
        Err(BindingError::ChildPending)
    ));
    arena.release_shared_retention(second).unwrap();
    assert_eq!(arena.take(slot(1, 0)).unwrap().0, 0);
}
#[test]
fn foreign_arena_with_same_numeric_key_cannot_release_and_returns_original_handle() {
    let cfg = cfg(1);
    let mut first = BindingArena::default();
    let mut second = BindingArena::default();
    reserve(&mut first, &cfg, 1, 1, 1, Stage::Complex, None);
    reserve(&mut second, &cfg, 1, 1, 1, Stage::Complex, None);
    let handle = first
        .prepare_shared_retention(vec![slot(1, 0)])
        .unwrap()
        .commit();
    let other = second
        .prepare_shared_retention(vec![slot(1, 0)])
        .unwrap()
        .commit();
    assert_eq!(handle.key(), other.key());
    let rejected = second.release_shared_retention(handle).unwrap_err();
    assert_eq!(rejected.error, BindingError::Provenance);
    assert_eq!(first.shared_retention_references(), 1);
    assert_eq!(second.shared_retention_references(), 1);
    first.release_shared_retention(rejected.handle).unwrap();
    second.release_shared_retention(other).unwrap();
}
#[test]
fn cross_device_old_new_snapshots_are_retained_without_cloning_payloads() {
    let old = cfg(1);
    let new = cfg(2);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &old, 1, 1, 1, Stage::Complex, None);
    reserve(&mut arena, &new, 2, 2, 1, Stage::Complex, None);
    arena.insert(slot(1, 0), Payload(10)).unwrap();
    arena.insert(slot(2, 0), Payload(20)).unwrap();
    let handle = arena
        .prepare_shared_retention(vec![slot(1, 0), slot(2, 0)])
        .unwrap()
        .commit();
    assert_eq!(
        arena.retained_members(&handle).unwrap(),
        [slot(1, 0), slot(2, 0)]
    );
    let key = handle.key();
    assert_eq!(
        arena.blocking_subtree_retentions(BatchId(2)).unwrap(),
        [key]
    );
    assert!(matches!(
        arena.cancel_subtree(BatchId(2)),
        Err(BindingError::ChildPending)
    ));
    assert_eq!(arena.live_slots(), 2);
    arena.release_shared_retention(handle).unwrap();
    let canceled = arena.cancel_subtree(BatchId(2)).unwrap();
    assert_eq!(canceled[0].payloads[0].payload.0, 20);
    assert!(Arc::ptr_eq(&canceled[0].origin.config, &new));
    assert_eq!(arena.take(slot(1, 0)).unwrap().0, 10);
}
#[test]
fn subtree_cancellation_preflights_retained_descendants_before_removing_any_sibling() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 2, Stage::Simple, None);
    arena.insert(slot(1, 0), Payload(1)).unwrap();
    arena.insert(slot(1, 1), Payload(2)).unwrap();
    reserve(&mut arena, &cfg, 2, 1, 1, Stage::Complex, Some(slot(1, 0)));
    reserve(&mut arena, &cfg, 3, 1, 1, Stage::Complex, Some(slot(1, 1)));
    arena.insert(slot(2, 0), Payload(3)).unwrap();
    arena.insert(slot(3, 0), Payload(4)).unwrap();
    let handle = arena
        .prepare_shared_retention(vec![slot(2, 0)])
        .unwrap()
        .commit();
    assert_eq!(
        arena.blocking_subtree_retentions(BatchId(1)).unwrap(),
        [handle.key()]
    );
    assert!(matches!(
        arena.cancel_subtree(BatchId(1)),
        Err(BindingError::ChildPending)
    ));
    assert_eq!(arena.live_slots(), 4);
    assert_eq!(arena.live(slot(3, 0)).unwrap().0, 4);
    arena.release_shared_retention(handle).unwrap();
    assert_eq!(arena.cancel_subtree(BatchId(1)).unwrap().len(), 3);
    assert_eq!(arena.reserved_sources(), 0);
}
#[test]
fn bad_duplicate_released_unknown_and_mixed_stage_members_reject_cold() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 2, Stage::Complex, None);
    reserve(&mut arena, &cfg, 2, 1, 1, Stage::Output, None);
    arena.insert(slot(1, 1), Payload(1)).unwrap();
    arena.take(slot(1, 1)).unwrap();
    for members in [
        vec![slot(1, 0), slot(1, 0)],
        vec![slot(1, 1)],
        vec![slot(3, 0)],
    ] {
        assert!(matches!(
            arena.prepare_shared_retention(members),
            Err(BindingError::Lifecycle)
        ));
        assert_eq!(arena.shared_retention_references(), 0);
    }
    assert!(matches!(
        arena.prepare_shared_retention(vec![slot(1, 0), slot(2, 0)]),
        Err(BindingError::Provenance)
    ));
    assert!(matches!(
        arena.prepare_shared_retention(vec![]),
        Err(BindingError::Capacity)
    ));
    assert!(matches!(
        arena.prepare_shared_retention(vec![slot(1, 0); 33]),
        Err(BindingError::Capacity)
    ));
}
#[test]
fn exact_reference_budget_rejects_extra_then_release_admits_fresh_id() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 32, Stage::Complex, None);
    let members = (0..32).map(|index| slot(1, index)).collect::<Vec<_>>();
    let mut handles = vec![];
    for _ in 0..MAX_RETENTION_REFERENCES / 32 {
        handles.push(
            arena
                .prepare_shared_retention(members.clone())
                .unwrap()
                .commit(),
        );
    }
    assert_eq!(
        arena.shared_retention_references(),
        MAX_RETENTION_REFERENCES
    );
    assert!(matches!(
        arena.prepare_shared_retention(vec![slot(1, 0)]),
        Err(BindingError::Capacity)
    ));
    let last = handles.last().unwrap().key();
    arena
        .release_shared_retention(handles.pop().unwrap())
        .unwrap();
    let next = arena.prepare_shared_retention(members).unwrap().commit();
    assert!(next.key() > last);
    arena.release_shared_retention(next).unwrap();
    for handle in handles {
        arena.release_shared_retention(handle).unwrap();
    }
    assert_eq!(arena.shared_retention_references(), 0);
}

#[test]
fn actual_saved_producer_up_cannot_finish_until_shared_retention_is_released() {
    use keyweave_engine::{
        generated_stage::GeneratedStageQueue,
        producer_decision::{ProducerDecision, ProducerUpError},
        timeline::DueEvent,
    };
    let cfg = cfg(1);
    let mut arena = BindingArena::<ProducerDecision>::default();
    let mut queue = GeneratedStageQueue::new(Stage::Output, cfg.clone()).unwrap();
    let mut plans = vec![];
    for (batch, name) in [(1, "a"), (2, "b")] {
        let origin = origin(&cfg, batch, batch, Stage::Complex);
        arena.prepare(origin.clone(), 1, None).unwrap().commit();
        queue.observe_device(origin.device, false).unwrap();
        let plan = plan_actions(&[name.parse::<KeyId>().unwrap().into()], origin, false).unwrap();
        let down = DueEvent {
            event: plan.events()[0].clone(),
            delivered_at: MonoTime(100),
        };
        let decision = queue
            .decide_producer_down(&down, &ContextSnapshot::default())
            .unwrap()
            .with_original_up(&plan.releases()[0])
            .unwrap();
        arena.insert(slot(batch, 0), decision).unwrap();
        plans.push(plan);
    }
    let handle = arena
        .prepare_shared_retention(vec![slot(1, 0), slot(2, 0)])
        .unwrap()
        .commit();
    for plan in &plans {
        let up = DueEvent {
            event: plan.input_up_events(MonoTime(120)).remove(0),
            delivered_at: MonoTime(120),
        };
        arena.prepare_producer_up(&up).unwrap().commit();
        assert!(matches!(
            arena.finish_producer(up.event.payload.slot),
            Err(ProducerUpError::Binding(BindingError::ChildPending))
        ));
    }
    arena.release_shared_retention(handle).unwrap();
    for batch in [1, 2] {
        assert_eq!(
            arena
                .finish_producer(slot(batch, 0))
                .unwrap()
                .up_delivered_at(),
            Some(MonoTime(120))
        );
        arena.retire(BatchId(batch)).unwrap();
    }
    assert_eq!(arena.reserved_sources(), 0);
}

#[test]
fn exact_group_limit_is_independent_from_reference_budget() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 1, Stage::Complex, None);
    let mut handles = vec![];
    for _ in 0..MAX_RETENTION_GROUPS {
        handles.push(
            arena
                .prepare_shared_retention(vec![slot(1, 0)])
                .unwrap()
                .commit(),
        );
    }
    assert_eq!(arena.shared_retention_references(), MAX_RETENTION_GROUPS);
    assert!(matches!(
        arena.prepare_shared_retention(vec![slot(1, 0)]),
        Err(BindingError::Capacity)
    ));
    arena
        .release_shared_retention(handles.pop().unwrap())
        .unwrap();
    let extra = arena
        .prepare_shared_retention(vec![slot(1, 0)])
        .unwrap()
        .commit();
    arena.release_shared_retention(extra).unwrap();
    for handle in handles {
        arena.release_shared_retention(handle).unwrap();
    }
    assert_eq!(arena.shared_retention_groups(), 0);
}

#[test]
fn dropping_committed_handle_does_not_claim_metadata_or_output_cleanup() {
    let cfg = cfg(1);
    let mut arena = BindingArena::default();
    reserve(&mut arena, &cfg, 1, 1, 1, Stage::Complex, None);
    arena.insert(slot(1, 0), Payload(1)).unwrap();
    let handle = arena
        .prepare_shared_retention(vec![slot(1, 0)])
        .unwrap()
        .commit();
    drop(handle);
    assert_eq!(arena.shared_retention_references(), 1);
    assert!(matches!(
        arena.take(slot(1, 0)),
        Err(BindingError::ChildPending)
    ));
    assert!(matches!(
        arena.cancel_subtree(BatchId(1)),
        Err(BindingError::ChildPending)
    ));
}
