#![allow(clippy::unwrap_used)]
use keyweave_engine::{ConfigSnapshot, timeline::*};
use keyweave_types::*;
use std::sync::Arc;
fn device(generation: u64) -> DeviceInstanceId {
    DeviceInstanceId { id: 1, generation }
}
fn config(generation: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "timeline".into(),
        simple: vec![],
        complex: vec![],
    })
}
fn event(stage: Stage, at: u64, payload: u64) -> StageEvent<u64> {
    StageEvent {
        stage,
        at_ns: MonoTime(at),
        device: device(1),
        source: SourceToken(1),
        config: config(1),
        payload,
    }
}
fn queue() -> StageTimeline<u64> {
    let mut queue = StageTimeline::new();
    queue.observe_device(device(1), false).unwrap();
    queue
}
#[test]
fn future_up_does_not_block_downs_child_stage_but_same_stage_keeps_fifo() {
    let mut queue = queue();
    queue
        .enqueue(vec![
            event(Stage::Complex, 10, 1),
            event(Stage::Complex, 510, 2),
        ])
        .unwrap();
    let down = queue.pop_due(MonoTime(10)).unwrap().unwrap();
    assert_eq!(down.event.payload, 1);
    queue
        .enqueue(vec![
            event(Stage::Output, 10, 3),
            event(Stage::Complex, 20, 4),
        ])
        .unwrap();
    assert_eq!(
        queue.pop_due(MonoTime(10)).unwrap().unwrap().event.payload,
        3
    );
    assert!(queue.pop_due(MonoTime(509)).unwrap().is_none());
    assert_eq!(
        queue.pop_due(MonoTime(510)).unwrap().unwrap().event.payload,
        2
    );
    let next = queue.pop_due(MonoTime(510)).unwrap().unwrap();
    assert_eq!(next.event.payload, 4);
    assert_eq!(next.event.at_ns, MonoTime(510));
}
#[test]
fn equal_time_across_stages_uses_causal_enqueue_order() {
    let mut queue = queue();
    queue
        .enqueue(vec![
            event(Stage::Output, 0, 1),
            event(Stage::Simple, 0, 2),
            event(Stage::Complex, 0, 3),
        ])
        .unwrap();
    let mut seen = vec![];
    while let Some(due) = queue.pop_due(MonoTime(0)).unwrap() {
        seen.push(due.event.payload);
    }
    assert_eq!(seen, [1, 2, 3]);
}
#[test]
fn deferred_work_retains_original_snapshot_source_and_device() {
    let mut queue = queue();
    let old = config(7);
    let mut work = event(Stage::Complex, 100, 42);
    work.config = old.clone();
    work.source = SourceToken(99);
    queue.enqueue(vec![work]).unwrap();
    let new = config(8);
    assert_eq!(new.generation, ConfigGeneration(8));
    let due = queue.pop_due(MonoTime(120)).unwrap().unwrap();
    assert!(Arc::ptr_eq(&due.event.config, &old));
    assert_eq!(due.event.source, SourceToken(99));
    assert_eq!(due.event.device, device(1));
    assert_eq!(due.event.at_ns, MonoTime(100));
    assert_eq!(due.delivered_at, MonoTime(120));
}
#[test]
fn removed_or_replaced_device_cancels_only_its_unexecuted_work() {
    let mut queue = queue();
    let other = DeviceInstanceId {
        id: 2,
        generation: 1,
    };
    queue.observe_device(other, false).unwrap();
    let mut retained = event(Stage::Output, 0, 2);
    retained.device = other;
    queue
        .enqueue(vec![event(Stage::Complex, 0, 1), retained])
        .unwrap();
    let canceled = queue.observe_device(device(2), false).unwrap();
    assert_eq!(canceled.len(), 1);
    assert_eq!(canceled[0].payload, 1);
    assert_eq!(
        queue.observe_device(device(1), true).unwrap_err(),
        TimelineError::RetiredDevice
    );
    assert_eq!(
        queue.pop_due(MonoTime(0)).unwrap().unwrap().event.device,
        other
    );
    let mut new = event(Stage::Complex, 0, 3);
    new.device = device(2);
    queue.enqueue(vec![new]).unwrap();
    assert_eq!(queue.observe_device(device(2), true).unwrap()[0].payload, 3);
    assert!(queue.observe_device(device(2), true).unwrap().is_empty());
    assert_eq!(
        queue.observe_device(device(2), false).unwrap_err(),
        TimelineError::RetiredDevice
    );
    assert!(queue.is_empty());
}
#[test]
fn source_cancellation_returns_payloads_in_effective_time_and_enqueue_order() {
    let mut queue = queue();
    let mut retained = event(Stage::Output, 10, 4);
    retained.source = SourceToken(2);
    queue
        .enqueue(vec![
            event(Stage::Simple, 50, 1),
            event(Stage::Complex, 10, 2),
            event(Stage::Output, 10, 3),
            retained,
        ])
        .unwrap();
    let canceled = queue.discard_source(device(1), SourceToken(1));
    assert_eq!(
        canceled
            .iter()
            .map(|event| event.payload)
            .collect::<Vec<_>>(),
        [2, 3, 1]
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue.pop_due(MonoTime(10)).unwrap().unwrap().event.payload,
        4
    );
}
#[test]
fn mixed_invalid_batch_returns_all_original_payloads_without_changing_queue() {
    let mut queue = queue();
    let mut bad = event(Stage::Complex, 0, 2);
    bad.device = device(2);
    let rejected = queue
        .enqueue(vec![event(Stage::Simple, 100, 1), bad])
        .unwrap_err();
    assert_eq!(rejected.error, TimelineError::RetiredDevice);
    assert_eq!(rejected.events.len(), 2);
    assert_eq!(rejected.events[0].at_ns, MonoTime(100));
    assert!(queue.is_empty());
    queue.enqueue(vec![event(Stage::Simple, 0, 3)]).unwrap();
    assert_eq!(queue.next_deadline(), Some(MonoTime(0)));
}
#[test]
fn queue_capacity_and_batch_limits_reject_atomically_and_reset_returns_work() {
    let mut queue = queue();
    let cfg = config(1);
    for _ in 0..MAX_STAGE_EVENTS / MAX_STAGE_APPEND {
        let batch = (0..MAX_STAGE_APPEND)
            .map(|i| StageEvent {
                config: cfg.clone(),
                ..event(Stage::Simple, 100, i as u64)
            })
            .collect();
        queue.enqueue(batch).unwrap();
    }
    assert_eq!(queue.len(), MAX_STAGE_EVENTS);
    let rejected = queue
        .enqueue(vec![event(Stage::Output, 0, 99)])
        .unwrap_err();
    assert_eq!(rejected.error, TimelineError::Capacity);
    assert_eq!(rejected.events[0].payload, 99);
    assert_eq!(queue.len(), MAX_STAGE_EVENTS);
    assert_eq!(queue.reset(MonoTime(1)).unwrap().len(), MAX_STAGE_EVENTS);
    assert!(queue.is_empty());
    assert_eq!(queue.known_devices(), 1);
    let rejected = queue
        .enqueue(
            (0..MAX_STAGE_APPEND + 1)
                .map(|i| event(Stage::Output, 0, i as u64))
                .collect(),
        )
        .unwrap_err();
    assert_eq!(rejected.error, TimelineError::Capacity);
    assert!(queue.is_empty());
}
#[test]
fn clock_reversal_does_not_pop_or_reset_queued_work() {
    let mut queue = queue();
    queue.enqueue(vec![event(Stage::Output, 100, 1)]).unwrap();
    assert!(queue.pop_due(MonoTime(10)).unwrap().is_none());
    assert_eq!(
        queue.pop_due(MonoTime(9)).unwrap_err(),
        TimelineError::TimeReversed
    );
    assert_eq!(
        queue.reset(MonoTime(9)).unwrap_err(),
        TimelineError::TimeReversed
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(
        queue.pop_due(MonoTime(100)).unwrap().unwrap().event.payload,
        1
    );
}
#[test]
fn reset_keeps_retired_generation_and_device_history_is_bounded() {
    let mut queue = queue();
    queue.observe_device(device(1), true).unwrap();
    queue.reset(MonoTime(0)).unwrap();
    assert_eq!(
        queue
            .enqueue(vec![event(Stage::Simple, 0, 1)])
            .unwrap_err()
            .error,
        TimelineError::RetiredDevice
    );
    for id in 2..=keyweave_engine::MAX_DEVICE_IDENTITIES as u64 {
        queue
            .observe_device(DeviceInstanceId { id, generation: 1 }, true)
            .unwrap();
    }
    assert_eq!(
        queue
            .observe_device(
                DeviceInstanceId {
                    id: 5000,
                    generation: 1
                },
                false
            )
            .unwrap_err(),
        TimelineError::DeviceHistoryFull
    );
    assert_eq!(
        queue.known_devices(),
        keyweave_engine::MAX_DEVICE_IDENTITIES
    );
    queue.observe_device(device(2), false).unwrap();
    let mut work = event(Stage::Simple, 0, 2);
    work.device = device(2);
    queue.enqueue(vec![work]).unwrap();
}

#[test]
fn a_blocked_batch_preserves_delays_and_shifts_each_stage_independently() {
    let mut queue = queue();
    queue
        .enqueue(vec![
            event(Stage::Simple, 100, 1),
            event(Stage::Output, 20, 2),
        ])
        .unwrap();
    queue
        .enqueue(vec![
            event(Stage::Simple, 0, 3),
            event(Stage::Output, 0, 4),
            event(Stage::Simple, 50, 5),
            event(Stage::Output, 10, 6),
        ])
        .unwrap();
    let mut seen = vec![];
    for at in [20, 30, 100, 150] {
        while let Some(due) = queue.pop_due(MonoTime(at)).unwrap() {
            seen.push((due.event.at_ns.0, due.event.payload));
        }
    }
    assert_eq!(
        seen,
        [(20, 2), (20, 4), (30, 6), (100, 1), (100, 3), (150, 5)]
    );
}
#[test]
fn late_batch_keeps_relative_duration_after_the_current_actor_clock() {
    let mut queue = queue();
    queue.advance(MonoTime(100)).unwrap();
    queue
        .enqueue(vec![
            event(Stage::Complex, 0, 1),
            event(Stage::Complex, 50, 2),
        ])
        .unwrap();
    assert_eq!(
        queue.pop_due(MonoTime(100)).unwrap().unwrap().event.at_ns,
        MonoTime(100)
    );
    assert!(queue.pop_due(MonoTime(149)).unwrap().is_none());
    assert_eq!(
        queue.pop_due(MonoTime(150)).unwrap().unwrap().event.at_ns,
        MonoTime(150)
    );
}
#[test]
fn shift_overflow_rejects_whole_original_batch_and_leaves_queue_unchanged() {
    let mut queue = queue();
    queue.enqueue(vec![event(Stage::Output, 100, 1)]).unwrap();
    let rejected = queue
        .enqueue(vec![
            event(Stage::Output, 0, 2),
            event(Stage::Output, u64::MAX, 3),
        ])
        .unwrap_err();
    assert_eq!(rejected.error, TimelineError::TimeOverflow);
    assert_eq!(
        rejected
            .events
            .iter()
            .map(|e| (e.at_ns.0, e.payload))
            .collect::<Vec<_>>(),
        [(0, 2), (u64::MAX, 3)]
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.next_deadline(), Some(MonoTime(100)));
    assert_eq!(
        queue.pop_due(MonoTime(100)).unwrap().unwrap().event.payload,
        1
    );
    assert!(queue.is_empty());
}

#[test]
fn prepared_abort_returns_original_nonclone_payload_and_metadata_without_time_shift() {
    struct Payload(u64);
    let mut queue = StageTimeline::new();
    queue.observe_device(device(1), false).unwrap();
    queue.advance(MonoTime(100)).unwrap();
    let old = config(7);
    let work = StageEvent {
        stage: Stage::Output,
        at_ns: MonoTime(10),
        device: device(1),
        source: SourceToken(99),
        config: old.clone(),
        payload: Payload(42),
    };
    let prepared = match queue.prepare_enqueue(vec![work]) {
        Ok(prepared) => prepared,
        Err(_) => panic!("valid admission"),
    };
    let events = prepared.abort();
    assert!(queue.is_empty());
    assert_eq!(events[0].at_ns, MonoTime(10));
    assert_eq!(events[0].payload.0, 42);
    assert_eq!(events[0].source, SourceToken(99));
    assert!(Arc::ptr_eq(&events[0].config, &old));
    assert_eq!(
        queue.advance(MonoTime(99)),
        Err(TimelineError::TimeReversed)
    );
    let prepared = match queue.prepare_enqueue(events) {
        Ok(prepared) => prepared,
        Err(_) => panic!("retry admission"),
    };
    prepared.commit();
    let due = queue.pop_due(MonoTime(100)).unwrap().unwrap();
    assert_eq!(due.event.at_ns, MonoTime(100));
    assert_eq!(due.event.payload.0, 42);
}

#[test]
fn second_queue_overflow_aborts_first_admission_and_retry_preserves_duration() {
    let mut first = queue();
    let mut second = queue();
    first.enqueue(vec![event(Stage::Output, 100, 1)]).unwrap();
    second.advance(MonoTime(u64::MAX)).unwrap();
    let prepared = first
        .prepare_enqueue(vec![
            event(Stage::Output, 10, 2),
            event(Stage::Output, 60, 3),
        ])
        .unwrap();
    let rejection = second
        .prepare_enqueue(vec![event(Stage::Output, 0, 4), event(Stage::Output, 1, 5)])
        .err()
        .unwrap();
    assert_eq!(rejection.error, TimelineError::TimeOverflow);
    assert_eq!(rejection.events[1].at_ns, MonoTime(1));
    drop(prepared);
    assert_eq!(first.len(), 1);
    assert_eq!(first.next_deadline(), Some(MonoTime(100)));
    assert!(second.is_empty());
    first
        .prepare_enqueue(vec![
            event(Stage::Output, 10, 2),
            event(Stage::Output, 60, 3),
        ])
        .unwrap()
        .commit();
    assert_eq!(
        first.pop_due(MonoTime(100)).unwrap().unwrap().event.payload,
        1
    );
    let down = first.pop_due(MonoTime(100)).unwrap().unwrap();
    assert_eq!(down.event.payload, 2);
    assert_eq!(down.event.at_ns, MonoTime(100));
    assert!(first.pop_due(MonoTime(149)).unwrap().is_none());
    let up = first.pop_due(MonoTime(150)).unwrap().unwrap();
    assert_eq!(up.event.payload, 3);
    assert_eq!(up.event.at_ns, MonoTime(150));
}

#[test]
fn independent_admissions_commit_after_both_preflight_and_keep_causal_order() {
    let mut first = queue();
    let mut second = queue();
    let a = first
        .prepare_enqueue(vec![
            event(Stage::Output, 10, 1),
            event(Stage::Simple, 10, 2),
        ])
        .unwrap();
    let b = second
        .prepare_enqueue(vec![event(Stage::Complex, 10, 3)])
        .unwrap();
    b.commit();
    a.commit();
    assert_eq!(
        first.pop_due(MonoTime(10)).unwrap().unwrap().event.payload,
        1
    );
    assert_eq!(
        first.pop_due(MonoTime(10)).unwrap().unwrap().event.payload,
        2
    );
    assert_eq!(
        second.pop_due(MonoTime(10)).unwrap().unwrap().event.payload,
        3
    );
}
