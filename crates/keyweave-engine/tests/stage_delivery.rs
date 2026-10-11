#![allow(clippy::unwrap_used)]
use keyweave_engine::{
    action_plan::*, generated_stage::*, modifier_scope::ScopedActionPlan, rule_selection::*,
    timeline::Stage, *,
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
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "delivery".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key("right_control").into()],
            allow_extra_modifiers: true,
            extras: MappingExtras {
                if_alone: vec![Action {
                    hold_down_ns: 50,
                    ..key("b").into()
                }],
                ..Default::default()
            },
        }],
    })
}
fn queue() -> GeneratedStageQueue {
    let mut queue = GeneratedStageQueue::new(Stage::Output, config()).unwrap();
    queue.observe_device(device(1), false).unwrap();
    queue.observe_device(device(2), false).unwrap();
    queue
}
fn plan(
    queue: &GeneratedStageQueue,
    id: u64,
    start: u64,
    source_device: u64,
    tap: bool,
) -> ActionPlan {
    plan_actions(
        &[Action {
            hold_down_ns: 50,
            ..key("b").into()
        }],
        PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(start),
            device: device(source_device),
            source: SourceToken(100),
            config: queue.snapshot().clone(),
        },
        tap,
    )
    .unwrap()
}
#[test]
fn future_tap_slot_stays_reserved_and_cannot_alias_until_delivery_and_explicit_retirement() {
    let mut queue = queue();
    let plan = plan(&queue, 1, 100, 1, true);
    queue.enqueue(plan.events().to_vec()).unwrap();
    assert_eq!(queue.logical_slots(), 0);
    assert_eq!(queue.reserved_slots(), 1);
    assert_eq!(queue.pending_batches(), 1);
    assert_eq!(
        queue.delivery_progress(BatchId(1)).unwrap(),
        DeliveryProgress {
            slots: 1,
            down_delivered: 0,
            up_generated: 1,
            up_delivered: 0,
            scope_reservations: 0
        }
    );
    assert_eq!(
        queue.enqueue(plan.events().to_vec()).unwrap_err().error,
        GeneratedError::Lifecycle
    );
    assert_eq!(
        queue.retire_batch(BatchId(1)),
        Err(GeneratedError::Lifecycle)
    );
    queue.poll(MonoTime(100)).unwrap().unwrap();
    assert_eq!(
        queue.delivery_progress(BatchId(1)).unwrap().down_delivered,
        1
    );
    assert_eq!(
        queue.retire_batch(BatchId(1)),
        Err(GeneratedError::Lifecycle)
    );
    queue.poll(MonoTime(150)).unwrap().unwrap();
    assert_eq!(queue.queued_events(), 0);
    assert_eq!(queue.reserved_slots(), 1);
    assert_eq!(
        queue.enqueue(plan.events().to_vec()).unwrap_err().error,
        GeneratedError::Lifecycle
    );
    queue.retire_batch(BatchId(1)).unwrap();
    assert_eq!(queue.reserved_slots(), 0);
    assert_eq!(queue.pending_batches(), 0);
    assert!(queue.delivery_progress(BatchId(1)).is_none());
}
#[test]
fn ledger_rejects_batch_metadata_aliases_and_partial_initial_down_extensions() {
    let mut queue = queue();
    let first = plan(&queue, 1, 100, 1, true);
    let mut mixed = first.events().to_vec();
    let mut second = first.events()[0].clone();
    second.payload.slot.index = 1;
    second.device = device(2);
    mixed.push(second);
    assert_eq!(
        queue.enqueue(mixed).unwrap_err().error,
        GeneratedError::Provenance
    );
    assert_eq!(queue.reserved_slots(), 0);
    assert_eq!(queue.queued_events(), 0);
    queue.enqueue(first.events().to_vec()).unwrap();
    let mut extension = first.events()[0].clone();
    extension.payload.slot.index = 1;
    assert_eq!(
        queue.enqueue(vec![extension]).unwrap_err().error,
        GeneratedError::Lifecycle
    );
    assert_eq!(queue.reserved_slots(), 1);
}
#[test]
fn scoped_snapshot_sources_remain_reserved_after_logical_scope_is_closed() {
    let mut queue = queue();
    let rule = select_rule(
        queue.snapshot().clone(),
        MappingStage::Complex,
        key("a"),
        BTreeSet::from([key("left_shift"), key("left_control")]),
        |_| Truth::True,
    )
    .unwrap();
    let branch = rule
        .select_branch(RuleBranch::Alone, false, |_| Truth::True)
        .unwrap();
    let scoped = ScopedActionPlan::branch(
        &branch,
        BatchId(1),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap();
    queue.enqueue_scoped(&scoped).unwrap();
    assert_eq!(queue.logical_slots(), 0);
    assert!(queue.modifiers().is_empty());
    assert_eq!(queue.reserved_slots(), 3);
    assert_eq!(
        queue
            .delivery_progress(BatchId(1))
            .unwrap()
            .scope_reservations,
        2
    );
    while queue.poll(MonoTime(150)).unwrap().is_some() {}
    assert_eq!(queue.reserved_slots(), 3);
    queue.retire_batch(BatchId(1)).unwrap();
    assert_eq!(queue.reserved_slots(), 0);
}
#[test]
fn completed_modifier_tap_cannot_retire_before_its_original_scope_up() {
    let mut cfg = (*config()).clone();
    cfg.complex[0].to[0].repeat = false;
    cfg.complex[0].to[0].hold_down_ns = 50;
    let mut queue = GeneratedStageQueue::new(Stage::Output, Arc::new(cfg)).unwrap();
    queue.observe_device(device(1), false).unwrap();
    let rule = queue
        .select(
            key("a"),
            &ContextSnapshot::default(),
            device(1),
            MonoTime(0),
        )
        .unwrap()
        .unwrap();
    let scoped = ScopedActionPlan::main(
        &rule,
        BatchId(1),
        MonoTime(100),
        device(1),
        SourceToken(100),
    )
    .unwrap();
    let mut saved = queue.enqueue_scoped(&scoped).unwrap();
    while queue.poll(MonoTime(150)).unwrap().is_some() {}
    assert_eq!(queue.delivery_progress(BatchId(1)).unwrap().up_delivered, 1);
    assert_eq!(
        queue.retire_batch(BatchId(1)),
        Err(GeneratedError::Lifecycle)
    );
    queue.input_up_scoped(&mut saved, MonoTime(150)).unwrap();
    queue.retire_batch(BatchId(1)).unwrap();
    assert_eq!(queue.reserved_slots(), 0);
}
#[test]
fn cancellation_device_generation_and_reset_reclaim_pending_delivery_reservations() {
    let mut queue = queue();
    queue
        .enqueue(plan(&queue, 1, 100, 1, true).events().to_vec())
        .unwrap();
    queue
        .enqueue(plan(&queue, 2, 100, 2, true).events().to_vec())
        .unwrap();
    assert_eq!(queue.reserved_slots(), 2);
    queue
        .observe_device(
            DeviceInstanceId {
                id: 1,
                generation: 2,
            },
            false,
        )
        .unwrap();
    assert_eq!(queue.reserved_slots(), 1);
    assert_eq!(queue.pending_batches(), 1);
    assert!(queue.delivery_progress(BatchId(1)).is_none());
    queue.cancel_batch(BatchId(2));
    assert_eq!(queue.reserved_slots(), 0);
    queue
        .enqueue(plan(&queue, 3, 100, 2, true).events().to_vec())
        .unwrap();
    queue.reset(MonoTime(0)).unwrap();
    assert_eq!(queue.reserved_slots(), 0);
    assert_eq!(queue.pending_batches(), 0);
}
#[test]
fn admission_failure_does_not_leave_delivery_reservation_and_same_identity_can_retry() {
    let mut queue = queue();
    queue.poll(MonoTime(u64::MAX - 10)).unwrap();
    let failing = plan(&queue, 1, 0, 1, true);
    assert!(matches!(
        queue.enqueue(failing.events().to_vec()),
        Err(RejectedGeneratedBatch {
            error: GeneratedError::Timeline(_),
            ..
        })
    ));
    assert_eq!(queue.reserved_slots(), 0);
    assert_eq!(queue.pending_batches(), 0);
    queue
        .enqueue(plan(&queue, 1, 0, 1, false).events().to_vec())
        .unwrap();
    assert_eq!(queue.reserved_slots(), 1);
}
#[test]
fn old_arc_up_delivery_survives_reload_and_empty_plan_has_no_delivery_batch() {
    let mut queue = queue();
    let held = plan(&queue, 1, 100, 1, false);
    queue.enqueue(held.events().to_vec()).unwrap();
    queue
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    queue.enqueue(held.input_up_events(MonoTime(150))).unwrap();
    while queue.poll(MonoTime(150)).unwrap().is_some() {}
    assert_eq!(queue.delivery_progress(BatchId(1)).unwrap().up_delivered, 1);
    queue.retire_batch(BatchId(1)).unwrap();
    queue.enqueue(vec![]).unwrap();
    assert_eq!(queue.pending_batches(), 0);
    assert_eq!(queue.reserved_slots(), 0);
}
#[test]
fn one_delivered_up_does_not_retire_other_held_slots_in_the_same_batch() {
    let mut queue = queue();
    let plan = plan_actions(
        &[
            Action {
                hold_down_ns: 50,
                ..key("b").into()
            },
            key("c").into(),
        ],
        PlanOrigin {
            batch: BatchId(1),
            stage: Stage::Output,
            start: MonoTime(0),
            device: device(1),
            source: SourceToken(100),
            config: queue.snapshot().clone(),
        },
        false,
    )
    .unwrap();
    queue.enqueue(plan.events().to_vec()).unwrap();
    while queue.poll(MonoTime(50)).unwrap().is_some() {}
    let progress = queue.delivery_progress(BatchId(1)).unwrap();
    assert_eq!(
        (
            progress.slots,
            progress.down_delivered,
            progress.up_delivered
        ),
        (2, 2, 1)
    );
    assert_eq!(
        queue.retire_batch(BatchId(1)),
        Err(GeneratedError::Lifecycle)
    );
    queue.enqueue(plan.input_up_events(MonoTime(100))).unwrap();
    assert_eq!(queue.delivery_progress(BatchId(1)).unwrap().up_generated, 2);
    assert_eq!(
        queue.retire_batch(BatchId(1)),
        Err(GeneratedError::Lifecycle)
    );
    queue.poll(MonoTime(100)).unwrap().unwrap();
    queue.retire_batch(BatchId(1)).unwrap();
    assert_eq!(queue.reserved_slots(), 0);
}
