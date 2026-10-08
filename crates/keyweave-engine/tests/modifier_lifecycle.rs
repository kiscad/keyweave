#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::{collections::BTreeSet, sync::Arc};

fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn filter(mandatory: &[&str], optional: &[&str]) -> ModifierFilter {
    ModifierFilter {
        mandatory: mandatory
            .iter()
            .map(|name| Modifier::named(name).unwrap())
            .collect(),
        optional: optional
            .iter()
            .map(|name| Modifier::named(name).unwrap())
            .collect(),
    }
}
fn configuration(mandatory: &[&str], optional: &[&str], to: &str) -> ConfigSnapshot {
    ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "modifier".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key(to).into()],
            allow_extra_modifiers: false,
            extras: MappingExtras {
                from_modifiers: filter(mandatory, optional),
                ..MappingExtras::default()
            },
        }],
    }
}
fn input(seq: u64, device: u64, name: &str, edge: KeyEdge) -> InputEnvelope {
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
        .handle(input(seq, device, name, edge))
        .unwrap()
        .outputs
        .into_iter()
        .map(|out| (out.key.name(), out.edge))
        .collect()
}
fn edges(items: &[(&str, KeyEdge)]) -> Vec<(String, KeyEdge)> {
    items
        .iter()
        .map(|(name, edge)| ((*name).into(), *edge))
        .collect()
}

