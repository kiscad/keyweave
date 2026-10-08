#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;

fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn config(generation: u64, alone: &str, after: &str) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "tap".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "caps".into(),
            from: key("caps_lock"),
            to: vec![key("left_control").into()],
            allow_extra_modifiers: true,
            extras: MappingExtras {
                from_modifiers: ModifierFilter::default(),
                conditions: vec![],
                if_alone: vec![key(alone).into()],
                after_key_up: vec![key(after).into()],
                alone_timeout_ns: 100_000_000,
            },
        }],
    })
}
fn input(seq: u64, time: u64, name: &str, edge: KeyEdge) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(time),
        device: DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        provenance: Provenance::Physical,
        event: InputEvent::Key {
            key: key(name),
            edge,
        },
    }
}
fn send(
    engine: &mut Engine,
    seq: u64,
    time: u64,
    name: &str,
    edge: KeyEdge,
) -> Vec<(String, KeyEdge)> {
    engine
        .handle(input(seq, time, name, edge))
        .unwrap()
        .outputs
        .into_iter()
        .map(|out| (out.key.name(), out.edge))
        .collect()
}
fn pairs(items: &[(&str, KeyEdge)]) -> Vec<(String, KeyEdge)> {
    items
        .iter()
        .map(|(name, edge)| ((*name).into(), *edge))
        .collect()
}

#[test]
fn strict_timeout_boundary_and_order_of_up_alone_after_actions() {
    for (duration, alone) in [
        (99_999_999, true),
        (100_000_000, false),
        (100_000_001, false),
    ] {
        let mut engine = Engine::new(config(1, "escape", "tab")).unwrap();
        assert_eq!(
            send(&mut engine, 1, 0, "caps_lock", KeyEdge::Down),
            pairs(&[("left_control", KeyEdge::Down)])
        );
        let mut expected = pairs(&[("left_control", KeyEdge::Up)]);
        if alone {
            expected.extend(pairs(&[("escape", KeyEdge::Down), ("escape", KeyEdge::Up)]));
        }
        expected.extend(pairs(&[("tab", KeyEdge::Down), ("tab", KeyEdge::Up)]));
        assert_eq!(
            send(&mut engine, 2, duration, "caps_lock", KeyEdge::Up),
            expected
        );
        assert_eq!(engine.state(), EngineState::default());
    }
}

#[test]
fn old_snapshot_is_used_for_alone_and_after_actions_after_reload() {
    let mut engine = Engine::new(config(1, "escape", "tab")).unwrap();
    send(&mut engine, 1, 0, "caps_lock", KeyEdge::Down);
    engine
        .activate(config(2, "return_or_enter", "spacebar"))
        .unwrap();
    assert_eq!(
        send(&mut engine, 2, 1, "caps_lock", KeyEdge::Up),
        pairs(&[
            ("left_control", KeyEdge::Up),
            ("escape", KeyEdge::Down),
            ("escape", KeyEdge::Up),
            ("tab", KeyEdge::Down),
            ("tab", KeyEdge::Up)
        ])
    );
    send(&mut engine, 3, 2, "caps_lock", KeyEdge::Down);
    assert_eq!(
        send(&mut engine, 4, 3, "caps_lock", KeyEdge::Up),
        pairs(&[
            ("left_control", KeyEdge::Up),
            ("return_or_enter", KeyEdge::Down),
            ("return_or_enter", KeyEdge::Up),
            ("spacebar", KeyEdge::Down),
            ("spacebar", KeyEdge::Up)
        ])
    );
}

#[test]
fn any_other_physical_key_down_including_button_cancels_alone() {
    for name in ["a", "left_shift", "button1"] {
        let mut engine = Engine::new(config(1, "escape", "tab")).unwrap();
        send(&mut engine, 1, 0, "caps_lock", KeyEdge::Down);
        send(&mut engine, 2, 1, name, KeyEdge::Down);
        send(&mut engine, 3, 2, name, KeyEdge::Up);
        assert_eq!(
            send(&mut engine, 4, 3, "caps_lock", KeyEdge::Up),
            pairs(&[
                ("left_control", KeyEdge::Up),
                ("tab", KeyEdge::Down),
                ("tab", KeyEdge::Up)
            ])
        );
        assert_eq!(engine.state(), EngineState::default());
    }
}

