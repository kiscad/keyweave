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
fn advanced_example_is_not_falsely_claimed_supported() {
    let result = cli(&[
        "validate",
        "--config",
        "examples/karabiner-basic.json",
        "--format",
        "json",
    ]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"valid\": false"));
}
#[test]
fn doctor_separates_platform_plan_from_implementation() {
    let result = cli(&["doctor"]);
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("\"hardware_validated\": false"));
}
