#![allow(clippy::unwrap_used)]
use std::process::Command;
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_keyweavectl"))
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("CLI process failed: {e}"))
}
#[test]
fn replay_assertion_uses_real_binary() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/basic.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("offline_synthetic_no_device_access"));
}
#[test]
fn tap_alone_replay_asserts_strict_threshold_and_wheel_cancellation() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/tap-alone.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"asserted\": true"));
}
#[test]
fn mandatory_modifier_replay_preserves_lazy_restoration() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/modifiers.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn output_shortcuts_replay_includes_modifier_scopes_for_up_macros() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/output-shortcuts.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn caps_tap_alone_example_validates_without_accessing_hardware() {
    let result = cli(&[
        "validate",
        "--config",
        "examples/karabiner-basic.json",
        "--format",
        "json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"valid\": true"));
}
#[test]
fn doctor_separates_backend_code_from_native_acceptance() {
    let result = cli(&["doctor"]);
    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["hardware_validated"], false);
    assert_eq!(report["platform_backends_implemented"], true);
    assert_eq!(report["native_acceptance"], "unverified");
}
#[test]
fn device_context_replay_preserves_old_up_after_expiry() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/device-context.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"asserted\": true"));
}
#[test]
fn text_context_replay_uses_live_up_action_conditions() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/text-context.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains("synthetic-text-fixture"));
}
#[test]
fn variable_replay_asserts_values_without_exporting_them() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/variables.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(!output.contains("do-not-export"));
    assert!(!output.contains("secret-fixture-value"));
}
#[test]
fn held_replay_uses_explicit_deadline_and_ignores_stale_callback() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/held.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn delayed_replay_asserts_invoked_canceled_and_pending_lifetimes() {
    for fixture in [
        "tests/fixtures/delayed.json",
        "tests/fixtures/delayed-pending.json",
    ] {
        let result = cli(&["replay", "--fixture", fixture, "--assert"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if fixture.contains("pending") {
            let value: serde_json::Value = serde_json::from_slice(&result.stdout)
                .unwrap_or_else(|error| panic!("invalid CLI JSON: {error}"));
            assert_eq!(value["state"]["pending_bindings"], 1);
            assert_eq!(
                value["next_deadline"],
                serde_json::json!({"at_ns":5000000,"token":2})
            );
        }
    }
}

#[test]
fn lazy_and_halt_replay_preserves_current_batch_and_pending_modifiers() {
    let result = cli(&[
        "replay",
        "--fixture",
        "tests/fixtures/lazy-halt.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn device_inventory_scope_legacy_and_partial_json_roundtrip_is_explicit() {
    let original = serde_json::json!({"revision":1,"provider":"synthetic","observed_at_ns":0,"valid_until_ns":100,"devices":[],"application":null,"input_source":null});
    let mut snapshot: keyweave_types::ContextSnapshot = serde_json::from_value(original.clone())
        .unwrap_or_else(|error| panic!("context JSON: {error}"));
    assert_eq!(
        snapshot.device_inventory_scope,
        keyweave_types::DeviceInventoryScope::Complete
    );
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap_or_else(|error| panic!("context JSON: {error}")),
        original
    );
    snapshot.device_inventory_scope = keyweave_types::DeviceInventoryScope::Partial;
    let mut partial =
        serde_json::to_value(&snapshot).unwrap_or_else(|error| panic!("context JSON: {error}"));
    assert_eq!(partial["device_inventory_scope"], "partial");
    assert_eq!(
        serde_json::from_value::<keyweave_types::ContextSnapshot>(partial.clone())
            .unwrap_or_else(|error| panic!("context JSON: {error}")),
        snapshot
    );
    partial["device_inventory_scope"] = serde_json::json!("assumed_complete");
    assert!(serde_json::from_value::<keyweave_types::ContextSnapshot>(partial).is_err());
}

#[test]
fn staged_replay_uses_actual_chord_controller_and_repeat_query() {
    let result = cli(&[
        "replay",
        "--execution-mode",
        "staged-keyboard",
        "--fixture",
        "tests/fixtures/staged-chord.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["execution_mode"], "staged-keyboard");
    assert_eq!(
        result["repeat_targets"],
        serde_json::json!([{"seq":3,"at_ns":115,"key":"c"}])
    );
    assert!(result["next_deadline"].is_null());
}
#[test]
fn staged_replay_honors_endpoint_timeout_and_virtual_clock_without_timer_inputs() {
    for name in ["staged-chord-endpoint", "staged-chord-timeout"] {
        let fixture = format!("tests/fixtures/{name}.json");
        let result = cli(&[
            "replay",
            "--execution-mode",
            "staged-keyboard",
            "--fixture",
            &fixture,
            "--assert",
        ]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
#[test]
fn staged_replay_retains_old_duration_snapshot_across_explicit_activation() {
    let result = cli(&[
        "replay",
        "--execution-mode",
        "staged-keyboard",
        "--fixture",
        "tests/fixtures/staged-duration-reload.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn staged_replay_device_control_cancels_actual_group_without_after_up() {
    let result = cli(&[
        "replay",
        "--execution-mode",
        "staged-keyboard",
        "--fixture",
        "tests/fixtures/staged-chord-remove.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn staged_replay_preserves_pointer_receipts_and_actual_output_variables() {
    let result = cli(&[
        "replay",
        "--execution-mode",
        "staged-keyboard",
        "--fixture",
        "tests/fixtures/staged-pointer-variable.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn staged_validation_and_import_keep_explicit_mode_and_default_rejection() {
    let legacy = cli(&["validate", "--config", "examples/staged-chord.json"]);
    assert!(!legacy.status.success());
    let staged = cli(&[
        "validate",
        "--execution-mode",
        "staged-keyboard",
        "--config",
        "examples/staged-chord.json",
        "--format",
        "json",
    ]);
    assert!(
        staged.status.success(),
        "{}",
        String::from_utf8_lossy(&staged.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&staged.stdout).unwrap();
    assert_eq!(result["execution_mode"], "staged-keyboard");
    let root = std::env::temp_dir().join(format!("keyweave-staged-import-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let output = root.join("copy.json");
    let report = root.join("report.json");
    let result = cli(&[
        "import-karabiner",
        "--execution-mode",
        "staged-keyboard",
        "--input",
        "examples/staged-chord.json",
        "--output",
        output.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert_eq!(result["activation"], "never");
    assert_eq!(result["execution_mode"], "staged-keyboard");
    assert!(output.is_file());
    std::fs::remove_dir_all(root).unwrap();
}

fn with_staged_fixture(
    name: &str,
    fixture: &serde_json::Value,
    assert_expected: bool,
) -> std::process::Output {
    let root = std::env::temp_dir().join(format!("keyweave-staged-{name}-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("fixture.json");
    std::fs::write(&path, serde_json::to_vec(fixture).unwrap()).unwrap();
    let mut args = vec![
        "replay",
        "--execution-mode",
        "staged-keyboard",
        "--fixture",
        path.to_str().unwrap(),
    ];
    if assert_expected {
        args.push("--assert");
    }
    let result = cli(&args);
    std::fs::remove_dir_all(root).unwrap();
    result
}
#[test]
fn staged_replay_reports_pending_window_without_inventing_eof_completion() {
    let mut fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/staged-chord.json"
        ))
        .unwrap(),
    )
    .unwrap();
    fixture["steps"].as_array_mut().unwrap().truncate(1);
    let result = with_staged_fixture("pending", &fixture, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["outputs"], serde_json::json!([]));
    assert_eq!(result["state"]["bindings"], 1);
    assert_eq!(result["next_deadline"]["at_ns"], 50_000_101);
}
#[test]
fn staged_replay_rejects_schema_clock_generation_timer_and_sequence_errors() {
    use serde_json::json;
    let fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/staged-chord.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let mut cases = vec![];
    let mut bad = fixture.clone();
    bad["schema_version"] = json!(1);
    cases.push(bad);
    let mut bad = fixture.clone();
    bad["inputs"] = json!([bad["steps"][0]["input"].clone()]);
    cases.push(bad);
    let mut bad = fixture.clone();
    bad["steps"] = json!([{"type":"advance","at_ns":100},{"type":"advance","at_ns":99}]);
    cases.push(bad);
    let mut bad = fixture.clone();
    bad["steps"] =
        json!([{"type":"activate","at_ns":0,"generation":1,"config":bad["config"].clone()}]);
    cases.push(bad);
    let mut bad = fixture.clone();
    bad["steps"][0]["input"]["event"] = json!({"type":"timer_fired","token":1});
    cases.push(bad);
    let mut bad = fixture.clone();
    bad["steps"][1]["input"]["seq"] = json!(1);
    cases.push(bad);
    let mut bad = fixture.clone();
    bad["steps"][0]["unknown"] = json!(true);
    cases.push(bad);
    for (index, bad) in cases.iter().enumerate() {
        let result = with_staged_fixture(&format!("invalid-{index}"), bad, false);
        assert!(!result.status.success(), "bad case {index} accepted");
        assert!(result.stdout.is_empty());
    }
    let result = cli(&["replay", "--fixture", "tests/fixtures/staged-chord.json"]);
    assert!(!result.status.success());
}
#[test]
fn staged_replay_assertions_use_actual_outputs_and_schema_one_remains_available() {
    let mut fixture: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/staged-chord.json"
        ))
        .unwrap(),
    )
    .unwrap();
    fixture["expected"][0]["key"] = serde_json::json!("z");
    let result = with_staged_fixture("mismatch", &fixture, true);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("replay mismatch"));
    let result = cli(&[
        "replay",
        "--execution-mode",
        "staged-keyboard",
        "--fixture",
        "tests/fixtures/basic.json",
        "--assert",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
