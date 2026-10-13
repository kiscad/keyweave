#![allow(clippy::unwrap_used)]
use keyweave_config::*;
use keyweave_engine::{Engine, EngineError, simultaneous::*};
use keyweave_types::*;
use serde_json::{Value, json};
use std::sync::Arc;
fn compile_value(mut manipulator: Value, parameters: Value) -> CompiledConfig {
    if manipulator.get("type").is_none() {
        manipulator["type"] = json!("basic");
    }
    compile(&json!({"profiles":[{"selected":true,"complex_modifications":{"parameters":parameters,"rules":[{"description":"private chord","manipulators":[manipulator]}]}}]}).to_string(),&BackendCapabilities::offline(),ConfigGeneration(1)).unwrap()
}
fn chord() -> Value {
    json!({"from":{"simultaneous":[{"key_code":"a"},{"key_code":"b"}]},"to":[{"key_code":"x"}]})
}
#[test]
fn all_options_and_after_up_ir_preserve_raw_metadata_but_refuse_runtime_activation() {
    let mut manipulator = chord();
    manipulator["from"]["modifiers"] = json!({"mandatory":["shift"],"optional":["any"]});
    manipulator["from"]["simultaneous_options"] = json!({"description":"private options","detect_key_down_uninterruptedly":true,"key_down_order":"strict_inverse","key_up_order":"strict","key_up_when":"all","to_after_key_up":{"set_variable":{"name":"flag","value":1},"hold_down_milliseconds":10}});
    manipulator["parameters"] = json!({"basic.simultaneous_threshold_milliseconds":25});
    let result = compile_value(
        manipulator,
        json!({"basic.simultaneous_threshold_milliseconds":75}),
    );
    assert!(!result.executable_without_loss());
    assert_eq!(result.disabled_rules, 1);
    assert!(result.diagnostics.iter().any(|d| d.code == "KW-SIM-001"));
    let spec = result.snapshot.complex[0]
        .extras
        .simultaneous
        .as_ref()
        .unwrap();
    assert_eq!(spec.threshold_ns, 25_000_000);
    assert_eq!(spec.key_down_order, KeyOrder::StrictInverse);
    assert_eq!(spec.key_up_order, KeyOrder::Strict);
    assert_eq!(spec.key_up_when, KeyUpWhen::All);
    assert!(spec.detect_key_down_uninterruptedly);
    assert_eq!(spec.after_key_up.len(), 1);
    assert_eq!(spec.after_key_up[0].hold_down_ns, 10_000_000);
    assert!(result.snapshot.variable_names().is_ok());
    assert_eq!(
        result.raw["profiles"][0]["complex_modifications"]["rules"][0]["description"],
        "private chord"
    );
    assert!(matches!(
        Engine::new(Arc::new(result.snapshot)),
        Err(EngineError::UnsupportedSimultaneous)
    ));
}
#[test]
fn default_profile_override_zero_threshold_and_after_up_only_are_parsed() {
    for threshold in [None, Some(0), Some(100)] {
        let parameters = threshold.map_or(
            json!({}),
            |value| json!({"basic.simultaneous_threshold_milliseconds":value}),
        );
        let result = compile_value(chord(), parameters);
        let spec = result.snapshot.complex[0]
            .extras
            .simultaneous
            .as_ref()
            .unwrap();
        assert_eq!(spec.threshold_ns, threshold.unwrap_or(50) * 1_000_000);
        assert_eq!(spec.key_down_order, KeyOrder::Insensitive);
        assert_eq!(spec.key_up_order, KeyOrder::Insensitive);
        assert_eq!(spec.key_up_when, KeyUpWhen::Any);
        assert!(!spec.detect_key_down_uninterruptedly);
    }
    let mut manipulator = chord();
    manipulator.as_object_mut().unwrap().remove("to");
    manipulator["from"]["simultaneous_options"] = json!({"to_after_key_up":[{"key_code":"y"}]});
    let result = compile_value(manipulator, json!({}));
    assert_eq!(result.snapshot.complex.len(), 1);
    assert_eq!(result.disabled_rules, 1);
}
#[test]
fn malformed_member_options_and_exclusive_from_shapes_are_precisely_disabled() {
    let mut cases = vec![];
    for value in [
        json!([]),
        json!([{ "key_code":"a"},{"key_code":"a"}]),
        Value::Array(vec![json!({"key_code":"a"}); 33]),
        json!("a"),
    ] {
        let mut body = chord();
        body["from"]["simultaneous"] = value;
        cases.push(body);
    }
    for options in [
        json!({"key_up_order":"invalid"}),
        json!({"key_down_order":false}),
        json!({"key_up_when":"none"}),
        json!({"detect_key_down_uninterruptedly":1}),
        json!({"unsupported":true}),
        json!(false),
    ] {
        let mut body = chord();
        body["from"]["simultaneous_options"] = options;
        cases.push(body);
    }
    let mut body = chord();
    body["from"]["key_code"] = json!("c");
    cases.push(body);
    cases.push(json!({"from":{"key_code":"a","simultaneous_options":{}},"to":[{"key_code":"x"}]}));
    for body in cases {
        let result = compile_value(body, json!({}));
        assert!(!result.executable_without_loss());
        assert!(result.snapshot.complex.is_empty());
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.pointer.contains("/from"))
        );
    }
}
#[test]
fn thresholds_reject_bad_type_range_and_unknown_parameter_without_accepting_chord() {
    for value in [json!(-1), json!(1.5), json!("50"), json!(86_400_001)] {
        let mut body = chord();
        body["parameters"] = json!({"basic.simultaneous_threshold_milliseconds":value});
        let result = compile_value(body, json!({}));
        assert!(!result.executable_without_loss());
        assert!(result.snapshot.complex.is_empty());
        assert!(result.diagnostics.iter().any(|d| {
            d.pointer
                .ends_with("/basic.simultaneous_threshold_milliseconds")
        }));
    }
    let mut body = chord();
    body["parameters"] = json!({"unknown_parameter":1});
    let result = compile_value(body, json!({}));
    assert!(result.snapshot.complex.is_empty());
}
#[test]
fn unsupported_after_up_action_and_same_rule_bad_sibling_disable_whole_rule() {
    let mut body = chord();
    body["from"]["simultaneous_options"] =
        json!({"to_after_key_up":{"shell_command":"private command"}});
    let result = compile_value(body, json!({}));
    assert!(result.snapshot.complex.is_empty());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.pointer.contains("/to_after_key_up"))
    );
    let mut valid = chord();
    valid["type"] = json!("basic");
    let raw = json!({"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[valid,{"type":"unsupported","from":{"key_code":"c"},"to":[]}]}]}}]});
    let result = compile(
        &raw.to_string(),
        &BackendCapabilities::offline(),
        ConfigGeneration(1),
    )
    .unwrap();
    assert!(result.snapshot.complex.is_empty());
    assert_eq!(result.disabled_rules, 1);
    assert!(!result.executable_without_loss());
}

#[test]
fn effective_threshold_clamps_to_upstream_one_second_while_raw_value_is_preserved() {
    let result = compile_value(
        chord(),
        json!({"basic.simultaneous_threshold_milliseconds":86400000}),
    );
    assert_eq!(
        result.snapshot.complex[0]
            .extras
            .simultaneous
            .as_ref()
            .unwrap()
            .threshold_ns,
        MAX_CHORD_THRESHOLD_NS
    );
    assert_eq!(
        result.raw["profiles"][0]["complex_modifications"]["parameters"]["basic.simultaneous_threshold_milliseconds"],
        86400000
    );
}
