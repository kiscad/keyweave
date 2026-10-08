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
