#![allow(clippy::unwrap_used)]
use keyweave_engine::{physical_controller::*, timeline::TimelineError, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(id: u64, generation: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation }
}
fn mapping(from: &str, to: Vec<Action>) -> Mapping {
    Mapping {
        id: from.into(),
        from: key(from),
        to,
        allow_extra_modifiers: true,
        extras: Default::default(),
    }
}
fn config(simple: Vec<Mapping>, complex: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "physical-controller".into(),
        simple,
        complex,
    })
}
fn envelope(seq: u64, at: u64, dev: DeviceInstanceId, event: InputEvent) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(at),
        device: dev,
        provenance: Provenance::Physical,
        event,
    }
}
fn keyboard(seq: u64, at: u64, dev: DeviceInstanceId, name: &str, edge: KeyEdge) -> InputEnvelope {
    envelope(
        seq,
        at,
        dev,
        InputEvent::Key {
            key: key(name),
            edge,
        },
    )
}
fn edges(outputs: Vec<OutputIntent>) -> Vec<(KeyId, KeyEdge)> {
    outputs.into_iter().map(|o| (o.key, o.edge)).collect()
}
fn drain(actor: &mut PhysicalStageController, at: u64) -> Vec<(KeyId, KeyEdge)> {
    let mut outputs = vec![];
    for _ in 0..10000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return outputs;
        };
        assert_eq!(step.capture, CaptureDecision::Pass);
        assert_eq!(step.repeat_target, None);
        outputs.extend(edges(step.outputs));
    }
    panic!("bounded fixture failed to finish");
}
#[test]
fn physical_binding_follows_two_saved_mappings_repeat_and_up_across_reload() {
    let mut actor = PhysicalStageController::new(config(
        vec![mapping("a", vec![key("b").into()])],
        vec![mapping("b", vec![key("c").into()])],
    ))
    .unwrap();
    let dev = device(1, 1);
    let down = actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    assert!(down.outputs.is_empty());
    assert_eq!(down.capture, CaptureDecision::Capture);
    assert_eq!(actor.active_bindings(), 1);
    assert_eq!(drain(&mut actor, 100), vec![(key("c"), KeyEdge::Down)]);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    let repeated = actor
        .handle(keyboard(2, 110, dev, "a", KeyEdge::Repeat))
        .unwrap();
    assert_eq!(repeated.repeat_target, Some(key("c")));
    assert!(repeated.outputs.is_empty());
    assert_eq!(actor.active_bindings(), 1);
    actor
        .handle(keyboard(3, 120, dev, "a", KeyEdge::Up))
        .unwrap();
    assert_eq!(actor.active_bindings(), 0);
    assert_eq!(actor.pending_bindings(), 1);
    assert_eq!(drain(&mut actor, 120), vec![(key("c"), KeyEdge::Up)]);
    assert_eq!(actor.pending_bindings(), 0);
    actor
        .handle(keyboard(4, 130, dev, "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(drain(&mut actor, 130), vec![(key("a"), KeyEdge::Down)]);
}
#[test]
fn duplicate_down_unknown_up_and_repeat_never_create_owners_or_replay_macros() {
    let mut actor = PhysicalStageController::new(config(vec![], vec![])).unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    let duplicate = actor
        .handle(keyboard(2, 110, dev, "a", KeyEdge::Down))
        .unwrap();
    assert!(duplicate.outputs.is_empty());
    assert_eq!(actor.active_bindings(), 1);
    actor
        .handle(keyboard(3, 120, dev, "b", KeyEdge::Up))
        .unwrap();
    assert_eq!(actor.active_bindings(), 1);
    assert_eq!(
        actor
            .handle(keyboard(4, 130, dev, "b", KeyEdge::Repeat))
            .unwrap()
            .repeat_target,
        None
    );
    let reset = actor
        .handle(envelope(5, 140, dev, InputEvent::Reset))
        .unwrap();
    assert_eq!(edges(reset.outputs), vec![(key("a"), KeyEdge::Up)]);
    assert_eq!(actor.active_bindings(), 0);
}
#[test]
fn due_pending_does_not_consume_sequence_and_clock_or_sequence_rejection_is_cold() {
    let mut actor = PhysicalStageController::new(config(vec![], vec![])).unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(10, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    assert!(matches!(
        actor.handle(keyboard(11, 120, dev, "a", KeyEdge::Up)),
        Err(PhysicalControllerError::DeadlinePending)
    ));
    assert!(!actor.is_failed());
    drain(&mut actor, 100);
    actor
        .handle(keyboard(11, 120, dev, "a", KeyEdge::Up))
        .unwrap();
    assert_eq!(drain(&mut actor, 120), vec![(key("a"), KeyEdge::Up)]);
    assert!(matches!(
        actor.handle(keyboard(11, 130, dev, "a", KeyEdge::Down)),
        Err(PhysicalControllerError::Engine(
            EngineError::SequenceBackwards
        ))
    ));
    assert!(matches!(
        actor.poll(MonoTime(119)),
        Err(PhysicalControllerError::Engine(EngineError::TimeBackwards))
    ));
    assert!(!actor.is_failed());
    actor
        .handle(keyboard(12, 130, dev, "a", KeyEdge::Down))
        .unwrap();
}
#[test]
fn generation_replacement_releases_old_key_and_old_up_cannot_release_new_binding() {
    let mut actor = PhysicalStageController::new(config(vec![], vec![])).unwrap();
    actor
        .handle(keyboard(1, 100, device(1, 1), "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    let replacement = actor
        .handle(keyboard(2, 150, device(1, 2), "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(edges(replacement.outputs), vec![(key("a"), KeyEdge::Up)]);
    assert_eq!(drain(&mut actor, 150), vec![(key("a"), KeyEdge::Down)]);
    assert_eq!(actor.active_bindings(), 1);
    assert!(matches!(
        actor.handle(keyboard(3, 160, device(1, 1), "a", KeyEdge::Up)),
        Err(PhysicalControllerError::Timeline(
            TimelineError::RetiredDevice
        ))
    ));
    actor
        .handle(keyboard(3, 160, device(1, 2), "a", KeyEdge::Up))
        .unwrap();
    assert_eq!(drain(&mut actor, 160), vec![(key("a"), KeyEdge::Up)]);
}
#[test]
fn shared_key_owners_and_device_removal_keep_other_physical_source_live() {
    let mut actor = PhysicalStageController::new(config(
        vec![
            mapping("a", vec![key("c").into()]),
            mapping("b", vec![key("c").into()]),
        ],
        vec![],
    ))
    .unwrap();
    actor
        .handle(keyboard(1, 100, device(1, 1), "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(drain(&mut actor, 100), vec![(key("c"), KeyEdge::Down)]);
    actor
        .handle(keyboard(2, 110, device(2, 1), "b", KeyEdge::Down))
        .unwrap();
    assert!(drain(&mut actor, 110).is_empty());
    let removed = actor
        .handle(envelope(3, 120, device(1, 1), InputEvent::DeviceRemoved))
        .unwrap();
    assert!(removed.outputs.is_empty());
    assert_eq!(actor.active_bindings(), 1);
    assert_eq!(
        actor
            .handle(keyboard(4, 130, device(2, 1), "b", KeyEdge::Repeat))
            .unwrap()
            .repeat_target,
        Some(key("c"))
    );
    actor
        .handle(keyboard(5, 140, device(2, 1), "b", KeyEdge::Up))
        .unwrap();
    assert_eq!(drain(&mut actor, 140), vec![(key("c"), KeyEdge::Up)]);
}
#[test]
fn reset_precedes_due_work_cancels_future_macro_and_preserves_device_tombstone() {
    let mut actor = PhysicalStageController::new(config(
        vec![mapping(
            "a",
            vec![
                Action {
                    repeat: false,
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("c").into(),
            ],
        )],
        vec![],
    ))
    .unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(drain(&mut actor, 100), vec![(key("b"), KeyEdge::Down)]);
    let reset = actor
        .handle(envelope(2, 150, dev, InputEvent::Reset))
        .unwrap();
    assert_eq!(edges(reset.outputs), vec![(key("b"), KeyEdge::Up)]);
    assert_eq!(actor.next_deadline(), None);
    assert!(drain(&mut actor, 200).is_empty());
    actor
        .handle(envelope(3, 210, dev, InputEvent::DeviceRemoved))
        .unwrap();
    actor
        .handle(envelope(4, 220, dev, InputEvent::Reset))
        .unwrap();
    assert!(matches!(
        actor.handle(keyboard(5, 230, dev, "a", KeyEdge::Down)),
        Err(PhysicalControllerError::Timeline(
            TimelineError::RetiredDevice
        ))
    ));
    actor
        .handle(keyboard(5, 230, device(1, 2), "a", KeyEdge::Down))
        .unwrap();
}
#[test]
fn up_after_delayed_empty_main_keeps_pending_root_then_same_key_can_press_again() {
    let mut mapping = mapping("b", vec![]);
    mapping.extras.delayed = Some(DelayedAction {
        delay_ns: 50,
        if_invoked: vec![Action {
            hold_down_ns: 10,
            ..key("h").into()
        }],
        if_canceled: vec![],
    });
    let mut actor = PhysicalStageController::new(config(
        vec![crate::mapping("a", vec![key("b").into()])],
        vec![mapping],
    ))
    .unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    actor
        .handle(keyboard(2, 120, dev, "a", KeyEdge::Up))
        .unwrap();
    drain(&mut actor, 120);
    assert_eq!(actor.active_bindings(), 0);
    assert_eq!(actor.pending_bindings(), 1);
    assert_eq!(drain(&mut actor, 150), vec![(key("h"), KeyEdge::Down)]);
    assert_eq!(actor.pending_bindings(), 1);
    assert_eq!(drain(&mut actor, 160), vec![(key("h"), KeyEdge::Up)]);
    assert_eq!(actor.pending_bindings(), 0);
    actor
        .handle(keyboard(3, 170, dev, "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(actor.active_bindings(), 1);
}
#[test]
fn repeat_uses_current_held_replacement_and_tap_or_modifier_has_no_repeat_target() {
    let mut mapping = mapping("b", vec![key("c").into()]);
    mapping.extras.held_threshold_ns = 50;
    mapping.extras.if_held_down.push(key("h").into());
    let mut actor = PhysicalStageController::new(config(
        vec![crate::mapping("a", vec![key("b").into()])],
        vec![mapping],
    ))
    .unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    assert_eq!(
        actor
            .handle(keyboard(2, 110, dev, "a", KeyEdge::Repeat))
            .unwrap()
            .repeat_target,
        Some(key("c"))
    );
    drain(&mut actor, 150);
    assert_eq!(
        actor
            .handle(keyboard(3, 160, dev, "a", KeyEdge::Repeat))
            .unwrap()
            .repeat_target,
        Some(key("h"))
    );
    actor
        .handle(envelope(4, 170, dev, InputEvent::Reset))
        .unwrap();
    actor
        .handle(keyboard(5, 180, dev, "left_shift", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 180);
    assert_eq!(
        actor
            .handle(keyboard(6, 190, dev, "left_shift", KeyEdge::Repeat))
            .unwrap()
            .repeat_target,
        None
    );
    let mut tap = PhysicalStageController::new(config(
        vec![crate::mapping(
            "a",
            vec![Action {
                repeat: false,
                hold_down_ns: 10,
                ..key("b").into()
            }],
        )],
        vec![],
    ))
    .unwrap();
    tap.handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut tap, 100);
    assert_eq!(
        tap.handle(keyboard(2, 105, dev, "a", KeyEdge::Repeat))
            .unwrap()
            .repeat_target,
        None
    );
}
#[test]
fn injected_input_passes_without_creating_owners_or_accepting_context_or_device_control() {
    let mut actor = PhysicalStageController::new(config(vec![], vec![])).unwrap();
    let dev = device(1, 1);
    for (seq, provenance) in [(1, Provenance::OurVirtual), (2, Provenance::OtherInjected)] {
        let mut event = keyboard(seq, seq * 100, dev, "a", KeyEdge::Down);
        event.provenance = provenance;
        assert_eq!(actor.handle(event).unwrap().capture, CaptureDecision::Pass);
    }
    assert_eq!(actor.active_bindings(), 0);
    let mut event = envelope(3, 300, dev, InputEvent::DeviceRemoved);
    event.provenance = Provenance::OurVirtual;
    actor.handle(event).unwrap();
    actor
        .handle(keyboard(4, 310, dev, "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(drain(&mut actor, 310), vec![(key("a"), KeyEdge::Down)]);
}
fn context(revision: u64, at: u64, until: u64, app: &str) -> ContextSnapshot {
    ContextSnapshot {
        revision,
        provider: "fixture".into(),
        observed_at_ns: MonoTime(at),
        valid_until_ns: MonoTime(until),
        application: Some(ApplicationContext {
            bundle_identifier: None,
            file_path: Some(app.into()),
        }),
        ..Default::default()
    }
}
#[test]
fn context_changed_before_due_timer_supplies_fresh_branch_conditions_and_rejects_replay() {
    let mut mapping = mapping("a", vec![]);
    mapping.extras.delayed = Some(DelayedAction {
        delay_ns: 50,
        if_invoked: vec![Action {
            conditions: vec![Condition::Application {
                bundle_identifiers: vec![],
                file_paths: vec![Pattern::compile("allowed").unwrap()],
                unless: false,
            }],
            hold_down_ns: 10,
            ..key("h").into()
        }],
        if_canceled: vec![],
    });
    let mut actor = PhysicalStageController::new(config(vec![], vec![mapping])).unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    actor
        .handle(envelope(
            2,
            150,
            dev,
            InputEvent::ContextChanged {
                context: context(1, 150, 200, "allowed"),
            },
        ))
        .unwrap();
    assert_eq!(actor.context_revision(), 1);
    assert_eq!(drain(&mut actor, 150), vec![(key("h"), KeyEdge::Down)]);
    assert!(matches!(
        actor.handle(envelope(
            3,
            150,
            dev,
            InputEvent::ContextChanged {
                context: context(1, 150, 200, "allowed")
            }
        )),
        Err(PhysicalControllerError::Engine(EngineError::InvalidContext))
    ));
    assert!(!actor.is_failed());
    actor
        .handle(envelope(
            3,
            150,
            dev,
            InputEvent::ContextChanged {
                context: context(2, 150, 200, "other"),
            },
        ))
        .unwrap();
    assert_eq!(drain(&mut actor, 160), vec![(key("h"), KeyEdge::Up)]);
}
#[test]
fn wheel_cancels_alone_pointer_move_does_not_and_pointer_flushes_lazy_modifier() {
    let mut mapping = mapping("a", vec![]);
    mapping.extras.if_alone.push(key("h").into());
    let mut actor = PhysicalStageController::new(config(vec![], vec![mapping])).unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    assert_eq!(
        actor
            .handle(envelope(
                2,
                110,
                dev,
                InputEvent::PointerMove { x: 1, y: 2 }
            ))
            .unwrap()
            .capture,
        CaptureDecision::Pass
    );
    actor
        .handle(keyboard(3, 120, dev, "a", KeyEdge::Up))
        .unwrap();
    assert_eq!(
        drain(&mut actor, 120),
        vec![(key("h"), KeyEdge::Down), (key("h"), KeyEdge::Up)]
    );
    actor
        .handle(keyboard(4, 130, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 130);
    actor
        .handle(envelope(
            5,
            140,
            dev,
            InputEvent::Wheel {
                horizontal: 1,
                vertical: 0,
            },
        ))
        .unwrap();
    actor
        .handle(keyboard(6, 150, dev, "a", KeyEdge::Up))
        .unwrap();
    assert!(drain(&mut actor, 150).is_empty());
    let lazy = Action {
        lazy: true,
        ..key("left_shift").into()
    };
    let mut actor =
        PhysicalStageController::new(config(vec![], vec![crate::mapping("b", vec![lazy])]))
            .unwrap();
    actor
        .handle(keyboard(1, 100, dev, "b", KeyEdge::Down))
        .unwrap();
    assert!(drain(&mut actor, 100).is_empty());
    let pointer = actor
        .handle(envelope(
            2,
            110,
            dev,
            InputEvent::PointerMove { x: 1, y: 2 },
        ))
        .unwrap();
    assert_eq!(
        edges(pointer.outputs),
        vec![(key("left_shift"), KeyEdge::Down)]
    );
}
#[test]
fn external_tokens_are_rejected_without_consuming_sequence_or_cancelling_internal_work() {
    let mut actor = PhysicalStageController::new(config(vec![], vec![])).unwrap();
    let dev = device(1, 1);
    for event in [
        InputEvent::TimerFired {
            token: DeadlineToken(1),
        },
        InputEvent::CancelDelayed {
            token: DeadlineToken(1),
        },
    ] {
        assert!(matches!(
            actor.handle(envelope(1, 100, dev, event)),
            Err(PhysicalControllerError::UnsupportedControl)
        ));
    }
    assert!(!actor.is_failed());
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
}
#[test]
fn new_down_of_same_key_cancels_old_pending_delayed_root_after_up_without_aliasing_binding() {
    let mut mapping = mapping("a", vec![]);
    mapping.extras.delayed = Some(DelayedAction {
        delay_ns: 50,
        if_invoked: vec![key("h").into()],
        if_canceled: vec![],
    });
    let mut actor = PhysicalStageController::new(config(vec![], vec![mapping])).unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    actor
        .handle(keyboard(2, 120, dev, "a", KeyEdge::Up))
        .unwrap();
    drain(&mut actor, 120);
    assert_eq!(actor.pending_bindings(), 1);
    actor
        .handle(keyboard(3, 130, dev, "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(actor.active_bindings(), 1);
    assert_eq!(actor.pending_bindings(), 1);
    assert!(drain(&mut actor, 130).is_empty());
    assert_eq!(actor.pending_bindings(), 0);
    assert_eq!(actor.active_bindings(), 1);
    assert_eq!(actor.next_deadline(), Some(MonoTime(180)));
    assert!(drain(&mut actor, 150).is_empty());
}
#[test]
fn causal_plan_overflow_after_registered_root_latches_failure_and_reset_recovers_without_fake_output()
 {
    let mut actor = PhysicalStageController::new(config(
        vec![mapping(
            "a",
            vec![Action {
                repeat: false,
                hold_down_ns: 100,
                ..key("b").into()
            }],
        )],
        vec![],
    ))
    .unwrap();
    let dev = device(1, 1);
    assert!(
        actor
            .handle(keyboard(1, u64::MAX - 50, dev, "a", KeyEdge::Down))
            .is_err()
    );
    assert!(actor.is_failed());
    assert!(matches!(
        actor.poll(MonoTime(u64::MAX - 40)),
        Err(PhysicalControllerError::Failed)
    ));
    let reset = actor
        .handle(envelope(2, u64::MAX - 40, dev, InputEvent::Reset))
        .unwrap();
    assert!(reset.outputs.is_empty());
    assert!(!actor.is_failed());
    assert_eq!(actor.active_bindings(), 0);
    assert_eq!(actor.pending_bindings(), 0);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor
        .handle(keyboard(3, u64::MAX - 30, dev, "a", KeyEdge::Down))
        .unwrap();
    assert_eq!(
        drain(&mut actor, u64::MAX - 30),
        vec![(key("a"), KeyEdge::Down)]
    );
}
#[test]
fn wheel_and_pointer_cannot_overtake_future_stage_actions_and_rejection_does_not_consume_sequence()
{
    let mut actor = PhysicalStageController::new(config(
        vec![mapping(
            "a",
            vec![
                Action {
                    repeat: false,
                    hold_down_ns: 50,
                    ..key("b").into()
                },
                key("c").into(),
            ],
        )],
        vec![],
    ))
    .unwrap();
    let dev = device(1, 1);
    actor
        .handle(keyboard(1, 100, dev, "a", KeyEdge::Down))
        .unwrap();
    drain(&mut actor, 100);
    for event in [
        InputEvent::Wheel {
            horizontal: 1,
            vertical: 0,
        },
        InputEvent::PointerMove { x: 1, y: 1 },
    ] {
        assert!(matches!(
            actor.handle(envelope(2, 120, dev, event)),
            Err(PhysicalControllerError::UnsupportedPointerScheduling)
        ));
    }
    assert!(!actor.is_failed());
    assert_eq!(actor.active_bindings(), 1);
    assert_eq!(
        edges(
            actor
                .handle(envelope(2, 120, dev, InputEvent::Reset))
                .unwrap()
                .outputs
        ),
        vec![(key("b"), KeyEdge::Up)]
    );
    assert_eq!(
        actor
            .handle(envelope(
                3,
                130,
                dev,
                InputEvent::Wheel {
                    horizontal: 1,
                    vertical: 0
                }
            ))
            .unwrap()
            .capture,
        CaptureDecision::Pass
    );
}
