#![allow(clippy::unwrap_used)]
use keyweave_config::*;
use keyweave_engine::Engine;
use keyweave_types::*;
use serde_json::{Value, json};
use std::sync::Arc;

fn compile_value(value: Value) -> CompiledConfig {
    compile(
        &value.to_string(),
        &BackendCapabilities::offline(),
        ConfigGeneration(1),
    )
    .unwrap()
}
fn profile(rules: Value) -> Value {
    json!({"profiles":[{"selected":true,"complex_modifications":{"rules":rules}}]})
}
#[test]
fn jsonc_preserves_urls_escapes_unicode_and_error_lines() {
    assert_eq!(
        parse_jsonc(
            "{/* 中文 */\"url\":\"https://example.com/a/*b*/\",// note\n\"text\":\"\\\"//雪\"}"
        )
        .unwrap(),
        json!({"url":"https://example.com/a/*b*/","text":"\"//雪"})
    );
    let error = parse_jsonc("{\n/* one\n two */\n\"a\":invalid\n}").unwrap_err();
    match error {
        ConfigError::Json(error) => assert_eq!(error.line(), 4),
        other => panic!("{other}"),
    }
    assert!(matches!(
        parse_jsonc("{/* never closed"),
        Err(ConfigError::UnterminatedComment)
    ));
    assert!(parse_jsonc("{\"a\":1,}").is_err());
}
#[test]
fn disabled_javascript_is_preserved_and_never_executed() {
    let raw = profile(json!([{"enabled":false,"eval_js":"for (;;) {}"}]));
    let config = compile_value(raw.clone());
    assert!(config.executable_without_loss());
    assert_eq!(config.raw, raw);
    let enabled = compile_value(profile(json!([{"eval_js":"throw 'oops'"}])));
    assert_eq!(enabled.disabled_rules, 1);
    assert!(enabled.snapshot.complex.is_empty());
}
#[test]
fn unsupported_condition_disables_the_entire_outer_rule() {
    let config = compile_value(profile(json!([{"description":"example","manipulators":[
        {"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"b"}]},
        {"type":"basic","from":{"key_code":"c"},"to":[{"key_code":"d"}],"conditions":[{"type":"expression_if","expression":"x > 0"}]}
    ]}])));
    assert!(!config.executable_without_loss());
    assert!(config.snapshot.complex.is_empty());
    assert_eq!(
        config.diagnostics[0].pointer,
        "/profiles/0/complex_modifications/rules/0/manipulators/1/conditions"
    );
}
#[test]
fn metadata_round_trips_and_functional_unknowns_fail_closed() {
    let raw = json!({"maintainers":["example"],"profiles":[{"name":"雪","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":{"key_code":"b"}}]}]});
    let config = compile_value(raw.clone());
    assert_eq!(config.raw, raw);
    assert_eq!(config.snapshot.name, "雪");
    let config = compile_value(profile(
        json!([{"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"shell_command":"touch /tmp/MUST_NOT_RUN"}]}]}]),
    ));
    assert_eq!(config.disabled_rules, 1);
    assert!(config.snapshot.complex.is_empty());
}
#[test]
fn profile_selection_schema_size_and_nesting_are_checked() {
    assert!(
        compile(
            "{\"profiles\":[]}",
            &BackendCapabilities::offline(),
            ConfigGeneration(1)
        )
        .is_err()
    );
    assert!(
        compile(
            &json!({"profiles":[{"selected":true},{"selected":true}]}).to_string(),
            &BackendCapabilities::offline(),
            ConfigGeneration(1)
        )
        .is_err()
    );
    assert!(
        compile(
            &json!({"keyweave":{"schema_version":9},"profiles":[{}]}).to_string(),
            &BackendCapabilities::offline(),
            ConfigGeneration(1)
        )
        .is_err()
    );
    assert!(parse_jsonc(&" ".repeat(MAX_CONFIG_BYTES + 1)).is_err());
    assert!(parse_jsonc(&format!("{}0{}", "[".repeat(200), "]".repeat(200))).is_err());
}
#[test]
fn consumer_actions_are_rejected_on_hook_backend() {
    let raw = json!({"profiles":[{"simple_modifications":[{"from":{"key_code":"f10"},"to":[{"consumer_key_code":"mute"}]}]}]});
    let config = compile(
        &raw.to_string(),
        &BackendCapabilities::windows_hook(),
        ConfigGeneration(1),
    )
    .unwrap();
    assert_eq!(config.disabled_rules, 1);
    assert!(config.snapshot.simple.is_empty());
}

#[test]
fn multi_action_arrays_compile_but_cross_phase_expansion_is_bounded() {
    let config = compile_value(profile(json!([{"manipulators":[{
        "type":"basic","from":{"key_code":"a"},
        "to":[{"key_code":"b"},{"key_code":"c"}]
    }]}])));
    assert!(config.executable_without_loss());
    assert_eq!(config.snapshot.complex[0].to.len(), 2);
    let actions = vec![json!({"key_code":"b"}); 256];
    let config = compile_value(json!({"profiles":[{
        "simple_modifications":[{"from":{"key_code":"a"},"to":actions}],
        "complex_modifications":{"rules":[{"manipulators":[{
            "type":"basic","from":{"key_code":"b"},"to":vec![json!({"key_code":"c"}); 256]
        }]}]}
    }]}));
    assert!(!config.executable_without_loss());
    assert_eq!(config.diagnostics[0].code, "KW-LIMIT-001");
    assert_eq!(
        config.diagnostics[0].pointer,
        "/profiles/0/simple_modifications/0/to"
    );
}

#[test]
fn functional_fields_at_every_executable_level_refuse_activation() {
    let cases = [
        (
            json!({"profiles":[{}],"global":{"unsafe":"ignored"}}),
            "/global",
        ),
        (
            json!({"profiles":[{"complex_modification":{}}]}),
            "/profiles/0/complex_modification",
        ),
        (
            json!({"profiles":[{"parameters":{"delay_milliseconds_before_open_device":1000}}]}),
            "/profiles/0/parameters/delay_milliseconds_before_open_device",
        ),
        (
            json!({"profiles":[{"virtual_hid_keyboard":{"keyboard_type_v2":"iso"}}]}),
            "/profiles/0/virtual_hid_keyboard/keyboard_type_v2",
        ),
        (
            json!({"profiles":[{"ignore_pointing_device_events_by_default":false}]}),
            "/profiles/0/ignore_pointing_device_events_by_default",
        ),
        (
            json!({"profiles":[{"complex_modifications":{"unexpected/rule~":1}}]}),
            "/profiles/0/complex_modifications/unexpected~1rule~0",
        ),
        (
            json!({"profiles":[{"complex_modifications":{"parameters":{"basic.unsupported_timeout_milliseconds":100}}}]}),
            "/profiles/0/complex_modifications/parameters/basic.unsupported_timeout_milliseconds",
        ),
        (
            profile(
                json!([{"conditions":[],"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":{"key_code":"b"}}]}]),
            ),
            "/profiles/0/complex_modifications/rules/0/conditions",
        ),
    ];
    for (value, pointer) in cases {
        let config = compile_value(value.clone());
        assert!(!config.executable_without_loss(), "{pointer}");
        assert_eq!(config.raw, value);
        assert_eq!(config.diagnostics[0].pointer, pointer);
    }
}