#[test]
fn wheel_cancels_alone_but_move_and_zero_wheel_do_not() {
    for (activity, canceled) in [
        (
            InputEvent::Wheel {
                horizontal: 1,
                vertical: 0,
            },
            true,
        ),
        (
            InputEvent::Wheel {
                horizontal: 0,
                vertical: -1,
            },
            true,
        ),
        (
            InputEvent::Wheel {
                horizontal: 0,
                vertical: 0,
            },
            false,
        ),
        (InputEvent::PointerMove { x: 100, y: -42 }, false),
    ] {
        let mut engine = Engine::new(config(1, "escape", "tab")).unwrap();
        send(&mut engine, 1, 0, "caps_lock", KeyEdge::Down);
        let mut event = input(2, 1, "a", KeyEdge::Down);
        event.event = activity;
        let result = engine.handle(event).unwrap();
        assert_eq!(result.capture, CaptureDecision::Pass);
        assert!(result.outputs.is_empty());
        let outputs = send(&mut engine, 3, 2, "caps_lock", KeyEdge::Up);
        assert_eq!(outputs.iter().any(|(name, _)| name == "escape"), !canceled);
        assert_eq!(engine.state(), EngineState::default());
    }
}

#[test]
fn repeat_duplicate_and_other_injected_activity_do_not_cancel_alone() {
    let mut engine = Engine::new(config(1, "escape", "tab")).unwrap();
    send(&mut engine, 1, 0, "caps_lock", KeyEdge::Down);
    assert!(send(&mut engine, 2, 1, "caps_lock", KeyEdge::Down).is_empty());
    assert!(send(&mut engine, 3, 2, "caps_lock", KeyEdge::Repeat).is_empty());
    let mut injected = input(4, 3, "a", KeyEdge::Down);
    injected.provenance = Provenance::OtherInjected;
    assert_eq!(
        engine.handle(injected).unwrap().capture,
        CaptureDecision::Pass
    );
    assert!(
        send(&mut engine, 5, 4, "caps_lock", KeyEdge::Up)
            .iter()
            .any(|(name, _)| name == "escape")
    );
}

#[test]
fn unplug_and_reset_release_without_running_alone_or_after_macros() {
    for removal in [InputEvent::DeviceRemoved, InputEvent::Reset] {
        let mut engine = Engine::new(config(1, "escape", "tab")).unwrap();
        send(&mut engine, 1, 0, "caps_lock", KeyEdge::Down);
        let mut event = input(2, 1, "caps_lock", KeyEdge::Up);
        event.event = removal;
        let result = engine.handle(event).unwrap();
        assert_eq!(result.outputs.len(), 1);
        assert_eq!(result.outputs[0].key, key("left_control"));
        assert_eq!(result.outputs[0].edge, KeyEdge::Up);
        assert_eq!(engine.state(), EngineState::default());
    }
}

#[test]
fn alone_restores_modifiers_from_down_even_when_physical_modifier_was_released() {
    let mut engine = Engine::new(config(1, "escape", "vk_none")).unwrap();
    send(&mut engine, 1, 0, "left_shift", KeyEdge::Down);
    send(&mut engine, 2, 1, "caps_lock", KeyEdge::Down);
    send(&mut engine, 3, 2, "left_shift", KeyEdge::Up);
    assert_eq!(
        send(&mut engine, 4, 3, "caps_lock", KeyEdge::Up),
        pairs(&[
            ("left_control", KeyEdge::Up),
            ("left_shift", KeyEdge::Down),
            ("escape", KeyEdge::Down),
            ("escape", KeyEdge::Up),
            ("left_shift", KeyEdge::Up)
        ])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn nested_phase_taps_complete_their_up_macros_before_next_action() {
    let mut configuration = (*config(1, "escape", "tab")).clone();
    configuration.simple.push(Mapping {
        id: "simple".into(),
        from: key("a"),
        to: vec![key("caps_lock").into(), key("b").into()],
        allow_extra_modifiers: true,
        extras: MappingExtras::default(),
    });
    let mut engine = Engine::new(Arc::new(configuration)).unwrap();
    assert_eq!(
        send(&mut engine, 1, 0, "a", KeyEdge::Down),
        pairs(&[
            ("left_control", KeyEdge::Down),
            ("left_control", KeyEdge::Up),
            ("escape", KeyEdge::Down),
            ("escape", KeyEdge::Up),
            ("tab", KeyEdge::Down),
            ("tab", KeyEdge::Up),
            ("b", KeyEdge::Down)
        ])
    );
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Up),
        pairs(&[("b", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn resource_reservation_includes_future_macros_and_their_modifier_scopes() {
    let mut configuration = (*config(1, "escape", "tab")).clone();
    configuration.complex[0].extras.if_alone = vec![key("escape").into(); 256];
    configuration.complex[0].extras.after_key_up = vec![key("tab").into(); 256];
    let mut engine = Engine::new(Arc::new(configuration)).unwrap();
    assert_eq!(
        engine
            .handle(input(1, 0, "caps_lock", KeyEdge::Down))
            .unwrap_err(),
        EngineError::ResourceLimit
    );
    assert_eq!(engine.state(), EngineState::default());
}
