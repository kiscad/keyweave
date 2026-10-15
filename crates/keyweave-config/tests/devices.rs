#![allow(clippy::unwrap_used)]
use keyweave_config::{CompileMode, compile_with_mode};
use keyweave_engine::{
    Condition, ConfigSnapshot, Engine, EngineState, physical_controller::PhysicalStageController,
};
use keyweave_types::*;
use serde_json::{Value, json};
use std::sync::Arc;
fn map(from: &str, to: &str) -> Value {
    json!({"from":{"key_code":from},"to":[{"key_code":to}]})
}
fn raw() -> Value {
    json!({"profiles":[{"selected":true,"simple_modifications":[map("a","d")],"devices":[
        {"identifiers":{"vendor_id":10,"product_id":20,"is_keyboard":true,"is_pointing_device":false,"location_id":999},"ignore":false,"simple_modifications":[map("a","b")]},
        {"identifiers":{"vendor_id":10,"product_id":20,"is_keyboard":true,"is_pointing_device":false},"simple_modifications":[map("a","c")]}
    ],"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"key_code":"b"},"to":[{"key_code":"x"}]}]}]}}]})
}
fn compiled(raw: &Value, mode: CompileMode) -> Arc<ConfigSnapshot> {
    let config = compile_with_mode(
        &raw.to_string(),
        &BackendCapabilities::linux(),
        ConfigGeneration(1),
        mode,
    )
    .unwrap();
    assert!(config.executable_without_loss(), "{:?}", config.diagnostics);
    assert_eq!(&config.raw, raw);
    Arc::new(config.snapshot)
}
fn context(revision: u64, at: u64, until: u64) -> ContextSnapshot {
    ContextSnapshot {
        revision,
        provider: "test trusted identities".into(),
        observed_at_ns: MonoTime(at),
        valid_until_ns: MonoTime(until),
        devices: Some(vec![
            DeviceContext {
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                properties: DeviceProperties {
                    vendor_id: Some(10),
                    product_id: Some(20),
                    is_keyboard: Some(true),
                    is_pointing_device: Some(false),
                    location_id: Some(111),
                    ..Default::default()
                },
            },
            DeviceContext {
                device: DeviceInstanceId {
                    id: 2,
                    generation: 1,
                },
                properties: DeviceProperties {
                    vendor_id: Some(10),
                    product_id: Some(20),
                    is_keyboard: Some(false),
                    is_pointing_device: Some(true),
                    ..Default::default()
                },
            },
            DeviceContext {
                device: DeviceInstanceId {
                    id: 3,
                    generation: 1,
                },
                properties: DeviceProperties {
                    vendor_id: Some(10),
                    product_id: Some(20),
                    ..Default::default()
                },
            },
        ]),
        ..Default::default()
    }
}
enum Runner {
    Legacy(Box<Engine>),
    Staged(Box<PhysicalStageController>),
}
impl Runner {
    fn new(raw: &Value, mode: CompileMode) -> Self {
        let config = compiled(raw, mode);
        match mode {
            CompileMode::Legacy => Self::Legacy(Box::new(Engine::new(config).unwrap())),
            CompileMode::StagedKeyboard => {
                Self::Staged(Box::new(PhysicalStageController::new(config).unwrap()))
            }
        }
    }
    fn step(
        &mut self,
        seq: u64,
        at: u64,
        device: DeviceInstanceId,
        event: InputEvent,
    ) -> Vec<(KeyId, KeyEdge)> {
        let envelope = InputEnvelope {
            seq,
            at_ns: MonoTime(at),
            device,
            provenance: Provenance::Physical,
            event,
        };
        let output = match self {
            Self::Legacy(e) => e.handle(envelope).unwrap().outputs,
            Self::Staged(e) => {
                let mut output = e.handle(envelope).unwrap().outputs;
                let mut done = false;
                for _ in 0..128 {
                    match e.poll(MonoTime(at)).unwrap() {
                        Some(s) => output.extend(s.outputs),
                        None => {
                            done = true;
                            break;
                        }
                    }
                }
                assert!(done);
                output
            }
        };
        output.into_iter().map(|o| (o.key, o.edge)).collect()
    }
    fn key(
        &mut self,
        seq: u64,
        at: u64,
        id: u64,
        generation: u64,
        edge: KeyEdge,
    ) -> Vec<(KeyId, KeyEdge)> {
        self.step(
            seq,
            at,
            DeviceInstanceId { id, generation },
            InputEvent::Key {
                key: "a".parse().unwrap(),
                edge,
            },
        )
    }
    fn activate(&mut self) {
        let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
        match self {
            Self::Legacy(e) => e.activate(config).unwrap(),
            Self::Staged(e) => e.activate(config).unwrap(),
        }
    }
    fn state(&self) -> EngineState {
        match self {
            Self::Legacy(e) => e.state(),
            Self::Staged(e) => e.state(),
        }
    }
}
fn edge(name: &str, edge: KeyEdge) -> Vec<(KeyId, KeyEdge)> {
    vec![(name.parse().unwrap(), edge)]
}
#[test]
fn device_order_precedes_global_simple_and_uses_upstream_four_field_selector() {
    let config = compiled(&raw(), CompileMode::Legacy);
    assert_eq!(config.simple.len(), 3);
    assert!(
        config.simple[0]
            .id
            .contains("/devices/0/simple_modifications/0")
    );
    assert!(
        config.simple[1]
            .id
            .contains("/devices/1/simple_modifications/0")
    );
    assert!(config.simple[2].id.ends_with("/simple_modifications/0"));
    let Condition::Device {
        identifiers,
        unless,
        exists,
    } = &config.simple[0].extras.conditions[0]
    else {
        panic!("device condition")
    };
    assert!(!unless && !exists);
    assert_eq!(identifiers[0].location_id, None);
    assert_eq!(identifiers[0].is_keyboard, Some(true));
    let raw = json!({"profiles":[{"devices":[{"identifiers":{},"simple_modifications":[map("a","b")]}]}]});
    let config = compiled(&raw, CompileMode::Legacy);
    let Condition::Device { identifiers, .. } = &config.simple[0].extras.conditions[0] else {
        panic!("device condition")
    };
    assert_eq!(identifiers[0].vendor_id, Some(0));
    assert_eq!(identifiers[0].product_id, Some(0));
    assert_eq!(identifiers[0].is_keyboard, Some(false));
    assert_eq!(identifiers[0].is_pointing_device, Some(false));
}
#[test]
fn actual_engines_route_by_identity_and_fallback_for_unknown_expired_or_other_generation() {
    for mode in [CompileMode::Legacy, CompileMode::StagedKeyboard] {
        let mut runner = Runner::new(&raw(), mode);
        assert!(
            runner
                .step(
                    1,
                    0,
                    DeviceInstanceId {
                        id: 1,
                        generation: 1
                    },
                    InputEvent::ContextChanged {
                        context: context(1, 0, 100)
                    }
                )
                .is_empty()
        );
        let mut seq = 2;
        // Same source key, different trusted physical interfaces; same matching
        // vendor/product with unknown flags must not be treated as a match.
        for (id, generation, at, target) in [
            (1, 1, 1, "x"),
            (2, 1, 3, "d"),
            (3, 1, 5, "d"),
            (4, 1, 7, "d"),
            (1, 2, 9, "d"),
            (2, 1, 100, "d"),
        ] {
            assert_eq!(
                runner.key(seq, at, id, generation, KeyEdge::Down),
                edge(target, KeyEdge::Down)
            );
            seq += 1;
            assert_eq!(
                runner.key(seq, at + 1, id, generation, KeyEdge::Up),
                edge(target, KeyEdge::Up)
            );
            seq += 1;
        }
        assert_eq!(runner.state().bindings, 0);
        assert!(runner.state().pressed_outputs.is_empty());
    }
}
#[test]
fn device_selected_binding_keeps_saved_outputs_across_context_change_reload_and_removal() {
    for mode in [CompileMode::Legacy, CompileMode::StagedKeyboard] {
        let mut runner = Runner::new(&raw(), mode);
        runner.step(
            1,
            0,
            DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            InputEvent::ContextChanged {
                context: context(1, 0, 100),
            },
        );
        assert_eq!(
            runner.key(2, 1, 1, 1, KeyEdge::Down),
            edge("x", KeyEdge::Down)
        );
        let mut changed = context(2, 2, 100);
        changed.devices = None;
        assert!(
            runner
                .step(
                    3,
                    2,
                    DeviceInstanceId {
                        id: 1,
                        generation: 1
                    },
                    InputEvent::ContextChanged { context: changed }
                )
                .is_empty()
        );
        runner.activate();
        assert_eq!(
            runner.key(4, 101, 1, 1, KeyEdge::Up),
            edge("x", KeyEdge::Up)
        );
        assert_eq!(
            runner.key(5, 102, 1, 1, KeyEdge::Down),
            edge("a", KeyEdge::Down)
        );
        assert_eq!(
            runner.step(
                6,
                103,
                DeviceInstanceId {
                    id: 1,
                    generation: 1
                },
                InputEvent::DeviceRemoved
            ),
            edge("a", KeyEdge::Up)
        );
        assert_eq!(runner.state().bindings, 0);
        assert_eq!(runner.state().output_owners, 0);
    }
}
#[test]
fn bad_device_settings_reject_entire_device_and_hook_never_accepts_device_identity() {
    for (field, value) in [
        ("ignore", json!(true)),
        ("mouse_flip_x", json!(false)),
        ("fn_function_keys", json!([map("f1", "f2")])),
        ("unknown", json!(true)),
    ] {
        let mut input = raw();
        input["profiles"][0]["devices"][0][field] = value;
        let config = compile_with_mode(
            &input.to_string(),
            &BackendCapabilities::linux(),
            ConfigGeneration(1),
            CompileMode::Legacy,
        )
        .unwrap();
        assert!(!config.executable_without_loss());
        assert!(
            !config
                .snapshot
                .simple
                .iter()
                .any(|m| m.id.contains("/devices/0/"))
        );
        assert!(
            config
                .diagnostics
                .iter()
                .any(|d| d.pointer == format!("/profiles/0/devices/0/{field}"))
        );
    }
    let config = compile_with_mode(
        &raw().to_string(),
        &BackendCapabilities::windows_hook(),
        ConfigGeneration(1),
        CompileMode::StagedKeyboard,
    )
    .unwrap();
    assert!(!config.executable_without_loss());
    assert_eq!(config.snapshot.simple.len(), 1);
    assert!(config.snapshot.simple[0].extras.conditions.is_empty());
    let mut input = raw();
    input["profiles"][0]["devices"][0]["identifiers"]["vendor_id"] = json!(-1);
    let config = compile_with_mode(
        &input.to_string(),
        &BackendCapabilities::linux(),
        ConfigGeneration(1),
        CompileMode::Legacy,
    )
    .unwrap();
    assert!(
        config
            .diagnostics
            .iter()
            .any(|d| d.pointer == "/profiles/0/devices/0/identifiers/vendor_id")
    );
    let input = json!({"profiles":[{"devices":vec![json!({"identifiers":{}});257]}]});
    assert!(
        compile_with_mode(
            &input.to_string(),
            &BackendCapabilities::linux(),
            ConfigGeneration(1),
            CompileMode::Legacy
        )
        .is_err()
    );
}
#[test]
fn bad_sibling_mapping_discards_device_batch_and_all_rules_share_compiler_budget() {
    let mut input = raw();
    input["profiles"][0]["devices"][0]["simple_modifications"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from":{"key_code":"f1"},"to":[],"unsupported":true}));
    let config = compile_with_mode(
        &input.to_string(),
        &BackendCapabilities::linux(),
        ConfigGeneration(1),
        CompileMode::Legacy,
    )
    .unwrap();
    assert!(!config.executable_without_loss());
    assert!(
        !config
            .snapshot
            .simple
            .iter()
            .any(|m| m.id.contains("/devices/0/"))
    );
    assert!(
        config
            .snapshot
            .simple
            .iter()
            .any(|m| m.id.contains("/devices/1/"))
    );
    let raw = json!({"profiles":[{"simple_modifications":[map("a","d")],"devices":[{"identifiers":{"vendor_id":1},"simple_modifications":vec![map("a","b");4096]}]}]});
    assert!(
        compile_with_mode(
            &raw.to_string(),
            &BackendCapabilities::linux(),
            ConfigGeneration(1),
            CompileMode::Legacy
        )
        .is_err()
    );
}
