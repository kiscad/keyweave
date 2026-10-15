#![allow(clippy::unwrap_used)]
use keyweave_engine::*;
use keyweave_types::*;
fn pattern(value: &str) -> Pattern {
    Pattern::compile(value).unwrap()
}
fn context() -> ContextSnapshot {
    ContextSnapshot {
        device_inventory_scope: DeviceInventoryScope::Complete,
        revision: 1,
        provider: "synthetic".into(),
        observed_at_ns: MonoTime(0),
        valid_until_ns: MonoTime(100),
        application: Some(ApplicationContext {
            bundle_identifier: Some("org.example.editor".into()),
            file_path: None,
        }),
        input_source: Some(InputSourceContext {
            language: Some("en".into()),
            input_source_id: Some("xkb.us".into()),
            input_mode_id: None,
        }),
        devices: None,
    }
}
fn evaluate(condition: &Condition, context: &ContextSnapshot, time: u64) -> Truth {
    condition.evaluate(
        context,
        DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        MonoTime(time),
    )
}
#[test]
fn app_search_is_or_across_fields_and_unknown_only_blocks_unresolved_results() {
    let mut context = context();
    let condition = Condition::Application {
        bundle_identifiers: vec![pattern("example\\.editor")],
        file_paths: vec![pattern("^/usr/bin/editor$")],
        unless: false,
    };
    assert_eq!(evaluate(&condition, &context, 0), Truth::True);
    context.application.as_mut().unwrap().bundle_identifier = Some("org.other".into());
    assert_eq!(evaluate(&condition, &context, 0), Truth::Unknown);
    context.application.as_mut().unwrap().file_path = Some("/usr/bin/editor".into());
    assert_eq!(evaluate(&condition, &context, 0), Truth::True);
    context.application.as_mut().unwrap().file_path = Some("/usr/bin/editor-plus".into());
    assert_eq!(evaluate(&condition, &context, 0), Truth::False);
}
#[test]
fn app_unless_does_not_turn_multiline_missing_or_expired_into_true() {
    let condition = Condition::Application {
        bundle_identifiers: vec![],
        file_paths: vec![pattern("editor")],
        unless: true,
    };
    let mut context = context();
    for path in [None, Some("/bin/other\n".into())] {
        context.application.as_mut().unwrap().file_path = path;
        assert_eq!(evaluate(&condition, &context, 0), Truth::Unknown);
    }
    context.application.as_mut().unwrap().file_path = Some("/bin/other".into());
    assert_eq!(evaluate(&condition, &context, 99), Truth::True);
    assert_eq!(evaluate(&condition, &context, 100), Truth::Unknown);
    context.application = None;
    assert_eq!(evaluate(&condition, &context, 0), Truth::Unknown);
}
#[test]
fn input_source_fields_are_and_specifiers_are_or_and_empty_requires_known_source() {
    let mut context = context();
    let condition = Condition::InputSource {
        sources: vec![
            InputSourceFilter {
                language: Some(pattern("^en$")),
                input_source_id: Some(pattern("^xkb\\.us$")),
                input_mode_id: Some(pattern("ascii")),
            },
            InputSourceFilter {
                language: Some(pattern("^zh$")),
                ..Default::default()
            },
        ],
        unless: false,
    };
    assert_eq!(evaluate(&condition, &context, 0), Truth::Unknown);
    context.input_source.as_mut().unwrap().language = Some("fr".into());
    assert_eq!(evaluate(&condition, &context, 0), Truth::False);
    context.input_source.as_mut().unwrap().language = Some("zh".into());
    assert_eq!(evaluate(&condition, &context, 0), Truth::True);
    let condition = Condition::InputSource {
        sources: vec![InputSourceFilter::default()],
        unless: false,
    };
    assert_eq!(evaluate(&condition, &context, 0), Truth::True);
    context.input_source = None;
    assert_eq!(evaluate(&condition, &context, 0), Truth::Unknown);
}
#[test]
fn unsupported_or_expensive_regex_is_rejected_before_replay() {
    for value in [
        "(?=editor)",
        "(a)\\1",
        "(?i)editor",
        "\\p{L}",
        "\\u0061",
        "\\x61",
        "[a&&b]",
        "[",
        "a{100000000}",
        "[[a]]",
        "[\\b]",
        "a++",
    ] {
        assert!(Pattern::compile(value).is_err(), "{value}");
    }
    assert!(Pattern::compile(&"a".repeat(1025)).is_err());
    for value in [
        "^org\\.example\\.(?:editor|terminal)$",
        "editor.*",
        "[A-Za-z0-9_-]+",
        "\\w+\\d*",
    ] {
        assert!(Pattern::compile(value).is_ok(), "{value}");
    }
}
#[test]
fn utf8_pattern_goldens_match_byte_ecmascript_search_for_app_and_input_source() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/regex/ecmascript-utf8.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let mut context = context();
        let text = case["value"].as_str().unwrap().to_owned();
        context.application.as_mut().unwrap().file_path = Some(text.clone());
        context.input_source.as_mut().unwrap().language = Some(text);
        let pattern = pattern(case["pattern"].as_str().unwrap());
        for unless in [false, true] {
            let expected = if case["matches"].as_bool().unwrap() != unless {
                Truth::True
            } else {
                Truth::False
            };
            for condition in [
                Condition::Application {
                    bundle_identifiers: vec![],
                    file_paths: vec![pattern.clone()],
                    unless,
                },
                Condition::InputSource {
                    sources: vec![InputSourceFilter {
                        language: Some(pattern.clone()),
                        ..Default::default()
                    }],
                    unless,
                },
            ] {
                assert_eq!(
                    evaluate(&condition, &context, 1),
                    expected,
                    "{}: {:?}",
                    case["id"],
                    condition
                );
            }
        }
    }
}
#[test]
fn utf8_unless_observes_real_nonmatch_but_never_missing_multiline_or_stale_data() {
    let condition = Condition::Application {
        bundle_identifiers: vec![],
        file_paths: vec![pattern("编辑器")],
        unless: true,
    };
    let mut context = context();
    context.application.as_mut().unwrap().file_path = Some("/home/用户/编辑器".into());
    assert_eq!(evaluate(&condition, &context, 1), Truth::False);
    context.application.as_mut().unwrap().file_path = Some("/home/用户/终端".into());
    assert_eq!(evaluate(&condition, &context, 1), Truth::True);
    assert_eq!(evaluate(&condition, &context, 100), Truth::Unknown);
    for value in [
        None,
        Some("/home/用户/终端\n".into()),
        Some("/home/用户/终端\r".into()),
    ] {
        context.application.as_mut().unwrap().file_path = value;
        assert_eq!(evaluate(&condition, &context, 1), Truth::Unknown);
    }
}
#[test]
fn utf8_byte_budget_and_unsupported_escape_diagnostics_remain_bounded() {
    assert!(Pattern::compile(&"é".repeat(512)).is_ok());
    assert!(Pattern::compile(&"é".repeat(513)).is_err());
    for value in [
        "编辑\n器",
        "编辑\r器",
        r"\中",
        r"\u4e2d",
        r"\xE4",
        r"(?i)编辑器",
        r"(?=编辑器)",
        r"[\w中]",
    ] {
        assert!(Pattern::compile(value).is_err(), "{value}");
    }
}
