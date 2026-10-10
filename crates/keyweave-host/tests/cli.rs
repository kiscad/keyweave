//! Offline CLI and metadata only; never run a valid capture worker.
#![allow(clippy::unwrap_used)]
use std::{fs, process::Command};
fn dir(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("keyweave-host-test-{}-{name}", std::process::id()));
    fs::create_dir(&p).unwrap();
    p
}
#[test]
fn check_preserves_private_body_and_reports_only_metadata() {
    let root = dir("check");
    let path = root.join("config with spaces.jsonc");
    fs::write(&path,br#"// synthetic private comment
{"profiles":[{"name":"private synthetic profile","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args(["check-windows", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("generation=1"));
    assert!(!text.contains("private"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn check_rejects_native_special_output_and_invalid_utf8_without_body() {
    let root = dir("rejection");
    let path = root.join("config.json");
    for bytes in [br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"print_screen"}]}]}]}"#.as_slice(),b"private-secret\xff"] {
        fs::write(&path,bytes).unwrap();let output=Command::new(env!("CARGO_BIN_EXE_keyweave-host")).args(["check-windows","--config"]).arg(&path).output().unwrap();
        assert!(!output.status.success());assert!(output.stdout.is_empty());assert!(!String::from_utf8(output.stderr).unwrap().contains("private-secret"));
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn run_requires_explicit_identity_and_no_default_capture() {
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args(["run-windows", "--config", "unused.json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}
#[cfg(not(windows))]
#[test]
fn linux_run_is_explicitly_unavailable() {
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args([
            "run-windows",
            "--config",
            "unused.json",
            "--identity",
            "strict",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(78));
}
#[test]
fn locator_requires_existing_file_and_explicit_absolute_choice() {
    let root = dir("locate");
    let host = root.join("host.exe");
    fs::write(&host, b"metadata only").unwrap();
    assert!(keyweave_host::locate_worker(&host, None).is_err());
    let worker = root.join("keyweave-worker.exe");
    fs::write(&worker, b"metadata only").unwrap();
    assert_eq!(
        keyweave_host::locate_worker(&host, None).unwrap(),
        worker.canonicalize().unwrap()
    );
    assert!(
        keyweave_host::locate_worker(&host, Some(std::path::Path::new("keyweave-worker.exe")))
            .is_err()
    );
    let folder = root.join("folder.exe");
    fs::create_dir(&folder).unwrap();
    assert!(keyweave_host::locate_worker(&host, Some(&folder)).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[cfg(unix)]
#[test]
fn packaged_sibling_cannot_resolve_outside_host_directory() {
    let root = dir("escape");
    let outside = root.join("other");
    fs::create_dir(&outside).unwrap();
    let host = root.join("host");
    fs::write(&host, b"metadata").unwrap();
    let target = outside.join("worker");
    fs::write(&target, b"metadata").unwrap();
    std::os::unix::fs::symlink(&target, root.join("keyweave-worker.exe")).unwrap();
    assert!(keyweave_host::locate_worker(&host, None).is_err());
    assert_eq!(
        keyweave_host::locate_worker(&host, Some(&target)).unwrap(),
        target.canonicalize().unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn linux_check_uses_linux_output_encoding_and_reports_offline_scope() {
    let root = dir("linux-check");
    let path = root.join("Linux config with spaces.jsonc");
    fs::write(&path, br#"// private comment
{"profiles":[{"name":"private profile","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"print_screen"}]}]}]}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args(["check-linux", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("offline")
            && text.contains("generation=1")
            && text.contains("readiness unverified")
    );
    assert!(!text.contains("private"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn linux_check_rejects_invalid_utf8_and_loss_without_activation() {
    let root = dir("linux-reject");
    let path = root.join("config.json");
    for bytes in [b"private-secret\xff".as_slice(),br#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"shell_command":"never execute"}]}]}]}}]}"#.as_slice()] {
        fs::write(&path, bytes).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
            .args(["check-linux", "--config"]).arg(&path).output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8(output.stderr).unwrap().contains("private-secret"));
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_run_rejects_invalid_scope_and_arguments_before_spawn() {
    for extra in [
        vec!["--session-provider", "console"],
        vec!["--session-provider", "none", "--vt", "/dev/tty1"],
        vec!["--session-provider", "console", "--vt", "/dev/tty01"],
        vec!["--session-provider", "none", "--input", "relative"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
            .args([
                "run-linux",
                "--config",
                "/keyweave-absent-config",
                "--worker",
                "/keyweave-absent-worker",
                "--input",
                "/keyweave-absent-input",
            ])
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let text = String::from_utf8(output.stderr).unwrap();
        assert!(!text.contains("worker_exit="));
        assert!(text.contains("--vt") || text.contains("Arguments"));
    }
}
#[cfg(target_os = "linux")]
#[test]
fn linux_run_bad_config_stops_before_launching_selected_executable() {
    let root = dir("linux-run-invalid");
    let config = root.join("private config.json");
    fs::write(&config, b"private-secret\xff").unwrap();
    // Host image is executable but must never be launched: config rejection
    // precedes signal install/spawn. None is permanent no-device scope.
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args([
            "run-linux",
            "--session-provider",
            "none",
            "--input",
            "/keyweave-absent-input",
            "--worker",
        ])
        .arg(env!("CARGO_BIN_EXE_keyweave-host"))
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("Configuration"));
    assert!(!text.contains("private-secret"));
    assert!(!text.contains("private config"));
    fs::remove_dir_all(root).unwrap();
}