#[test]
fn mandatory_is_temporarily_released_and_logically_restored_before_next_down() {
    let mut engine = Engine::new(Arc::new(configuration(&["control"], &[], "b"))).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Up), ("b", KeyEdge::Down)])
    );
    assert_eq!(
        send(&mut engine, 3, 1, "a", KeyEdge::Up),
        edges(&[("b", KeyEdge::Up)])
    );
    assert_eq!(
        send(&mut engine, 4, 1, "c", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Down), ("c", KeyEdge::Down)])
    );
    send(&mut engine, 5, 1, "c", KeyEdge::Up);
    assert_eq!(
        send(&mut engine, 6, 1, "left_control", KeyEdge::Up),
        edges(&[("left_control", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn optional_group_is_preserved_and_extra_group_rejects_the_rule() {
    for (modifier, mapped) in [
        ("left_shift", true),
        ("right_shift", true),
        ("left_option", false),
    ] {
        let mut engine =
            Engine::new(Arc::new(configuration(&["control"], &["shift"], "b"))).unwrap();
        send(&mut engine, 1, 1, "right_control", KeyEdge::Down);
        send(&mut engine, 2, 1, modifier, KeyEdge::Down);
        let expected = if mapped {
            edges(&[("right_control", KeyEdge::Up), ("b", KeyEdge::Down)])
        } else {
            edges(&[("a", KeyEdge::Down)])
        };
        assert_eq!(send(&mut engine, 3, 1, "a", KeyEdge::Down), expected);
        assert!(engine.state().pressed_outputs.contains(&key(modifier)));
        engine.reset(MonoTime(4)).unwrap();
        assert_eq!(engine.state(), EngineState::default());
    }
}

#[test]
fn abstract_group_consumes_left_first_but_allows_right_too() {
    let pressed: BTreeSet<_> = [key("left_control"), key("right_control")]
        .into_iter()
        .collect();
    assert_eq!(
        filter(&["control"], &[]).consumed(&pressed, false),
        Some([key("left_control")].into_iter().collect())
    );
    assert_eq!(
        filter(&["right_control"], &[]).consumed(&pressed, false),
        None
    );
    let mut engine = Engine::new(Arc::new(configuration(&["control"], &[], "b"))).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    send(&mut engine, 2, 1, "right_control", KeyEdge::Down);
    assert_eq!(
        send(&mut engine, 3, 1, "a", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Up), ("b", KeyEdge::Down)])
    );
    assert!(
        engine
            .state()
            .pressed_outputs
            .contains(&key("right_control"))
    );
}

#[test]
fn mandatory_any_consumes_all_flags_even_with_other_mandatory_names() {
    let pressed = [key("left_shift"), key("right_option")]
        .into_iter()
        .collect();
    assert_eq!(
        filter(&["any", "left_control"], &[]).consumed(&pressed, false),
        Some(pressed)
    );
    assert_eq!(
        filter(&["any"], &[]).consumed(&BTreeSet::new(), false),
        Some(BTreeSet::new())
    );
}

#[test]
fn held_modifier_output_masks_old_owners_but_not_a_new_source() {
    let mut engine = Engine::new(Arc::new(configuration(&["control"], &[], "left_shift"))).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Up), ("left_shift", KeyEdge::Down)])
    );
    assert_eq!(
        send(&mut engine, 3, 2, "left_control", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Down)])
    );
    assert!(send(&mut engine, 4, 1, "left_control", KeyEdge::Up).is_empty());
    assert_eq!(
        send(&mut engine, 5, 1, "a", KeyEdge::Up),
        edges(&[("left_shift", KeyEdge::Up)])
    );
    assert_eq!(
        send(&mut engine, 6, 2, "left_control", KeyEdge::Up),
        edges(&[("left_control", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn identical_modifier_output_does_not_emit_an_unnecessary_up_down_pair() {
    let mut engine =
        Engine::new(Arc::new(configuration(&["control"], &[], "left_control"))).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    assert!(send(&mut engine, 2, 1, "a", KeyEdge::Down).is_empty());
    send(&mut engine, 3, 1, "left_control", KeyEdge::Up);
    assert_eq!(
        send(&mut engine, 4, 1, "a", KeyEdge::Up),
        edges(&[("left_control", KeyEdge::Up)])
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn alone_and_after_macros_consume_mandatory_using_old_snapshot() {
    let mut config = configuration(&["control"], &[], "b");
    config.complex[0].extras.if_alone = vec![key("escape").into()];
    config.complex[0].extras.after_key_up = vec![key("tab").into()];
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    send(&mut engine, 2, 1, "a", KeyEdge::Down);
    engine
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    assert_eq!(
        send(&mut engine, 3, 1, "a", KeyEdge::Up),
        edges(&[
            ("b", KeyEdge::Up),
            ("escape", KeyEdge::Down),
            ("escape", KeyEdge::Up),
            ("tab", KeyEdge::Down),
            ("tab", KeyEdge::Up)
        ])
    );
    assert_eq!(engine.state().output_owners, 1);
    assert!(send(&mut engine, 4, 1, "left_control", KeyEdge::Up).is_empty());
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn unplug_clears_mask_without_macro_and_reset_releases_only_emitted_keys() {
    let mut engine = Engine::new(Arc::new(configuration(&["control"], &[], "left_shift"))).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    send(&mut engine, 2, 2, "a", KeyEdge::Down);
    let mut removed = input(3, 1, "a", KeyEdge::Up);
    removed.event = InputEvent::DeviceRemoved;
    assert!(engine.handle(removed).unwrap().outputs.is_empty());
    assert_eq!(engine.state().output_owners, 1);
    assert_eq!(
        engine
            .reset(MonoTime(4))
            .unwrap()
            .iter()
            .map(|out| out.key)
            .collect::<Vec<_>>(),
        vec![key("left_shift")]
    );
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn pointer_activity_flushes_lazy_modifiers_after_repeat_key_up() {
    let mut engine = Engine::new(Arc::new(configuration(&["control"], &[], "b"))).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    send(&mut engine, 2, 1, "a", KeyEdge::Down);
    let mut motion = input(3, 1, "a", KeyEdge::Down);
    motion.event = InputEvent::PointerMove { x: 1, y: 0 };
    assert!(engine.handle(motion.clone()).unwrap().outputs.is_empty());
    send(&mut engine, 4, 1, "a", KeyEdge::Up);
    motion.seq = 5;
    motion.at_ns = MonoTime(5);
    let step = engine.handle(motion).unwrap();
    assert_eq!(step.capture, CaptureDecision::Pass);
    assert_eq!(step.outputs.len(), 1);
    assert_eq!(step.outputs[0].key, key("left_control"));
    assert_eq!(step.outputs[0].edge, KeyEdge::Down);
    send(&mut engine, 6, 1, "left_control", KeyEdge::Up);
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn repeat_false_modifier_pulses_but_consumed_flags_wait_for_original_up() {
    let mut config = configuration(&["control"], &[], "left_shift");
    config.complex[0].to[0].repeat = false;
    let mut engine = Engine::new(Arc::new(config)).unwrap();
    send(&mut engine, 1, 1, "left_control", KeyEdge::Down);
    assert_eq!(
        send(&mut engine, 2, 1, "a", KeyEdge::Down),
        edges(&[
            ("left_control", KeyEdge::Up),
            ("left_shift", KeyEdge::Down),
            ("left_shift", KeyEdge::Up)
        ])
    );
    assert_eq!(engine.suppression_references(), 1);
    assert!(send(&mut engine, 3, 1, "a", KeyEdge::Up).is_empty());
    assert_eq!(engine.suppression_references(), 0);
    assert_eq!(
        send(&mut engine, 4, 1, "c", KeyEdge::Down),
        edges(&[("left_control", KeyEdge::Down), ("c", KeyEdge::Down)])
    );
    engine.reset(MonoTime(5)).unwrap();
    assert_eq!(engine.state(), EngineState::default());
}

#[test]
fn stale_suppression_snapshots_have_a_bound_and_resource_failure_preserves_ups() {
    let mut engine = Engine::new(Arc::new(configuration(
        &["control"],
        &["shift"],
        "left_shift",
    )))
    .unwrap();
    let mut seq = 0;
    let mut exhausted = false;
    for round in 1..=150 {
        for device in 1..=128 {
            seq += 1;
            engine
                .handle(input(seq, device, "left_control", KeyEdge::Down))
                .unwrap();
        }
        seq += 1;
        let before = engine.state();
        match engine.handle(input(seq, round + 1000, "a", KeyEdge::Down)) {
            Ok(_) => {}
            Err(EngineError::ResourceLimit) => {
                assert_eq!(engine.state(), before);
                exhausted = true;
                break;
            }
            other => panic!("unexpected result: {other:?}"),
        }
        for device in 1..=128 {
            seq += 1;
            engine
                .handle(input(seq, device, "left_control", KeyEdge::Up))
                .unwrap();
        }
        assert!(engine.suppression_references() <= MAX_PERSISTENT_SUPPRESSION_REFERENCES);
    }
    assert!(exhausted);
    let outputs = engine.reset(MonoTime(seq + 1)).unwrap();
    assert_eq!(outputs.len(), 2); // Last Control batch and held mapped Shift.
    assert_eq!(engine.suppression_references(), 0);
    assert_eq!(engine.state(), EngineState::default());
}