#[test]
fn inactive_rule_and_explicit_metadata_are_preserved_without_execution() {
    let raw = profile(json!([{"enabled":false,"unknown_future_action":{"execute":"NEVER"}}]));
    assert!(compile_value(raw).executable_without_loss());
    let raw = json!({"metadata":{"future_custom_data":42}, "profiles":[{
        "name":"Metadata", "metadata":{"maintainer_note":"anything"},
        "complex_modifications":{"metadata":{"custom":true},"rules":[{
            "description":"test", "metadata":{"custom":1},
            "manipulators":[{"type":"basic","from":{"key_code":"a"},"to":{"key_code":"b"}}]
        }]}
    }]});
    let config = compile_value(raw.clone());
    assert!(config.executable_without_loss());
    assert_eq!(config.raw, raw);
}

#[test]
fn malformed_profiles_sections_and_descriptions_are_not_ignored() {
    for raw in [
        json!({"profiles":[{"selected":true}, false]}),
        json!({"profiles":[{"selected":true}, {"selected":"yes"}]}),
        json!({"profiles":[{"selected":true}, {"name":123}]}),
        json!({"profiles":[{"devices":{}}]}),
        json!({"profiles":[{"fn_function_keys":null}]}),
        json!({"profiles":[{"parameters":[]}]}),
        json!({"profiles":[{"virtual_hid_keyboard":"ansi"}]}),
        json!({"profiles":[{"ignore_pointing_device_events_by_default":"no"}]}),
        profile(json!([{"description":123,"manipulators":[]}])),
    ] {
        assert!(
            compile(
                &raw.to_string(),
                &BackendCapabilities::offline(),
                ConfigGeneration(1)
            )
            .is_err(),
            "{raw}"
        );
    }
}

#[test]
fn duplicate_json_members_are_rejected_including_escaped_equivalents() {
    for text in [
        r#"{"profiles":[],"profiles":[{}]}"#,
        r#"{"profiles":[{"selected":true,"selected":false}]}"#,
        r#"{"a":1,"\u0061":2}"#,
        r#"{"metadata":{"a":1,"a":2}}"#,
    ] {
        let error = parse_jsonc(text).unwrap_err();
        assert!(
            error.to_string().contains("duplicate JSON member"),
            "{error}"
        );
    }
    assert_eq!(
        parse_jsonc(
            r#"[null,true,-9223372036854775808,18446744073709551615,1.25,"雪",{"a":1},{"a":2}]"#
        )
        .unwrap(),
        json!([null,true,i64::MIN,u64::MAX,1.25,"雪",{"a":1},{"a":2}])
    );
}

