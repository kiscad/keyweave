#![allow(clippy::unwrap_used)]
use keyweave_engine::{physical_controller::*, pointer_work::PointerMotion, timeline::DueEvent, *};
use keyweave_linux::{events::Event, relative::*, relative_pending::*};
use keyweave_types::*;
use std::sync::Arc;
fn cfg() -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn frame() -> RelativeFrame {
    RelativeFrame::prepare(
        &RelativeAxes::new(&SUPPORTED_AXES).unwrap(),
        &[
            Event::output(EV_REL, REL_X, 1),
            Event::output(EV_REL, REL_X, -1),
            Event::output(EV_REL, REL_WHEEL, 1),
            Event::output(EV_REL, REL_WHEEL_HI_RES, 13),
        ],
    )
    .unwrap()
}
fn input(seq: u64, at: u64, dev: DeviceInstanceId, event: InputEvent) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(at),
        device: dev,
        provenance: Provenance::Physical,
        event,
    }
}
fn key(seq: u64, at: u64, name: &str, edge: KeyEdge) -> InputEnvelope {
    input(
        seq,
        at,
        device(1),
        InputEvent::Key {
            key: name.parse().unwrap(),
            edge,
        },
    )
}
fn drain(
    actor: &mut PhysicalStageController,
    at: u64,
) -> (Vec<OutputIntent>, Vec<DueEvent<PointerMotion>>) {
    let mut outputs = vec![];
    let mut receipts = vec![];
    for _ in 0..1000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return (outputs, receipts);
        };
        outputs.extend(step.outputs);
        receipts.extend(step.forwarded_pointers);
    }
    panic!("controller did not stabilize")
}
fn register(
    actor: &mut PhysicalStageController,
    cache: &mut PendingRelativeFrames,
    seq: u64,
    at: u64,
    dev: DeviceInstanceId,
    frame: RelativeFrame,
) -> SourceToken {
    cache.can_accept(&frame).unwrap();
    let config = actor.snapshot().clone();
    let step = actor
        .handle(input(
            seq,
            at,
            dev,
            PendingRelativeFrames::input_event(&frame),
        ))
        .unwrap();
    let source = step.registered_pointer.unwrap();
    cache
        .register(source, dev, MonoTime(at), config, frame)
        .unwrap();
    source
}
fn copy(receipt: &DueEvent<PointerMotion>) -> DueEvent<PointerMotion> {
    DueEvent {
        event: receipt.event.clone(),
        delivered_at: receipt.delivered_at,
    }
}
#[test]
fn actual_registered_source_returns_exact_legacy_hires_packet_once() {
    let mut actor = PhysicalStageController::new_with_chord_model(cfg()).unwrap();
    let mut cache = PendingRelativeFrames::new();
    let original = frame();
    let source = register(&mut actor, &mut cache, 1, 100, device(1), original.clone());
    let (_, receipts) = drain(&mut actor, 100);
    assert_eq!(receipts.len(), 1);
    assert_eq!(source, receipts[0].event.source);
    assert_eq!(
        cache.take(&receipts[0]).unwrap().records(),
        original.records()
    );
    assert_eq!(cache.pending_records(), 0);
    assert_eq!(cache.take(&receipts[0]).unwrap_err(), PendingError::Receipt);
}
#[test]
fn delayed_packet_preserves_its_raw_frame_and_old_arc_after_reload() {
    let mut config = (*cfg()).clone();
    config.simple.push(Mapping {
        id: "timed".into(),
        from: "a".parse().unwrap(),
        to: vec![
            Action {
                hold_down_ns: 50,
                .."b".parse::<KeyId>().unwrap().into()
            },
            "c".parse::<KeyId>().unwrap().into(),
        ],
        allow_extra_modifiers: true,
        extras: Default::default(),
    });
    let config = Arc::new(config);
    let mut actor = PhysicalStageController::new_with_chord_model(config.clone()).unwrap();
    let mut cache = PendingRelativeFrames::new();
    actor.handle(key(1, 100, "a", KeyEdge::Down)).unwrap();
    drain(&mut actor, 100);
    let source = register(&mut actor, &mut cache, 2, 120, device(1), frame());
    assert!(drain(&mut actor, 120).1.is_empty());
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let (_, receipts) = drain(&mut actor, 150);
    assert_eq!(receipts[0].event.source, source);
    assert!(Arc::ptr_eq(&receipts[0].event.config, &config));
    assert!(receipts[0].event.at_ns >= MonoTime(120));
    assert_eq!(
        cache.take(&receipts[0]).unwrap().records(),
        frame().records()
    );
}
#[test]
fn forged_receipt_metadata_fails_cold_and_preserves_original_packet() {
    let config = cfg();
    let mut actor = PhysicalStageController::new_with_chord_model(config.clone()).unwrap();
    let mut cache = PendingRelativeFrames::new();
    register(&mut actor, &mut cache, 1, 100, device(1), frame());
    let (_, receipts) = drain(&mut actor, 100);
    let original = &receipts[0];
    let mut cases = vec![];
    let mut bad = copy(original);
    bad.event.source = SourceToken(1);
    cases.push(bad);
    let mut bad = copy(original);
    bad.event.device = device(2);
    cases.push(bad);
    let mut bad = copy(original);
    bad.event.config = Arc::new((*config).clone());
    cases.push(bad);
    let mut bad = copy(original);
    bad.event.at_ns = MonoTime(99);
    cases.push(bad);
    let mut bad = copy(original);
    bad.delivered_at = MonoTime(99);
    cases.push(bad);
    let mut bad = copy(original);
    bad.event.payload = PointerMotion::Activity {
        moved: true,
        scrolled: false,
    };
    cases.push(bad);
    for bad in cases {
        assert_eq!(cache.take(&bad).unwrap_err(), PendingError::Receipt);
        assert_eq!(cache.pending_frames(), 1);
    }
    assert_eq!(cache.take(original).unwrap().records(), frame().records());
}
#[test]
fn device_controls_discard_only_matching_generation_and_keep_other_packet() {
    let mut actor = PhysicalStageController::new_with_chord_model(cfg()).unwrap();
    let mut cache = PendingRelativeFrames::new();
    register(&mut actor, &mut cache, 1, 100, device(1), frame());
    let old = drain(&mut actor, 100).1;
    register(&mut actor, &mut cache, 2, 110, device(2), frame());
    let kept = drain(&mut actor, 110).1;
    cache.discard_device(DeviceInstanceId {
        id: 1,
        generation: 2,
    });
    assert_eq!(cache.pending_frames(), 2);
    cache.discard_older(DeviceInstanceId {
        id: 1,
        generation: 2,
    });
    assert_eq!(cache.pending_frames(), 1);
    assert!(cache.take(&old[0]).is_err());
    cache.take(&kept[0]).unwrap();
    assert_eq!(cache.pending_records(), 0);
}
#[test]
fn reset_discard_keeps_source_cursor_and_actual_controller_never_reuses_it() {
    let config = cfg();
    let mut actor = PhysicalStageController::new_with_chord_model(config.clone()).unwrap();
    let mut cache = PendingRelativeFrames::new();
    let source = register(&mut actor, &mut cache, 1, 100, device(1), frame());
    actor
        .handle(input(2, 110, device(1), InputEvent::Reset))
        .unwrap();
    cache.discard_all();
    assert_eq!(
        cache.register(source, device(1), MonoTime(110), config, frame()),
        Err(PendingError::Source)
    );
    let next = register(&mut actor, &mut cache, 3, 120, device(1), frame());
    assert!(next > source);
    let (_, receipts) = drain(&mut actor, 120);
    cache.take(&receipts[0]).unwrap();
}
#[test]
fn packet_budget_failure_is_cold_and_clear_does_not_reopen_source_identity() {
    let mut cache = PendingRelativeFrames::new();
    let config = cfg();
    let pair = RelativeFrame::prepare(
        &RelativeAxes::new(&[REL_X]).unwrap(),
        &[Event::output(EV_REL, REL_X, 1); 2],
    )
    .unwrap();
    for index in 1..=4096 {
        cache
            .register(
                SourceToken((1 << 63) | index),
                device(1),
                MonoTime(0),
                config.clone(),
                pair.clone(),
            )
            .unwrap();
    }
    assert_eq!(cache.pending_records(), 8192);
    assert_eq!(cache.can_accept(&pair), Err(PendingError::Capacity));
    assert_eq!(
        cache.register(
            SourceToken((1 << 63) | 4097),
            device(1),
            MonoTime(0),
            config.clone(),
            pair.clone()
        ),
        Err(PendingError::Capacity)
    );
    assert_eq!(cache.pending_frames(), 4096);
    cache.discard_all();
    assert_eq!(
        cache.register(
            SourceToken(1),
            device(1),
            MonoTime(0),
            config.clone(),
            pair.clone()
        ),
        Err(PendingError::Source)
    );
    cache
        .register(
            SourceToken((1 << 63) | 4097),
            device(1),
            MonoTime(0),
            config,
            pair,
        )
        .unwrap();
}
#[test]
fn activity_qualifiers_match_legacy_and_staged_alone_in_simple_and_complex() {
    for complex in [false, true] {
        for (record, canceled) in [
            (Event::output(EV_REL, REL_X, 1), false),
            (Event::output(EV_REL, REL_WHEEL_HI_RES, 1), true),
            (Event::output(EV_REL, REL_WHEEL_HI_RES, 0), false),
        ] {
            let mut config = (*cfg()).clone();
            let rule = Mapping {
                id: "alone".into(),
                from: "a".parse().unwrap(),
                to: vec![],
                allow_extra_modifiers: true,
                extras: MappingExtras {
                    if_alone: vec!["c".parse::<KeyId>().unwrap().into()],
                    ..Default::default()
                },
            };
            if complex {
                config.complex.push(rule);
            } else {
                config.simple.push(rule);
            }
            let config = Arc::new(config);
            let frame =
                RelativeFrame::prepare(&RelativeAxes::new(&SUPPORTED_AXES).unwrap(), &[record])
                    .unwrap();
            let activity = PendingRelativeFrames::input_event(&frame);
            let mut legacy = Engine::new(config.clone()).unwrap();
            legacy.handle(key(1, 100, "a", KeyEdge::Down)).unwrap();
            legacy
                .handle(input(2, 110, device(1), activity.clone()))
                .unwrap();
            let expected = legacy
                .handle(key(3, 120, "a", KeyEdge::Up))
                .unwrap()
                .outputs;
            let mut actor = PhysicalStageController::new_with_chord_model(config).unwrap();
            actor.handle(key(1, 100, "a", KeyEdge::Down)).unwrap();
            drain(&mut actor, 100);
            actor.handle(input(2, 110, device(1), activity)).unwrap();
            drain(&mut actor, 110);
            let mut actual = actor.handle(key(3, 120, "a", KeyEdge::Up)).unwrap().outputs;
            actual.extend(drain(&mut actor, 120).0);
            assert_eq!(actual, expected);
            assert_eq!(actual.len(), if canceled { 0 } else { 2 });
        }
    }
}
#[test]
fn injected_activity_has_no_registered_pointer_and_does_not_consume_source() {
    let mut actor = PhysicalStageController::new_with_chord_model(cfg()).unwrap();
    let mut injected = input(
        1,
        100,
        device(1),
        PendingRelativeFrames::input_event(&frame()),
    );
    injected.provenance = Provenance::OtherInjected;
    let step = actor.handle(injected).unwrap();
    assert_eq!(step.capture, CaptureDecision::Pass);
    assert_eq!(step.registered_pointer, None);
    let step = actor
        .handle(input(
            2,
            110,
            device(1),
            PendingRelativeFrames::input_event(&frame()),
        ))
        .unwrap();
    assert_eq!(step.registered_pointer, Some(SourceToken((1 << 63) | 1)));
}
#[test]
fn device_or_reset_cancellation_drops_future_packet_without_fabricating_forwarded_receipt() {
    for reset in [false, true] {
        let mut config = (*cfg()).clone();
        config.simple.push(Mapping {
            id: "timed".into(),
            from: "a".parse().unwrap(),
            to: vec![
                Action {
                    hold_down_ns: 50,
                    .."b".parse::<KeyId>().unwrap().into()
                },
                "c".parse::<KeyId>().unwrap().into(),
            ],
            allow_extra_modifiers: true,
            extras: Default::default(),
        });
        let mut actor = PhysicalStageController::new_with_chord_model(Arc::new(config)).unwrap();
        let mut cache = PendingRelativeFrames::new();
        actor.handle(key(1, 100, "a", KeyEdge::Down)).unwrap();
        drain(&mut actor, 100);
        register(&mut actor, &mut cache, 2, 120, device(1), frame());
        let step = actor
            .handle(input(
                3,
                130,
                device(1),
                if reset {
                    InputEvent::Reset
                } else {
                    InputEvent::DeviceRemoved
                },
            ))
            .unwrap();
        assert!(step.forwarded_pointers.is_empty());
        if reset {
            cache.discard_all();
        } else {
            cache.discard_device(device(1));
        }
        assert!(drain(&mut actor, 200).1.is_empty());
        assert_eq!(cache.pending_records(), 0);
    }
}
