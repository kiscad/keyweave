#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
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
        extras: MappingExtras::default(),
    }
}
fn config(simple: Vec<Mapping>, complex: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "repeat".into(),
        simple,
        complex,
    })
}
fn input(engine: &mut Engine, seq: u64, at: u64, d: DeviceInstanceId, name: &str, edge: KeyEdge) {
    engine
        .handle(InputEnvelope {
            seq,
            at_ns: MonoTime(at),
            device: d,
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: key(name),
                edge,
            },
        })
        .unwrap();
}
#[test]
fn repeats_are_source_specific_and_the_query_does_not_change_ownership() {
    let mut engine = Engine::new(config(vec![], vec![])).unwrap();
    let d = device(1, 1);
    input(&mut engine, 1, 1, d, "a", KeyEdge::Down);
    input(&mut engine, 2, 2, d, "b", KeyEdge::Down);
    assert_eq!(engine.physical_repeat_target(d, key("a")), Some(key("a")));
    for _ in 0..100 {
        assert_eq!(engine.physical_repeat_target(d, key("b")), Some(key("b")));
    }
    assert_eq!(engine.state().bindings, 2);
    input(&mut engine, 3, 3, d, "a", KeyEdge::Up);
    assert_eq!(engine.physical_repeat_target(d, key("a")), None);
    assert_eq!(engine.physical_repeat_target(d, key("b")), Some(key("b")));
}
#[test]
fn only_retained_main_action_survives_expansion_and_hot_reload() {
    let mut engine = Engine::new(config(
        vec![mapping("a", vec![key("b").into()])],
        vec![mapping("b", vec![key("c").into(), key("d").into()])],
    ))
    .unwrap();
    let d = device(1, 1);
    input(&mut engine, 1, 1, d, "a", KeyEdge::Down);
    let mut next = (*config(vec![mapping("a", vec![key("z").into()])], vec![])).clone();
    next.generation = ConfigGeneration(2);
    engine.activate(Arc::new(next)).unwrap();
    assert_eq!(engine.physical_repeat_target(d, key("a")), Some(key("d")));
    let repeat = engine
        .handle(InputEnvelope {
            seq: 2,
            at_ns: MonoTime(2),
            device: d,
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: key("a"),
                edge: KeyEdge::Repeat,
            },
        })
        .unwrap();
    assert!(repeat.outputs.is_empty());
    assert_eq!(engine.state().bindings, 1);
    input(&mut engine, 3, 3, d, "a", KeyEdge::Up);
    input(&mut engine, 4, 4, d, "a", KeyEdge::Down);
    assert_eq!(engine.physical_repeat_target(d, key("a")), Some(key("z")));
}
#[test]
fn tap_suppression_and_modifiers_never_fall_back_to_scope_bindings() {
    for repeat in [false, true] {
        let mut action: Action = key("left_shift").into();
        action.repeat = repeat;
        action.modifiers = vec![key("left_alt")];
        let mut engine = Engine::new(config(
            vec![mapping("a", vec![action])],
            vec![mapping("left_alt", vec![key("c").into()])],
        ))
        .unwrap();
        let d = device(1, 1);
        input(&mut engine, 1, 1, d, "a", KeyEdge::Down);
        assert_eq!(engine.physical_repeat_target(d, key("a")), None);
    }
    let mut tap: Action = key("b").into();
    tap.repeat = false;
    for actions in [vec![tap], vec![]] {
        let mut engine = Engine::new(config(vec![mapping("a", actions)], vec![])).unwrap();
        input(&mut engine, 1, 1, device(1, 1), "a", KeyEdge::Down);
        assert_eq!(engine.physical_repeat_target(device(1, 1), key("a")), None);
    }
}
#[test]
fn held_deadline_replaces_repeat_target_before_source_repeat() {
    let mut map = mapping("a", vec![key("b").into()]);
    map.extras.if_held_down = vec![key("c").into()];
    map.extras.held_threshold_ns = 10;
    let mut engine = Engine::new(config(vec![], vec![map])).unwrap();
    let d = device(1, 1);
    input(&mut engine, 1, 0, d, "a", KeyEdge::Down);
    assert_eq!(engine.physical_repeat_target(d, key("a")), Some(key("b")));
    let (at, token) = engine.next_deadline().unwrap();
    engine
        .handle(InputEnvelope {
            seq: 2,
            at_ns: at,
            device: d,
            provenance: Provenance::Physical,
            event: InputEvent::TimerFired { token },
        })
        .unwrap();
    assert_eq!(engine.physical_repeat_target(d, key("a")), Some(key("c")));
    engine.reset(at).unwrap();
    assert_eq!(engine.physical_repeat_target(d, key("a")), None);
}
#[test]
fn shared_output_does_not_make_released_or_stale_sources_repeatable() {
    let mut engine = Engine::new(config(
        vec![
            mapping("a", vec![key("b").into()]),
            mapping("c", vec![key("b").into()]),
        ],
        vec![],
    ))
    .unwrap();
    let d = device(1, 1);
    let other = device(2, 1);
    input(&mut engine, 1, 1, d, "a", KeyEdge::Down);
    input(&mut engine, 2, 2, other, "c", KeyEdge::Down);
    assert_eq!(engine.physical_repeat_target(d, key("a")), Some(key("b")));
    input(&mut engine, 3, 3, d, "a", KeyEdge::Up);
    assert_eq!(engine.physical_repeat_target(d, key("a")), None);
    assert_eq!(engine.physical_repeat_target(device(2, 2), key("c")), None);
    assert_eq!(
        engine.physical_repeat_target(other, key("c")),
        Some(key("b"))
    );
    engine
        .handle(InputEnvelope {
            seq: 4,
            at_ns: MonoTime(4),
            device: other,
            provenance: Provenance::Physical,
            event: InputEvent::DeviceRemoved,
        })
        .unwrap();
    assert_eq!(engine.physical_repeat_target(other, key("c")), None);
}