#[test]
fn alone_timeout_inherits_global_value_and_manipulator_override() {
    let config = compile_value(json!({"profiles":[{"complex_modifications":{
        "parameters":{"basic.to_if_alone_timeout_milliseconds":123,"basic.to_if_held_down_threshold_milliseconds":500},
        "rules":[{"manipulators":[
            {"type":"basic","from":{"key_code":"caps_lock","modifiers":{"optional":["any"]}},"to":{"key_code":"left_control"},"to_if_alone":{"key_code":"escape"}},
            {"type":"basic","from":{"key_code":"a"},"to":[],"to_after_key_up":{"key_code":"b"},"parameters":{"basic.to_if_alone_timeout_milliseconds":0}}
        ]}]
    }}]}));
    assert!(config.executable_without_loss());
    assert_eq!(
        config.snapshot.complex[0].extras.alone_timeout_ns,
        123_000_000
    );
    assert_eq!(config.snapshot.complex[1].extras.alone_timeout_ns, 0);
    assert!(config.snapshot.complex[0].allow_extra_modifiers);
    assert!(!config.snapshot.complex[1].allow_extra_modifiers);
    assert!(config.snapshot.complex[1].to.is_empty());
    assert_eq!(config.snapshot.complex[1].extras.after_key_up.len(), 1);
}

#[test]
fn malformed_or_unsupported_new_action_options_disable_whole_rule() {
    for value in [
        json!({"type":"basic","from":{"key_code":"a","modifiers":{"mandatory":["control"]}},"to":{"key_code":"b"}}),
        json!({"type":"basic","from":{"key_code":"a","modifiers":{"optional":["any","shift"]}},"to":{"key_code":"b"}}),
        json!({"type":"basic","from":{"key_code":"a"},"to_if_alone":[{"key_code":"b","repeat":false}]}),
        json!({"type":"basic","from":{"key_code":"a"},"to_after_key_up":false}),
        json!({"type":"basic","from":{"key_code":"a"},"to":{"key_code":"b"},"parameters":{"basic.to_if_alone_timeout_milliseconds":-1}}),
        json!({"type":"basic","from":{"key_code":"a"},"to":{"key_code":"b"},"parameters":{"basic.to_if_alone_timeout_milliseconds":0.5}}),
        json!({"type":"basic","from":{"key_code":"a"},"to":{"key_code":"b"},"parameters":{"basic.to_if_alone_timeout_milliseconds":86400001}}),
    ] {
        let config = compile_value(profile(json!([{"manipulators":[
            {"type":"basic","from":{"key_code":"c"},"to":{"key_code":"d"}},value
        ]}])));
        assert!(!config.executable_without_loss());
        assert!(config.snapshot.complex.is_empty());
        assert_eq!(config.disabled_rules, 1);
    }
}

#[test]
fn pure_alone_and_after_actions_require_no_normal_to_action() {
    let config = compile_value(profile(json!([{"manipulators":[
        {"type":"basic","from":{"key_code":"a"},"to_if_alone":{"key_code":"escape"}},
        {"type":"basic","from":{"key_code":"b"},"to_after_key_up":[{"key_code":"c"}]}
    ]}])));
    assert!(config.executable_without_loss());
    assert!(
        config
            .snapshot
            .complex
            .iter()
            .all(|mapping| mapping.to.is_empty())
    );
    let suppression = compile_value(profile(
        json!([{"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[]}]}]),
    ));
    assert!(suppression.executable_without_loss());
}
#[test]
fn upstream_spacebar_to_tab_matches_key_edges_without_assuming_mach_units() {
    let rules: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/upstream/rules/spacebar_to_tab.json"
    ))
    .unwrap();
    let config = compile_value(profile(json!([{"manipulators":rules}])));
    assert!(config.executable_without_loss());
    let input: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/upstream/input_event_queue/spacebar_to_tab.json"
    ))
    .unwrap();
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/upstream/expected_event_queue/spacebar_to_tab.json"
    ))
    .unwrap();
    let mut engine = Engine::new(Arc::new(config.snapshot));
    let mut actual = Vec::new();
    for (i, item) in input.iter().enumerate() {
        let key = item["event"]["momentary_switch_event"]["key_code"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let edge = if item["event_type"] == "key_down" {
            KeyEdge::Down
        } else {
            KeyEdge::Up
        };
        let step = engine
            .handle(InputEnvelope {
                seq: i as u64 + 1,
                at_ns: MonoTime(i as u64),
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                provenance: Provenance::Physical,
                event: InputEvent::Key { key, edge },
            })
            .unwrap();
        actual.extend(step.outputs.into_iter().map(|o| (o.key, o.edge)));
    }
    let expected: Vec<_> = expected
        .iter()
        .map(|v| {
            (
                v["event"]["momentary_switch_event"]["key_code"]
                    .as_str()
                    .unwrap()
                    .parse::<KeyId>()
                    .unwrap(),
                if v["event_type"] == "key_down" {
                    KeyEdge::Down
                } else {
                    KeyEdge::Up
                },
            )
        })
        .collect();
    assert_eq!(actual, expected);
    assert!(engine.state().pressed_outputs.is_empty());
}
