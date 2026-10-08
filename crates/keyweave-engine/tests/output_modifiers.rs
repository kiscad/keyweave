#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;

fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn action(name: &str, modifiers: &[&str]) -> Action {
    Action {
        conditions: vec![],
        target: ActionTarget::Key(key(name)),
        repeat: true,
        modifiers: modifiers.iter().map(|name| key(name)).collect(),
    }
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
fn config(actions: Vec<Action>) -> ConfigSnapshot {
    ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "output scopes".into(),
        simple: vec![mapping("a", actions)],
        complex: vec![],
    }
}
fn event(seq: u64, device: u64, name: &str, edge: KeyEdge) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(seq),
        device: DeviceInstanceId {
            id: device,
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
    device: u64,
    name: &str,
    edge: KeyEdge,
) -> Vec<(String, KeyEdge)> {
    engine
        .handle(event(seq, device, name, edge))
        .unwrap()
        .outputs
        .into_iter()
        .map(|output| (output.key.name(), output.edge))
        .collect()
}
fn edges(items: &[(&str, KeyEdge)]) -> Vec<(String, KeyEdge)> {
    items
        .iter()
        .map(|(name, edge)| ((*name).into(), *edge))
        .collect()
}

#[test]
fn shortcut_modifiers_are_lazy_up_until_next_down_or_device_neutral() {
    let mut engine = Engine::new(Arc::new(config(vec![action("b", &["left_control"])]))).unwrap();
    assert_eq!(
        send(&mut engine, 1, 1, "a", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Down), ("b", KeyEdge::Down)])
    );
    assert_eq!(engine.state().output_owners, 1);
    assert_eq!(
        send(&mut engine, 2, 1, "c", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Up), ("c", KeyEdge::Down)])
    );
    send(&mut engine, 3, 1, "c", KeyEdge::Up);
    assert_eq!(
        send(&mut engine, 4, 1, "a", KeyEdge::Up),
        edges(&[("b", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
    send(&mut engine, 5, 1, "a", KeyEdge::Down);
    assert_eq!(
        send(&mut engine, 6, 1, "a", KeyEdge::Up),
        edges(&[("b", KeyEdge::Up), ("left_control", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn shortcut_scope_does_not_release_another_devices_physical_owner() {
    let mut engine = Engine::new(Arc::new(config(vec![action("b", &["left_control"])]))).unwrap();
    send(&mut engine, 1, 2, "left_control", KeyEdge::Down);
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Down),
        edges(&[("b", KeyEdge::Down)])
    );
    assert_eq!(
        send(&mut engine, 3, 1, "a", KeyEdge::Up),
        edges(&[("b", KeyEdge::Up)])
    );
    assert_eq!(engine.state().output_owners, 1);
    assert_eq!(
        send(&mut engine, 4, 2, "left_control", KeyEdge::Up),
        edges(&[("left_control", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn final_modifier_action_retains_scope_even_when_main_key_does_not_repeat() {
    for repeat in [true, false] {
        let mut output = action("left_shift", &["left_control"]);
        output.repeat = repeat;
        let mut engine = Engine::new(Arc::new(config(vec![output]))).unwrap();
        let mut expected = edges(&[
            ("left_control", KeyEdge::Down),
            ("left_shift", KeyEdge::Down),
        ]);
        if !repeat {
            expected.push(("left_shift".into(), KeyEdge::Up));
        }
        assert_eq!(send(&mut engine, 1, 1, "a", KeyEdge::Down), expected);
        let mut expected = Vec::new();
        if repeat {
            expected.push(("left_shift".into(), KeyEdge::Up));
        }
        expected.push(("left_control".into(), KeyEdge::Up));
        assert_eq!(send(&mut engine, 2, 1, "a", KeyEdge::Up), expected);
        assert_eq!(engine.state(), EngineState::default());
    }
}

#[test]
fn each_sequence_action_has_its_own_modifier_scope() {
    let mut engine = Engine::new(Arc::new(config(vec![
        action("b", &["left_control"]),
        action("c", &["left_shift"]),
    ])))
    .unwrap();
    assert_eq!(
        send(&mut engine, 1, 1, "a", KeyEdge::Down),
        edges(&[
            ("left_control", KeyEdge::Down),
            ("b", KeyEdge::Down),
            ("b", KeyEdge::Up),
            ("left_control", KeyEdge::Up),
            ("left_shift", KeyEdge::Down),
            ("c", KeyEdge::Down)
        ])
    );
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Up),
        edges(&[("c", KeyEdge::Up), ("left_shift", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn lazy_modifier_input_can_be_remapped_in_next_stage() {
    let mut config = config(vec![action("b", &["left_control"])]);
    config
        .complex
        .push(mapping("left_control", vec![action("left_shift", &[])]));
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        send(&mut engine, 1, 1, "a", KeyEdge::Down),
        edges(&[
            ("left_shift", KeyEdge::Down),
            ("b", KeyEdge::Down),
            ("left_shift", KeyEdge::Up)
        ])
    );
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Up),
        edges(&[("b", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn next_stage_can_consume_modifiers_from_the_preceding_action() {
    let mut config = config(vec![action("b", &["left_control"])]);
    let mut b = mapping("b", vec![action("c", &[])]);
    b.allow_extra_modifiers = false;
    b.extras.from_modifiers.mandatory.insert(Modifier::Control);
    config.complex.push(b);
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        send(&mut engine, 1, 1, "a", KeyEdge::Down),
        edges(&[("c", KeyEdge::Down)])
    );
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Up),
        edges(&[("c", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn extra_action_scopes_finish_at_physical_up_using_old_snapshot() {
    let mut config = config(vec![action("b", &[])]);
    config.simple[0].extras.after_key_up = vec![action("c", &["left_control"])];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    send(&mut engine, 1, 1, "a", KeyEdge::Down);
    engine
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Up),
        edges(&[
            ("b", KeyEdge::Up),
            ("left_control", KeyEdge::Down),
            ("c", KeyEdge::Down),
            ("c", KeyEdge::Up),
            ("left_control", KeyEdge::Up)
        ])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn unplug_releases_latent_scope_without_running_extra_action() {
    let mut config = config(vec![action("b", &["left_control"])]);
    config.simple[0].extras.after_key_up = vec![action("z", &[])];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    send(&mut engine, 1, 1, "a", KeyEdge::Down);
    let mut removed = event(2, 1, "a", KeyEdge::Up);
    removed.event = InputEvent::DeviceRemoved;
    let outputs = engine.handle(removed).unwrap().outputs;
    assert_eq!(
        outputs
            .iter()
            .map(|output| (output.key.name(), output.edge))
            .collect::<Vec<_>>(),
        edges(&[("b", KeyEdge::Up), ("left_control", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn unknown_up_does_not_flush_a_scope_or_infer_neutral() {
    let mut engine = Engine::new(Arc::new(config(vec![action("b", &["left_control"])]))).unwrap();
    send(&mut engine, 1, 1, "a", KeyEdge::Down);
    assert!(send(&mut engine, 2, 1, "z", KeyEdge::Up).is_empty());
    assert!(
        engine
            .state()
            .pressed_outputs
            .contains(&key("left_control"))
    );
    engine.reset(MonoTime(3)).unwrap();
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn modifier_expansion_is_counted_before_mutating_ownership() {
    let config = config(vec![action("b", &["left_control", "left_shift"]); 256]);
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    assert_eq!(
        engine.handle(event(1, 1, "a", KeyEdge::Down)).unwrap_err(),
        EngineError::ResourceLimit
    );
    assert_eq!(engine.state(), EngineState::default());
}
