#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;

fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn snapshot(generation: u64, output: &str) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "test".into(),
        simple: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key(output).into()],
            allow_extra_modifiers: true,
            extras: MappingExtras::default(),
        }],
        complex: vec![],
    })
}
fn event(seq: u64, id: u64, generation: u64, name: &str, edge: KeyEdge) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(seq),
        device: DeviceInstanceId { id, generation },
        provenance: Provenance::Physical,
        event: InputEvent::Key {
            key: key(name),
            edge,
        },
    }
}
fn edges(outputs: &[OutputIntent]) -> Vec<(String, KeyEdge)> {
    outputs.iter().map(|o| (o.key.name(), o.edge)).collect()
}

#[test]
fn held_key_finishes_the_old_snapshot_after_reload() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(1, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Down)]
    );
    engine.activate(snapshot(2, "c")).unwrap();
    assert_eq!(
        engine.state().binding_generations,
        vec![ConfigGeneration(1)]
    );
    assert_eq!(
        edges(
            &engine
                .handle(event(2, 1, 1, "a", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Up)]
    );
    assert_eq!(
        edges(
            &engine
                .handle(event(3, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![("c".into(), KeyEdge::Down)]
    );
}
#[test]
fn same_output_has_multiple_device_owners_and_unplug_is_scoped() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    assert_eq!(
        engine
            .handle(event(1, 1, 1, "a", KeyEdge::Down))
            .unwrap()
            .outputs
            .len(),
        1
    );
    assert!(
        engine
            .handle(event(2, 2, 1, "a", KeyEdge::Down))
            .unwrap()
            .outputs
            .is_empty()
    );
    let mut removed = event(3, 1, 1, "a", KeyEdge::Up);
    removed.event = InputEvent::DeviceRemoved;
    assert!(engine.handle(removed).unwrap().outputs.is_empty());
    assert_eq!(engine.state().output_owners, 1);
    assert_eq!(
        edges(
            &engine
                .handle(event(4, 2, 1, "a", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state(), EngineState::default());
}
#[test]
fn duplicate_down_repeat_and_unknown_up_do_not_change_ownership() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    assert!(
        engine
            .handle(event(1, 1, 1, "a", KeyEdge::Up))
            .unwrap()
            .outputs
            .is_empty()
    );
    engine.handle(event(2, 1, 1, "a", KeyEdge::Down)).unwrap();
    assert!(
        engine
            .handle(event(3, 1, 1, "a", KeyEdge::Down))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert!(
        engine
            .handle(event(4, 1, 1, "a", KeyEdge::Repeat))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(engine.state().output_owners, 1);
    engine.handle(event(5, 1, 1, "a", KeyEdge::Up)).unwrap();
    assert_eq!(engine.state(), EngineState::default());
}
#[test]
fn replug_releases_old_generation_and_rejects_stale_up() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    engine.handle(event(1, 1, 1, "a", KeyEdge::Down)).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(2, 1, 2, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Up), ("b".into(), KeyEdge::Down)]
    );
    assert_eq!(
        engine.handle(event(3, 1, 1, "a", KeyEdge::Up)).unwrap_err(),
        EngineError::RetiredDevice
    );
    assert_eq!(engine.state().output_owners, 1);
    engine.handle(event(4, 1, 2, "a", KeyEdge::Up)).unwrap();
}
#[test]
fn own_and_other_injections_bypass_without_matching() {
    for provenance in [Provenance::OurVirtual, Provenance::OtherInjected] {
        let mut engine = Engine::new(snapshot(1, "b")).unwrap();
        let mut input = event(1, 1, 1, "a", KeyEdge::Down);
        input.provenance = provenance;
        let step = engine.handle(input).unwrap();
        assert_eq!(step.capture, CaptureDecision::Pass);
        assert!(step.outputs.is_empty());
        assert_eq!(engine.state(), EngineState::default());
    }
}
#[test]
fn one_complex_match_per_stage_and_no_recirculation() {
    let mut config = (*snapshot(1, "b")).clone();
    config.complex = vec![
        Mapping {
            id: "first".into(),
            from: key("b"),
            to: vec![key("c").into()],
            allow_extra_modifiers: false,
            extras: MappingExtras::default(),
        },
        Mapping {
            id: "second".into(),
            from: key("b"),
            to: vec![key("d").into()],
            allow_extra_modifiers: false,
            extras: MappingExtras::default(),
        },
        Mapping {
            id: "cycle".into(),
            from: key("c"),
            to: vec![key("a").into()],
            allow_extra_modifiers: false,
            extras: MappingExtras::default(),
        },
    ];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(1, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![("c".into(), KeyEdge::Down)]
    );
}
#[test]
fn bare_complex_from_rejects_extra_modifiers() {
    let mut config = ConfigSnapshot::empty(ConfigGeneration(1));
    config.complex = vec![Mapping {
        id: "a".into(),
        from: key("a"),
        to: vec![key("b").into()],
        allow_extra_modifiers: false,
        extras: MappingExtras::default(),
    }];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    engine
        .handle(event(1, 1, 1, "left_shift", KeyEdge::Down))
        .unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(2, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![("a".into(), KeyEdge::Down)]
    );
}
#[test]
fn reset_releases_keys_before_modifiers_and_is_idempotent() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    engine
        .handle(event(1, 1, 1, "left_control", KeyEdge::Down))
        .unwrap();
    engine.handle(event(2, 1, 1, "a", KeyEdge::Down)).unwrap();
    assert_eq!(
        edges(&engine.reset(MonoTime(3)).unwrap()),
        vec![
            ("b".into(), KeyEdge::Up),
            ("left_control".into(), KeyEdge::Up)
        ]
    );
    assert!(engine.reset(MonoTime(4)).unwrap().is_empty());
    assert_eq!(engine.state(), EngineState::default());
}
#[test]
fn ordering_and_generation_errors_are_explicit() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    engine.handle(event(2, 1, 1, "a", KeyEdge::Down)).unwrap();
    assert_eq!(
        engine.handle(event(1, 1, 1, "a", KeyEdge::Up)).unwrap_err(),
        EngineError::TimeBackwards
    );
    assert_eq!(
        engine.handle(event(2, 1, 1, "a", KeyEdge::Up)).unwrap_err(),
        EngineError::SequenceBackwards
    );
    assert_eq!(
        engine.activate(snapshot(1, "c")).unwrap_err(),
        EngineError::InvalidGeneration
    );
    assert_eq!(engine.state().output_owners, 1);
}
#[test]
fn bounded_source_capacity_fails_without_losing_existing_up() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    for i in 1..=MAX_BINDINGS as u64 {
        engine.handle(event(i, i, 1, "a", KeyEdge::Down)).unwrap();
    }
    let next = MAX_BINDINGS as u64 + 1;
    assert_eq!(
        engine
            .handle(event(next, next, 1, "a", KeyEdge::Down))
            .unwrap_err(),
        EngineError::ResourceLimit
    );
    let released = engine.reset(MonoTime(next + 1)).unwrap();
    assert_eq!(released.len(), 1);
    assert_eq!(engine.state(), EngineState::default());
}
#[test]
fn deterministic_interleavings_finish_balanced() {
    let mut seed = 73_u64;
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    for seq in 1..=10_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let edge = if seed & 0x100 == 0 {
            KeyEdge::Down
        } else {
            KeyEdge::Up
        };
        engine
            .handle(event(
                seq,
                seed % 8,
                1,
                if seed & 1 == 0 { "a" } else { "left_shift" },
                edge,
            ))
            .unwrap();
        assert!(engine.state().bindings <= 16);
    }
    engine.reset(MonoTime(10_001)).unwrap();
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn intermediate_sequences_finish_before_the_next_phase_input() {
    let mut config = (*snapshot(1, "b")).clone();
    config.simple[0].to = vec![key("b").into(), key("c").into()];
    config.complex.push(Mapping {
        id: "nested".into(),
        from: key("b"),
        to: vec![key("d").into(), key("e").into()],
        allow_extra_modifiers: false,
        extras: MappingExtras::default(),
    });
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(1, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![
            ("d".into(), KeyEdge::Down),
            ("d".into(), KeyEdge::Up),
            ("e".into(), KeyEdge::Down),
            ("e".into(), KeyEdge::Up),
            ("c".into(), KeyEdge::Down),
        ]
    );
    assert_eq!(engine.state().pressed_outputs, vec![key("c")]);
    engine.activate(snapshot(2, "z")).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(2, 1, 1, "a", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("c".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn repeated_sequence_key_has_separate_lifetimes_and_respects_existing_owners() {
    let mut config = (*snapshot(1, "b")).clone();
    config.simple[0].to = vec![key("b").into(), key("b").into()];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(1, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![
            ("b".into(), KeyEdge::Down),
            ("b".into(), KeyEdge::Up),
            ("b".into(), KeyEdge::Down)
        ]
    );
    assert!(
        engine
            .handle(event(2, 2, 1, "a", KeyEdge::Down))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(engine.state().output_owners, 2);
    assert!(
        engine
            .handle(event(3, 1, 1, "a", KeyEdge::Up))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(
        edges(
            &engine
                .handle(event(4, 2, 1, "a", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Up)]
    );
}

#[test]
fn sequence_does_not_repeat_and_vk_none_does_not_create_an_owner() {
    let mut config = (*snapshot(1, "b")).clone();
    config.simple[0].to = vec![key("b").into(), key("vk_none").into()];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        engine
            .handle(event(1, 1, 1, "a", KeyEdge::Down))
            .unwrap()
            .outputs
            .len(),
        2
    );
    assert_eq!(engine.state().output_owners, 0);
    assert!(
        engine
            .handle(event(2, 1, 1, "a", KeyEdge::Repeat))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert!(
        engine
            .handle(event(3, 1, 1, "a", KeyEdge::Up))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn expansion_limit_is_checked_before_any_output_changes() {
    let mut config = (*snapshot(1, "b")).clone();
    config.simple[0].to = vec![key("b").into(); 256];
    config.complex.push(Mapping {
        id: "too-large".into(),
        from: key("b"),
        to: vec![key("c").into(); 256],
        allow_extra_modifiers: false,
        extras: MappingExtras::default(),
    });
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    engine
        .handle(event(1, 1, 1, "left_control", KeyEdge::Down))
        .unwrap();
    let previous = engine.state();
    assert_eq!(
        engine
            .handle(event(2, 1, 1, "a", KeyEdge::Down))
            .unwrap_err(),
        EngineError::ResourceLimit
    );
    assert_eq!(engine.state(), previous);
    assert_eq!(
        edges(
            &engine
                .handle(event(3, 1, 1, "left_control", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("left_control".into(), KeyEdge::Up)]
    );
}

#[test]
fn stale_remove_cannot_release_the_new_connection() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    engine.handle(event(1, 1, 1, "a", KeyEdge::Down)).unwrap();
    engine.handle(event(2, 1, 2, "a", KeyEdge::Down)).unwrap();
    let before = engine.state();
    let mut removed = event(3, 1, 1, "a", KeyEdge::Up);
    removed.event = InputEvent::DeviceRemoved;
    assert!(engine.handle(removed).unwrap().outputs.is_empty());
    assert_eq!(engine.state(), before);
    assert_eq!(
        edges(
            &engine
                .handle(event(4, 1, 2, "a", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Up)]
    );
}

#[test]
fn newer_remove_retires_old_binding_even_without_a_down_from_new_connection() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    engine.handle(event(1, 1, 1, "a", KeyEdge::Down)).unwrap();
    let mut removed = event(2, 1, 2, "a", KeyEdge::Up);
    removed.event = InputEvent::DeviceRemoved;
    assert_eq!(
        edges(&engine.handle(removed.clone()).unwrap().outputs),
        vec![("b".into(), KeyEdge::Up)]
    );
    removed.seq = 3;
    removed.at_ns = MonoTime(3);
    assert!(engine.handle(removed).unwrap().outputs.is_empty());
    assert_eq!(
        engine
            .handle(event(4, 1, 1, "a", KeyEdge::Down))
            .unwrap_err(),
        EngineError::RetiredDevice
    );
    assert_eq!(
        engine
            .handle(event(5, 1, 2, "a", KeyEdge::Down))
            .unwrap_err(),
        EngineError::RetiredDevice
    );
    assert_eq!(engine.state(), EngineState::default());
    assert_eq!(
        engine
            .handle(event(6, 1, 3, "a", KeyEdge::Down))
            .unwrap()
            .outputs
            .len(),
        1
    );
}

#[test]
fn repeated_reconnects_store_only_latest_generation() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    for generation in 1..=10_000 {
        let seq = generation * 2;
        engine
            .handle(event(seq, 1, generation, "a", KeyEdge::Down))
            .unwrap();
        let mut removed = event(seq + 1, 1, generation, "a", KeyEdge::Up);
        removed.event = InputEvent::DeviceRemoved;
        assert_eq!(engine.handle(removed).unwrap().outputs.len(), 1);
        assert_eq!(engine.tracked_devices(), 1);
        assert_eq!(engine.state(), EngineState::default());
    }
}

#[test]
fn stale_wheel_and_move_are_rejected_after_reconnection() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    engine.handle(event(1, 1, 2, "a", KeyEdge::Down)).unwrap();
    for (index, activity) in [
        InputEvent::Wheel {
            horizontal: 0,
            vertical: 1,
        },
        InputEvent::PointerMove { x: 1, y: 1 },
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = event(index as u64 + 2, 1, 1, "a", KeyEdge::Down);
        input.event = activity;
        assert_eq!(
            engine.handle(input).unwrap_err(),
            EngineError::RetiredDevice
        );
        assert_eq!(engine.state().output_owners, 1);
    }
}

#[test]
fn identity_history_has_a_hard_limit_and_reset_keeps_retired_protection() {
    let mut engine = Engine::new(snapshot(1, "b")).unwrap();
    for id in 1..=MAX_DEVICE_IDENTITIES as u64 {
        let mut input = event(id, id, 1, "a", KeyEdge::Down);
        input.event = InputEvent::Wheel {
            horizontal: 0,
            vertical: 0,
        };
        assert!(engine.handle(input).unwrap().outputs.is_empty());
    }
    let seq = MAX_DEVICE_IDENTITIES as u64 + 1;
    let mut input = event(seq, seq, 1, "a", KeyEdge::Down);
    input.event = InputEvent::PointerMove { x: 1, y: 0 };
    assert_eq!(
        engine.handle(input).unwrap_err(),
        EngineError::DeviceHistoryFull
    );
    assert_eq!(engine.tracked_devices(), MAX_DEVICE_IDENTITIES);
    assert!(engine.reset(MonoTime(seq + 1)).unwrap().is_empty());
    assert_eq!(engine.tracked_devices(), MAX_DEVICE_IDENTITIES);
    let mut removed = event(seq + 2, 1, 1, "a", KeyEdge::Up);
    removed.event = InputEvent::DeviceRemoved;
    engine.handle(removed).unwrap();
    engine.reset(MonoTime(seq + 3)).unwrap();
    assert_eq!(
        engine
            .handle(event(seq + 4, 1, 1, "a", KeyEdge::Down))
            .unwrap_err(),
        EngineError::RetiredDevice
    );
}

#[test]
fn repeat_false_taps_last_action_and_never_replays_on_physical_repeat() {
    let mut config = (*snapshot(1, "b")).clone();
    config.simple[0].to[0].repeat = false;
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(1, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Down), ("b".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state().output_owners, 0);
    for (seq, edge) in [(2, KeyEdge::Repeat), (3, KeyEdge::Down), (4, KeyEdge::Up)] {
        assert!(
            engine
                .handle(event(seq, 1, 1, "a", edge))
                .unwrap()
                .outputs
                .is_empty()
        );
    }
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn repeat_false_in_next_phase_keeps_its_up_macro_bound_to_stage_input() {
    let mut config = (*snapshot(1, "b")).clone();
    config.complex.push(Mapping {
        id: "b".into(),
        from: key("b"),
        to: vec![Action {
            conditions: vec![],
            target: ActionTarget::Key(key("c")),
            repeat: false,
            lazy: false,
            halt: false,
            hold_down_ns: 0,
            modifiers: vec![],
        }],
        allow_extra_modifiers: false,
        extras: MappingExtras {
            after_key_up: vec![key("d").into()],
            ..MappingExtras::default()
        },
    });
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        edges(
            &engine
                .handle(event(1, 1, 1, "a", KeyEdge::Down))
                .unwrap()
                .outputs
        ),
        vec![("c".into(), KeyEdge::Down), ("c".into(), KeyEdge::Up)]
    );
    assert_eq!(
        edges(
            &engine
                .handle(event(2, 1, 1, "a", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("d".into(), KeyEdge::Down), ("d".into(), KeyEdge::Up)]
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn repeat_false_tap_does_not_release_another_device_owner() {
    let mut config = (*snapshot(1, "b")).clone();
    config.simple[0].to[0].repeat = false;
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    engine.handle(event(1, 2, 1, "b", KeyEdge::Down)).unwrap();
    assert!(
        engine
            .handle(event(2, 1, 1, "a", KeyEdge::Down))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(engine.state().output_owners, 1);
    assert!(
        engine
            .handle(event(3, 1, 1, "a", KeyEdge::Up))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(
        edges(
            &engine
                .handle(event(4, 2, 1, "b", KeyEdge::Up))
                .unwrap()
                .outputs
        ),
        vec![("b".into(), KeyEdge::Up)]
    );
}
