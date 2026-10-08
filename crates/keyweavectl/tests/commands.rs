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
fn doctor_separates_platform_plan_from_implementation() {
    let result = cli(&["doctor"]);
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"hardware_validated\": false"));
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
