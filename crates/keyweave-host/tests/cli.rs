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

#[test]
fn watch_options_require_explicit_enablement_and_bounded_interval_before_run() {
    for flags in [
        vec!["--watch-interval-ms", "1000"],
        vec!["--watch-config", "--watch-interval-ms", "99"],
        vec!["--watch-config", "--watch-interval-ms", "60001"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
            .args([
                "run-linux",
                "--config",
                "/absent-config",
                "--input",
                "/absent-input",
                "--session-provider",
                "none",
            ])
            .args(flags)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(
            !String::from_utf8(output.stderr)
                .unwrap()
                .contains("worker_exit=")
        );
    }
}
#[cfg(target_os = "linux")]
#[test]
fn watch_none_rejected_before_config_and_worker_lookup() {
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args([
            "run-linux",
            "--config",
            "/absent-config",
            "--worker",
            "/absent-worker",
            "--input",
            "/absent-input",
            "--session-provider",
            "none",
            "--watch-config",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("Arguments"));
    assert!(!text.contains("worker_exit="));
}
#[cfg(target_os = "linux")]
#[test]
fn watch_console_invalid_config_stops_before_any_signal_or_child_admission() {
    let root = dir("watch-invalid");
    let config = root.join("private config.json");
    fs::write(&config, b"private-secret\xff").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args([
            "run-linux",
            "--session-provider",
            "console",
            "--vt",
            "/dev/tty2",
            "--input",
            "/absent-input",
            "--watch-config",
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
    assert!(!text.contains("worker_exit="));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn linux_offline_mode_explicitly_admits_timed_json_and_reports_unverified_readiness() {
    let root = dir("staged-offline");
    let path = root.join("private config.jsonc");
    let body = br#"// private-duration-comment
{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"description":"private-duration-rule","manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"b","hold_down_milliseconds":50},{"key_code":"c"}]}]}]}}]}"#;
    fs::write(&path, body).unwrap();
    let legacy = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args(["check-linux", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!legacy.status.success());
    let staged = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args([
            "check-linux",
            "--execution-mode",
            "staged-keyboard",
            "--config",
        ])
        .arg(&path)
        .output()
        .unwrap();
    assert!(staged.status.success());
    assert!(staged.stderr.is_empty());
    let text = String::from_utf8(staged.stdout).unwrap();
    assert!(text.contains("mode=StagedKeyboard") && text.contains("device readiness unverified"));
    assert!(!text.contains("private-duration"));
    assert_eq!(fs::read(&path).unwrap(), body);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn windows_offline_mode_admits_timing_only_when_explicit_and_preserves_private_bytes() {
    let root = dir("windows-staged-offline");
    let path = root.join("private config with spaces.jsonc");
    let body = br#"// private-timed-comment
{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"description":"private-timed-rule","manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"b","hold_down_milliseconds":50},{"key_code":"c"}]}]}]}}]}"#;
    fs::write(&path, body).unwrap();
    for mode in [
        None,
        Some("legacy"),
        Some("staged-keyboard"),
        Some("unknown"),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_keyweave-host"));
        command.arg("check-windows").arg("--config").arg(&path);
        if let Some(mode) = mode {
            command.args(["--execution-mode", mode]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.success(), mode == Some("staged-keyboard"));
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stdout.contains("private-timed") && !stderr.contains("private-timed"));
        if mode == Some("staged-keyboard") {
            assert!(stderr.is_empty());
            assert!(stdout.contains("offline") && stdout.contains("mode=StagedKeyboard"));
            assert!(stdout.contains("device readiness unverified"));
        } else {
            assert!(stdout.is_empty());
        }
    }
    assert_eq!(fs::read(&path).unwrap(), body);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(not(windows))]
#[test]
fn windows_staged_run_remains_unavailable_on_non_windows_before_file_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
        .args([
            "run-windows",
            "--execution-mode",
            "staged-keyboard",
            "--config",
            "absent-private-config.json",
            "--identity",
            "strict",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(78));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "run-windows requires Windows"
    );
}

#[test]
fn parsed_chord_offline_admission_requires_explicit_staged_mode_without_private_body() {
    let root = dir("simultaneous-gate");
    let path = root.join("config.jsonc");
    let body=br#"// private-chord-comment
{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"description":"private-chord-description","manipulators":[{"type":"basic","from":{"simultaneous":[{"key_code":"a"},{"key_code":"b"}],"simultaneous_options":{"description":"private-chord-options","key_up_when":"all"}},"to":[{"key_code":"x"}]}]}]}}]}"#;
    fs::write(&path, body).unwrap();
    for platform in ["check-windows", "check-linux"] {
        for mode in ["legacy", "staged-keyboard"] {
            let output = Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
                .args([platform, "--execution-mode", mode, "--config"])
                .arg(&path)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if mode == "legacy" { 1 } else { 0 })
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(!stdout.contains("private-chord"));
            assert!(!stderr.contains("private-chord"));
            if mode == "legacy" {
                assert!(stdout.is_empty());
            } else {
                assert!(stderr.is_empty());
                assert!(stdout.contains("StagedKeyboard"));
                assert!(stdout.contains("readiness unverified"));
            }
        }
    }
    assert_eq!(fs::read(&path).unwrap(), body);
    fs::remove_dir_all(root).unwrap();
}
