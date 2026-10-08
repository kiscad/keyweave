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
            to: vec![key(output)],
            allow_extra_modifiers: true,
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
        let mut engine = Engine::new(snapshot(1, "b"));
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
            to: vec![key("c")],
            allow_extra_modifiers: false,
        },
        Mapping {
            id: "second".into(),
            from: key("b"),
            to: vec![key("d")],
            allow_extra_modifiers: false,
        },
        Mapping {
            id: "cycle".into(),
            from: key("c"),
            to: vec![key("a")],
            allow_extra_modifiers: false,
        },
    ];
    let mut engine = Engine::new(Arc::new(config));
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
        to: vec![key("b")],
        allow_extra_modifiers: false,
    }];
    let mut engine = Engine::new(Arc::new(config));
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
    let mut engine = Engine::new(snapshot(1, "b"));
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
