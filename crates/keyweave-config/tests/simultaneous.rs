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

fn staged_compile(manipulators: Value, capabilities: &BackendCapabilities) -> CompiledConfig {
    let text = json!({"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":manipulators}]}}]}).to_string();
    compile_with_mode(
        &text,
        capabilities,
        ConfigGeneration(3),
        CompileMode::StagedKeyboard,
    )
    .unwrap()
}
#[test]
fn staged_mode_preserves_full_chord_ir_and_default_mode_stays_lossy() {
    let mut rule = chord();
    rule["type"] = json!("basic");
    rule["from"]["simultaneous_options"] = json!({"key_down_order":"strict_inverse","key_up_order":"strict","key_up_when":"all","detect_key_down_uninterruptedly":true,"to_after_key_up":[{"key_code":"y","hold_down_milliseconds":10}]});
    let result = staged_compile(json!([rule.clone()]), &BackendCapabilities::offline());
    assert!(result.executable_without_loss());
    assert!(result.diagnostics.is_empty());
    let spec = result.snapshot.complex[0]
        .extras
        .simultaneous
        .as_ref()
        .unwrap();
    assert_eq!(spec.key_down_order, KeyOrder::StrictInverse);
    assert_eq!(spec.key_up_when, KeyUpWhen::All);
    assert_eq!(spec.after_key_up[0].hold_down_ns, 10_000_000);
    assert_eq!(result.snapshot.generation, ConfigGeneration(3));
    assert!(
        keyweave_engine::physical_controller::PhysicalStageController::new_with_chord_model(
            Arc::new(result.snapshot.clone())
        )
        .is_ok()
    );
    assert!(
        compile_value(rule, json!({}))
            .diagnostics
            .iter()
            .any(|d| d.code == "KW-SIM-001")
    );
}
#[test]
fn staged_mode_keeps_containing_rule_atomic_on_bad_options_or_sibling() {
    let mut valid = chord();
    valid["type"] = json!("basic");
    for invalid in [
        json!({"type":"basic","from":{"key_code":"c"},"to":[{"shell_command":"ignored"}]}),
        {
            let mut bad = valid.clone();
            bad["from"]["simultaneous_options"] = json!({"key_up_when":"invalid"});
            bad
        },
    ] {
        let result = staged_compile(
            json!([valid.clone(), invalid]),
            &BackendCapabilities::offline(),
        );
        assert!(!result.executable_without_loss());
        assert_eq!(result.disabled_rules, 1);
        assert!(result.snapshot.complex.is_empty());
    }
}
#[test]
fn staged_mode_keeps_backend_gate_for_group_after_up_and_device_conditions() {
    let mut rule = chord();
    rule["type"] = json!("basic");
    rule["from"]["simultaneous_options"] =
        json!({"to_after_key_up":[{"consumer_key_code":"volume_increment"}]});
    assert!(
        staged_compile(json!([rule.clone()]), &BackendCapabilities::linux())
            .executable_without_loss()
    );
    assert!(
        !staged_compile(json!([rule]), &BackendCapabilities::windows_hook())
            .executable_without_loss()
    );
    let mut rule = chord();
    rule["type"] = json!("basic");
    rule["conditions"] = json!([{"type":"device_if","identifiers":[{"vendor_id":1}]}]);
    assert!(
        staged_compile(json!([rule.clone()]), &BackendCapabilities::linux())
            .executable_without_loss()
    );
    assert!(
        !staged_compile(json!([rule]), &BackendCapabilities::windows_hook())
            .executable_without_loss()
    );
}
#[test]
fn staged_mode_does_not_enable_simple_simultaneous() {
    let text = json!({"profiles":[{"selected":true,"simple_modifications":[{"from":{"simultaneous":[{"key_code":"a"},{"key_code":"b"}]},"to":[{"key_code":"c"}]}]}]}).to_string();
    let result = compile_with_mode(
        &text,
        &BackendCapabilities::offline(),
        ConfigGeneration(1),
        CompileMode::StagedKeyboard,
    )
    .unwrap();
    assert!(!result.executable_without_loss());
    assert!(result.snapshot.simple.is_empty());
}

#[test]
fn staged_button_chords_preserve_first_event_and_require_backend_capability() {
    for members in [
        json!([{"pointing_button":"button1"},{"key_code":"a"}]),
        json!([{"key_code":"a"},{"pointing_button":"button32"}]),
    ] {
        let body = json!([{"type":"basic","from":{"simultaneous":members,"modifiers":{"optional":["any"]}},"to":[{"key_code":"x"}]}]);
        let accepted = staged_compile(body.clone(), &BackendCapabilities::offline());
        assert!(accepted.executable_without_loss());
        let mapping = &accepted.snapshot.complex[0];
        let chord = mapping.extras.simultaneous.as_ref().unwrap();
        assert_eq!(mapping.from, chord.keys[0]);
        assert!(chord.validate().is_ok());
        for caps in [
            BackendCapabilities::windows_hook(),
            BackendCapabilities::linux(),
        ] {
            let rejected = staged_compile(body.clone(), &caps);
            assert!(!rejected.executable_without_loss());
            assert!(rejected.snapshot.complex.is_empty());
        }
    }
}
#[test]
fn staged_button_chords_keep_duplicate_page_and_exclusive_shape_rejections() {
    for from in [
        json!({"simultaneous":[{"pointing_button":"button1"},{"pointing_button":"button1"}]}),
        json!({"simultaneous":[{"pointing_button":"button33"},{"key_code":"a"}]}),
        json!({"simultaneous":[{"pointing_button":"button1"},{"key_code":"a"}],"pointing_button":"button2"}),
    ] {
        let result = staged_compile(
            json!([{"type":"basic","from":from,"to":[{"key_code":"x"}]}]),
            &BackendCapabilities::offline(),
        );
        assert!(!result.executable_without_loss());
        assert!(result.snapshot.complex.is_empty());
    }
    assert!(
        Simultaneous::new(vec![KeyId::new(BUTTON_PAGE, 0)])
            .validate()
            .is_err()
    );
    assert!(
        Simultaneous::new(vec![KeyId::new(BUTTON_PAGE, 33)])
            .validate()
            .is_err()
    );
    assert!(
        Simultaneous::new(vec![KeyId::new(CONSUMER_PAGE, 0)])
            .validate()
            .is_err()
    );
}

#[test]
fn staged_consumer_chords_preserve_page_and_require_consumer_capability() {
    for members in [
        json!([{"consumer_key_code":"mute"},{"key_code":"a"}]),
        json!([{"key_code":"a"},{"consumer_key_code":"mute"}]),
    ] {
        let body =
            json!([{"type":"basic","from":{"simultaneous":members},"to":[{"key_code":"x"}]}]);
        for caps in [BackendCapabilities::offline(), BackendCapabilities::linux()] {
            let result = staged_compile(body.clone(), &caps);
            assert!(result.executable_without_loss());
            let rule = &result.snapshot.complex[0];
            let chord = rule.extras.simultaneous.as_ref().unwrap();
            assert!(chord.keys.contains(&KeyId::new(CONSUMER_PAGE, 0xe2)));
            assert_eq!(rule.from, chord.keys[0]);
        }
        let result = staged_compile(body, &BackendCapabilities::windows_hook());
        assert!(!result.executable_without_loss());
        assert!(result.snapshot.complex.is_empty());
    }
    assert!(
        Simultaneous::new(vec![KeyId::new(CONSUMER_PAGE, 0)])
            .validate()
            .is_err()
    );
    assert!(
        Simultaneous::new(vec![KeyId::new(0xffff, 1)])
            .validate()
            .is_err()
    );
}
