#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
use std::sync::Arc;

fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn device(generation: u64) -> DeviceInstanceId {
    DeviceInstanceId { id: 1, generation }
}
fn condition(unless: bool, exists: bool) -> Condition {
    Condition::Device {
        identifiers: vec![DeviceProperties {
            vendor_id: Some(42),
            ..Default::default()
        }],
        unless,
        exists,
    }
}
fn engine(conditions: Vec<Condition>, to: Vec<Action>, after: Vec<Action>) -> Engine {
    Engine::new(Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "devices".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "rule".into(),
            from: key("a"),
            to,
            allow_extra_modifiers: true,
            extras: MappingExtras {
                conditions,
                after_key_up: after,
                ..Default::default()
            },
        }],
    }))
}
fn context(revision: u64, vendor: Option<u32>, generation: u64) -> ContextSnapshot {
    ContextSnapshot {
        revision,
        application: None,
        input_source: None,
        provider: "synthetic".into(),
        observed_at_ns: MonoTime(0),
        valid_until_ns: MonoTime(100),
        devices: Some(vec![DeviceContext {
            device: device(generation),
            properties: DeviceProperties {
                vendor_id: vendor,
                ..Default::default()
            },
        }]),
    }
}
fn event(seq: u64, time: u64, generation: u64, event: InputEvent) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(time),
        device: device(generation),
        provenance: Provenance::Physical,
        event,
    }
}
fn update(engine: &mut Engine, seq: u64, time: u64, context: ContextSnapshot) {
    let step = engine
        .handle(event(seq, time, 1, InputEvent::ContextChanged { context }))
        .unwrap();
    assert!(step.outputs.is_empty());
    assert_eq!(step.capture, CaptureDecision::Pass);
}
fn stroke(
    engine: &mut Engine,
    seq: u64,
    time: u64,
    generation: u64,
    name: &str,
    edge: KeyEdge,
) -> Vec<(KeyId, KeyEdge)> {
    engine
        .handle(event(
            seq,
            time,
            generation,
            InputEvent::Key {
                key: key(name),
                edge,
            },
        ))
        .unwrap()
        .outputs
        .into_iter()
        .map(|output| (output.key, output.edge))
        .collect()
}
#[test]
fn unknown_and_expired_context_never_enable_if_or_unless() {
    for unless in [false, true] {
        let mut engine = engine(
            vec![condition(unless, false)],
            vec![key("b").into()],
            vec![],
        );
        assert_eq!(
            stroke(&mut engine, 1, 0, 1, "a", KeyEdge::Down),
            [(key("a"), KeyEdge::Down)]
        );
        stroke(&mut engine, 2, 1, 1, "a", KeyEdge::Up);
        update(&mut engine, 3, 2, context(1, None, 1));
        assert_eq!(
            stroke(&mut engine, 4, 3, 1, "a", KeyEdge::Down),
            [(key("a"), KeyEdge::Down)]
        );
        stroke(&mut engine, 5, 4, 1, "a", KeyEdge::Up);
        update(
            &mut engine,
            6,
            5,
            context(2, Some(if unless { 7 } else { 42 }), 1),
        );
        assert_eq!(
            stroke(&mut engine, 7, 99, 1, "a", KeyEdge::Down),
            [(key("b"), KeyEdge::Down)]
        );
        assert_eq!(
            stroke(&mut engine, 8, 100, 1, "a", KeyEdge::Up),
            [(key("b"), KeyEdge::Up)]
        );
        assert_eq!(
            stroke(&mut engine, 9, 100, 1, "a", KeyEdge::Down),
            [(key("a"), KeyEdge::Down)]
        );
    }
}
#[test]
fn identifiers_are_or_fields_are_and_with_known_false_distinct_from_unknown() {
    let device = device(1);
    let mut context = context(1, Some(42), 1);
    let cond = Condition::Device {
        identifiers: vec![
            DeviceProperties {
                vendor_id: Some(42),
                is_keyboard: Some(false),
                ..Default::default()
            },
            DeviceProperties {
                vendor_id: Some(7),
                ..Default::default()
            },
        ],
        unless: false,
        exists: false,
    };
    assert_eq!(cond.evaluate(&context, device, MonoTime(0)), Truth::Unknown);
    context.devices.as_mut().unwrap()[0].properties.is_keyboard = Some(false);
    assert_eq!(cond.evaluate(&context, device, MonoTime(0)), Truth::True);
    context.devices.as_mut().unwrap()[0].properties.is_keyboard = Some(true);
    assert_eq!(cond.evaluate(&context, device, MonoTime(0)), Truth::False);
    context.devices.as_mut().unwrap()[0].properties.vendor_id = Some(7);
    assert_eq!(cond.evaluate(&context, device, MonoTime(0)), Truth::True);
}
#[test]
fn inventory_unknown_empty_and_unannounced_device_are_distinct() {
    let cond = condition(true, true);
    let mut context = context(1, None, 1);
    context.devices = None;
    assert_eq!(
        cond.evaluate(&context, device(1), MonoTime(0)),
        Truth::Unknown
    );
    context.devices = Some(vec![]);
    assert_eq!(cond.evaluate(&context, device(1), MonoTime(0)), Truth::True);
    let mut engine = engine(vec![cond], vec![key("b").into()], vec![]);
    update(&mut engine, 1, 0, context);
    // Observing the real source adds unknown properties before matching.
    assert_eq!(
        stroke(&mut engine, 2, 1, 1, "a", KeyEdge::Down),
        [(key("a"), KeyEdge::Down)]
    );
}
#[test]
fn action_filter_selects_last_live_action_and_up_uses_current_context() {
    let mut action: Action = key("c").into();
    action.conditions = vec![condition(true, false)];
    let mut after: Action = key("d").into();
    after.conditions = vec![condition(true, false)];
    let mut engine = engine(
        vec![condition(false, false)],
        vec![key("b").into(), action],
        vec![after],
    );
    update(&mut engine, 1, 0, context(1, Some(42), 1));
    assert_eq!(
        stroke(&mut engine, 2, 1, 1, "a", KeyEdge::Down),
        [(key("b"), KeyEdge::Down)]
    );
    update(&mut engine, 3, 2, context(2, Some(7), 1));
    assert_eq!(
        stroke(&mut engine, 4, 3, 1, "a", KeyEdge::Up),
        [
            (key("b"), KeyEdge::Up),
            (key("d"), KeyEdge::Down),
            (key("d"), KeyEdge::Up)
        ]
    );
    assert_eq!(engine.state().output_owners, 0);
}
#[test]
fn reconnect_invalidates_metadata_and_stale_updates_are_atomic() {
    let mut engine = engine(vec![condition(true, false)], vec![key("b").into()], vec![]);
    update(&mut engine, 1, 0, context(1, Some(7), 1));
    assert_eq!(
        stroke(&mut engine, 2, 1, 1, "a", KeyEdge::Down),
        [(key("b"), KeyEdge::Down)]
    );
    assert_eq!(
        stroke(&mut engine, 3, 2, 2, "a", KeyEdge::Down),
        [(key("b"), KeyEdge::Up), (key("a"), KeyEdge::Down)]
    );
    let before = engine.state();
    assert_eq!(
        engine
            .handle(event(
                4,
                3,
                1,
                InputEvent::ContextChanged {
                    context: context(2, Some(7), 1)
                }
            ))
            .unwrap_err(),
        EngineError::InvalidContext
    );
    assert_eq!(engine.context_revision(), 1);
    assert_eq!(engine.state(), before);
    update(&mut engine, 4, 3, context(2, Some(7), 2));
    assert!(
        engine
            .handle(event(5, 4, 1, InputEvent::DeviceRemoved))
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(
        stroke(&mut engine, 6, 5, 2, "a", KeyEdge::Up),
        [(key("a"), KeyEdge::Up)]
    );
    assert_eq!(
        stroke(&mut engine, 7, 6, 2, "a", KeyEdge::Down),
        [(key("b"), KeyEdge::Down)]
    );
}
#[test]
fn context_limits_revision_and_duplicates_fail_without_advancing_event_order() {
    let mut engine = engine(vec![], vec![key("b").into()], vec![]);
    update(&mut engine, 1, 0, context(1, Some(42), 1));
    stroke(&mut engine, 2, 1, 1, "a", KeyEdge::Down);
    let before = engine.state();
    let base = context(2, Some(42), 1);
    let mut cases = vec![context(1, Some(42), 1)];
    let mut invalid = base.clone();
    invalid.provider = String::new();
    cases.push(invalid);
    let mut invalid = base.clone();
    invalid.observed_at_ns = MonoTime(4);
    cases.push(invalid);
    let mut invalid = base.clone();
    invalid.valid_until_ns = MonoTime(MAX_CONTEXT_LIFETIME_NS + 1);
    cases.push(invalid);
    let mut invalid = base.clone();
    let duplicate = invalid.devices.as_ref().unwrap()[0].clone();
    invalid.devices.as_mut().unwrap().push(duplicate);
    cases.push(invalid);
    let mut invalid = base.clone();
    invalid.devices.as_mut().unwrap()[0]
        .properties
        .device_address = Some("a".repeat(257));
    cases.push(invalid);
    let mut invalid = base.clone();
    invalid.devices = Some(
        (0..257)
            .map(|id| DeviceContext {
                device: DeviceInstanceId { id, generation: 1 },
                properties: Default::default(),
            })
            .collect(),
    );
    cases.push(invalid);
    let mut invalid = base.clone();
    invalid.application = Some(ApplicationContext {
        bundle_identifier: Some("a".repeat(4097)),
        file_path: None,
    });
    cases.push(invalid);
    let mut invalid = base.clone();
    invalid.input_source = Some(InputSourceContext {
        language: Some("a".repeat(4097)),
        ..Default::default()
    });
    cases.push(invalid);
    for context in cases {
        assert_eq!(
            engine
                .handle(event(3, 2, 1, InputEvent::ContextChanged { context }))
                .unwrap_err(),
            EngineError::InvalidContext
        );
        assert_eq!(engine.state(), before);
        assert_eq!(engine.context_revision(), 1);
    }
    update(&mut engine, 3, 2, base);
    assert_eq!(
        stroke(&mut engine, 4, 3, 1, "a", KeyEdge::Up),
        [(key("b"), KeyEdge::Up)]
    );
}
#[test]
fn full_inventory_falls_back_to_unknown_without_losing_releases() {
    let mut engine = engine(vec![condition(true, true)], vec![key("b").into()], vec![]);
    let mut context = context(1, Some(7), 1);
    context.devices = Some(
        (2..258)
            .map(|id| DeviceContext {
                device: DeviceInstanceId { id, generation: 1 },
                properties: DeviceProperties {
                    vendor_id: Some(7),
                    ..Default::default()
                },
            })
            .collect(),
    );
    update(&mut engine, 1, 0, context);
    assert_eq!(
        stroke(&mut engine, 2, 1, 1, "a", KeyEdge::Down),
        [(key("a"), KeyEdge::Down)]
    );
    assert_eq!(
        stroke(&mut engine, 3, 2, 1, "a", KeyEdge::Up),
        [(key("a"), KeyEdge::Up)]
    );
    assert_eq!(engine.state().output_owners, 0);
}

#[test]
fn filtered_modifier_action_does_not_keep_consumed_scope_hidden() {
    let mut skipped: Action = key("right_shift").into();
    skipped.conditions = vec![condition(true, false)];
    let config = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "filtered scope".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "a".into(),
            from: key("a"),
            to: vec![key("b").into(), skipped],
            allow_extra_modifiers: false,
            extras: MappingExtras {
                from_modifiers: ModifierFilter {
                    mandatory: [Modifier::Control].into_iter().collect(),
                    optional: Default::default(),
                },
                ..Default::default()
            },
        }],
    });
    let mut engine = Engine::new(config);
    update(&mut engine, 1, 0, context(1, Some(42), 1));
    stroke(&mut engine, 2, 1, 1, "left_control", KeyEdge::Down);
    assert_eq!(
        stroke(&mut engine, 3, 2, 1, "a", KeyEdge::Down),
        [
            (key("left_control"), KeyEdge::Up),
            (key("b"), KeyEdge::Down)
        ]
    );
    assert_eq!(engine.suppression_references(), 0);
    assert_eq!(
        stroke(&mut engine, 4, 3, 1, "c", KeyEdge::Down),
        [
            (key("left_control"), KeyEdge::Down),
            (key("c"), KeyEdge::Down)
        ]
    );
    engine.reset(MonoTime(4)).unwrap();
    assert_eq!(engine.state().output_owners, 0);
}
#[test]
fn reload_and_context_change_release_old_binding_and_evaluate_old_up_actions() {
    let mut after: Action = key("d").into();
    after.conditions = vec![condition(true, false)];
    let mut engine = engine(
        vec![condition(false, false)],
        vec![key("b").into()],
        vec![after],
    );
    update(&mut engine, 1, 0, context(1, Some(42), 1));
    stroke(&mut engine, 2, 1, 1, "a", KeyEdge::Down);
    engine
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    update(&mut engine, 3, 2, context(2, Some(7), 1));
    assert_eq!(
        stroke(&mut engine, 4, 3, 1, "a", KeyEdge::Up),
        [
            (key("b"), KeyEdge::Up),
            (key("d"), KeyEdge::Down),
            (key("d"), KeyEdge::Up)
        ]
    );
    assert_eq!(
        stroke(&mut engine, 5, 4, 1, "a", KeyEdge::Down),
        [(key("a"), KeyEdge::Down)]
    );
}
