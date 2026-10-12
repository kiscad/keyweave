#![allow(clippy::unwrap_used)]
use super::*;
use crate::{action_plan::*, modifier_scope::ScopedActionPlan, rule_selection::*};
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn fixture(target: &str) -> (GeneratedStageQueue, Arc<ConfigSnapshot>) {
    let mut extras = MappingExtras::default();
    extras.from_modifiers.mandatory.insert(Modifier::Shift);
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "scope dispatch".into(),
        simple: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key(target).into()],
            allow_extra_modifiers: true,
            extras,
        }],
        complex: vec![],
    });
    let mut queue = GeneratedStageQueue::new_complex_propagating(cfg.clone()).unwrap();
    queue
        .observe_device(
            DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            false,
        )
        .unwrap();
    let plan = plan_actions(&[key("left_shift").into()], origin(&cfg, 1, 0), false).unwrap();
    queue.enqueue(plan.events().to_vec()).unwrap();
    queue.poll_mixed(MonoTime(0)).unwrap().unwrap();
    (queue, cfg)
}
fn origin(cfg: &Arc<ConfigSnapshot>, id: u64, at: u64) -> PlanOrigin {
    PlanOrigin {
        batch: BatchId(id),
        stage: Stage::Complex,
        start: MonoTime(at),
        device: DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        source: SourceToken(id),
        config: cfg.clone(),
    }
}
fn scope(cfg: Arc<ConfigSnapshot>, id: u64) -> ScopedActionPlan {
    let selection = select_rule(
        cfg,
        MappingStage::Simple,
        key("a"),
        BTreeSet::from([key("left_shift")]),
        |_| Truth::True,
    )
    .unwrap();
    ScopedActionPlan::main(
        &selection,
        BatchId(id),
        MonoTime(100),
        DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        SourceToken(id),
    )
    .unwrap()
}
fn opening(queue: &mut GeneratedStageQueue) -> DueEvent<ScopeCommand> {
    let GeneratedDelivery::Scope(due) = queue.poll_mixed(MonoTime(100)).unwrap().unwrap() else {
        panic!("expected open")
    };
    assert!(due.event.payload.opening);
    assert_eq!(
        due.event.payload.slots,
        [ActionSlot {
            batch: BatchId(1),
            index: 0
        }]
    );
    due
}
#[test]
fn persistent_scope_brackets_saved_up_and_prevents_retirement_until_close_delivery() {
    let (mut queue, cfg) = fixture("right_control");
    let mut saved = queue.enqueue_scoped(&scope(cfg, 2)).unwrap();
    assert!(!queue.modifiers().contains(&key("left_shift")));
    opening(&mut queue);
    assert!(matches!(
        queue.poll_mixed(MonoTime(100)).unwrap(),
        Some(GeneratedDelivery::Action(_))
    ));
    queue.input_up_scoped(&mut saved, MonoTime(200)).unwrap();
    assert!(queue.modifiers().contains(&key("left_shift")));
    assert!(matches!(
        queue.poll_mixed(MonoTime(200)).unwrap(),
        Some(GeneratedDelivery::Action(_))
    ));
    assert!(!queue.scope_delivery_complete(BatchId(2)));
    assert_eq!(
        queue.retire_batch(BatchId(2)),
        Err(GeneratedError::Lifecycle)
    );
    let GeneratedDelivery::Scope(due) = queue.poll_mixed(MonoTime(200)).unwrap().unwrap() else {
        panic!("close")
    };
    assert!(!due.event.payload.opening);
    assert!(queue.scope_delivery_complete(BatchId(2)));
    queue.retire_batch(BatchId(2)).unwrap();
}
#[test]
fn transient_scope_closes_after_main_down_but_keeps_original_up_recipe() {
    let (mut queue, cfg) = fixture("b");
    let mut saved = queue.enqueue_scoped(&scope(cfg, 2)).unwrap();
    assert!(queue.modifiers().contains(&key("left_shift")));
    opening(&mut queue);
    assert!(matches!(
        queue.poll_mixed(MonoTime(100)).unwrap(),
        Some(GeneratedDelivery::Action(_))
    ));
    let GeneratedDelivery::Scope(due) = queue.poll_mixed(MonoTime(100)).unwrap().unwrap() else {
        panic!("close")
    };
    assert!(!due.event.payload.opening);
    assert!(queue.scope_delivery_complete(BatchId(2)));
    queue.input_up_scoped(&mut saved, MonoTime(200)).unwrap();
    assert!(matches!(
        queue.poll_mixed(MonoTime(200)).unwrap(),
        Some(GeneratedDelivery::Action(_))
    ));
    assert!(queue.poll_mixed(MonoTime(200)).unwrap().is_none());
    queue.retire_batch(BatchId(2)).unwrap();
}
#[test]
fn aborted_preparation_leaves_scope_view_and_batch_retryable() {
    let (mut queue, cfg) = fixture("right_control");
    queue
        .prepare_scoped(&scope(cfg.clone(), 2))
        .unwrap()
        .abort();
    assert_eq!(queue.pending_batches(), 1);
    assert_eq!(queue.queued_events(), 0);
    assert!(queue.modifiers().contains(&key("left_shift")));
    queue.enqueue_scoped(&scope(cfg, 2)).unwrap();
    opening(&mut queue);
}
#[test]
fn full_mixed_fifo_rejection_preserves_scope_view_and_same_batch_can_retry() {
    let (mut queue, cfg) = fixture("right_control");
    let actions = vec![
        Action {
            hold_down_ns: 1,
            ..key("x").into()
        };
        512
    ];
    for id in 100..108 {
        queue
            .enqueue(
                plan_actions(&actions, origin(&cfg, id, 500), true)
                    .unwrap()
                    .events()
                    .to_vec(),
            )
            .unwrap();
    }
    assert!(matches!(
        queue.enqueue_scoped(&scope(cfg.clone(), 2)),
        Err(GeneratedError::Timeline(TimelineError::Capacity))
    ));
    assert!(queue.modifiers().contains(&key("left_shift")));
    assert_eq!(queue.delivery_progress(BatchId(2)), None);
    for id in 100..108 {
        queue.cancel_batch(BatchId(id));
    }
    queue.enqueue_scoped(&scope(cfg, 2)).unwrap();
    opening(&mut queue);
}
#[test]
fn duplicate_wrong_slots_or_old_arc_marker_cannot_advance_delivery_and_cancel_discards_close() {
    let (mut queue, cfg) = fixture("right_control");
    queue.enqueue_scoped(&scope(cfg.clone(), 2)).unwrap();
    let due = opening(&mut queue);
    assert_eq!(
        queue.delivery.delivered_scope(&due.event),
        Err(GeneratedError::Lifecycle)
    );
    let mut close = due.event.clone();
    close.payload.opening = false;
    close.payload.slots.clear();
    assert_eq!(
        queue.delivery.delivered_scope(&close),
        Err(GeneratedError::Lifecycle)
    );
    close.payload.slots = due.event.payload.slots.clone();
    close.config = Arc::new((*cfg).clone());
    assert_eq!(
        queue.delivery.delivered_scope(&close),
        Err(GeneratedError::Provenance)
    );
    assert!(!queue.scope_delivery_complete(BatchId(2)));
    queue.cancel_batch(BatchId(2));
    assert!(queue.modifiers().contains(&key("left_shift")));
    assert_eq!(queue.queued_events(), 0);
    assert_eq!(queue.delivery_progress(BatchId(2)), None);
}
#[test]
fn early_original_up_keeps_close_after_saved_duration_and_uses_new_trigger_arc_with_old_consumed_slot()
 {
    let (mut queue, old) = fixture("right_control");
    let mut cfg = (*old).clone();
    cfg.generation = ConfigGeneration(2);
    cfg.simple[0].to.insert(
        0,
        Action {
            hold_down_ns: 50,
            ..key("x").into()
        },
    );
    let cfg = Arc::new(cfg);
    queue.activate(cfg.clone()).unwrap();
    let mut saved = queue.enqueue_scoped(&scope(cfg.clone(), 2)).unwrap();
    let open = opening(&mut queue);
    assert!(Arc::ptr_eq(&open.event.config, &cfg));
    assert!(matches!(
        queue.poll_mixed(MonoTime(100)).unwrap(),
        Some(GeneratedDelivery::Action(_))
    ));
    queue.input_up_scoped(&mut saved, MonoTime(110)).unwrap();
    assert!(queue.poll_mixed(MonoTime(149)).unwrap().is_none());
    assert!(matches!(
        queue.poll_mixed(MonoTime(150)).unwrap(),
        Some(GeneratedDelivery::Action(_))
    ));
    for edge in [KeyEdge::Down, KeyEdge::Up] {
        let GeneratedDelivery::Action(due) = queue.poll_mixed(MonoTime(150)).unwrap().unwrap()
        else {
            panic!("saved modifier")
        };
        assert_eq!(due.event.payload.edge, edge);
        assert_eq!(
            due.event.payload.target,
            ActionTarget::Key(key("right_control"))
        );
    }
    let GeneratedDelivery::Scope(close) = queue.poll_mixed(MonoTime(150)).unwrap().unwrap() else {
        panic!("close")
    };
    assert_eq!(close.event.at_ns, MonoTime(150));
    assert!(!close.event.payload.opening);
    queue.retire_batch(BatchId(2)).unwrap();
}
#[test]
fn retained_scope_reference_budget_rejects_before_logical_and_fifo_commit() {
    let (mut queue, cfg) = fixture("right_control");
    // An isolated delivery-ledger fixture fills the retained reference budget;
    // no fabricated slot is dispatched to a downstream actor.
    let plan = plan_actions(&[key("vk_none").into()], origin(&cfg, 100, 0), false).unwrap();
    let slots = (0..MAX_PERSISTENT_SUPPRESSION_REFERENCES)
        .map(|index| ActionSlot {
            batch: BatchId(1),
            index: index as u16,
        })
        .collect();
    queue
        .delivery
        .prepare(plan.events(), None)
        .unwrap()
        .with_scope(BatchId(100), slots)
        .unwrap()
        .commit();
    assert!(matches!(
        queue.prepare_scoped(&scope(cfg.clone(), 2)),
        Err(GeneratedError::Engine(EngineError::ResourceLimit))
    ));
    assert!(queue.modifiers().contains(&key("left_shift")));
    assert_eq!(queue.queued_events(), 0);
    assert_eq!(queue.delivery_progress(BatchId(2)), None);
    queue.delivery.cancel(BatchId(100));
    queue.enqueue_scoped(&scope(cfg, 2)).unwrap();
    opening(&mut queue);
}
#[test]
fn maximum_action_array_preserves_original_limit_and_two_controls_share_queue_budget() {
    let (mut queue, old) = fixture("b");
    let mut cfg = (*old).clone();
    cfg.generation = ConfigGeneration(2);
    cfg.simple[0].to = vec![
        Action {
            repeat: false,
            ..key("b").into()
        };
        512
    ];
    let cfg = Arc::new(cfg);
    queue.activate(cfg.clone()).unwrap();
    queue.enqueue_scoped(&scope(cfg, 2)).unwrap();
    assert_eq!(queue.queued_events(), 1026);
    opening(&mut queue);
    for _ in 0..1024 {
        assert!(matches!(
            queue.poll_mixed(MonoTime(100)).unwrap(),
            Some(GeneratedDelivery::Action(_))
        ));
    }
    assert!(matches!(
        queue.poll_mixed(MonoTime(100)).unwrap(),
        Some(GeneratedDelivery::Scope(_))
    ));
    queue.retire_batch(BatchId(2)).unwrap();
    assert_eq!(queue.queued_events(), 0);
}
