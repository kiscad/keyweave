//! No valid display/node queries; executable argument and fail-closed scope only.
use std::process::Command;
#[test]
fn invalid_remote_display_is_rejected_without_private_input_or_success_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-x11-probe"))
        .args([
            "--display",
            "remote-private-host:0",
            "--node",
            "/dev/input/event42",
            "--major",
            "13",
            "--minor",
            "106",
        ])
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("remote-private-host"));
}
#[test]
fn missing_arguments_never_start_a_probe() {
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-x11-probe"))
        .output()
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
