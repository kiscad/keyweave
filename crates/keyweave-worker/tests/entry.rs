//! Real executable rejection paths only. Never install a Hook or inject keys.
#![allow(clippy::unwrap_used)]
use keyweave_worker::{WORKER_ARGUMENT, WorkerExit};
use std::{ffi::OsString, process::Command};
fn extra(config: OsString) -> Vec<OsString> {
    vec![
        "--config".into(),
        config,
        "--tag".into(),
        "42".into(),
        "--identity".into(),
        "strict".into(),
    ]
}
#[test]
fn malformed_entry_exits_without_protocol_or_diagnostic_output() {
    for args in [
        vec![],
        vec![OsString::from("--help")],
        vec![OsString::from(WORKER_ARGUMENT)],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_keyweave-worker"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(WorkerExit::Arguments as i32));
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }
}
#[test]
fn direct_valid_worker_invocation_cannot_bypass_native_admission() {
    let path = std::env::temp_dir().join("keyweave-direct-no-config.json");
    let result = Command::new(env!("CARGO_BIN_EXE_keyweave-worker"))
        .args([WORKER_ARGUMENT, "1", "1"])
        .args(extra(path.into()))
        .output()
        .unwrap();
    let expected = if cfg!(windows) {
        WorkerExit::Admission
    } else {
        WorkerExit::UnsupportedPlatform
    };
    assert_eq!(result.status.code(), Some(expected as i32));
    assert!(result.stdout.is_empty());
    assert!(result.stderr.is_empty());
}
#[cfg(windows)]
#[test]
fn missing_config_after_owned_admission_exits_before_input_installation() {
    use keyweave_types::MonoTime;
    use keyweave_windows::{Supervisor, SupervisorStatus};
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    // Unique absent parent directory: the worker cannot accidentally load a
    // valid config and start capture. This test never creates input resources.
    let absent = std::env::temp_dir().join(format!(
        "keyweave-absent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!absent.exists());
    let start = Instant::now();
    let mut supervisor = Supervisor::spawn(
        Path::new(env!("CARGO_BIN_EXE_keyweave-worker")),
        &extra(absent.join("config.json").into()),
        73,
        keyweave_progress::Policy::default(),
        MonoTime(0),
    )
    .unwrap();
    let status = loop {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "owned worker failed to exit"
        );
        let status = supervisor
            .poll(MonoTime(u64::try_from(start.elapsed().as_nanos()).unwrap()))
            .unwrap();
        if matches!(status, SupervisorStatus::Exited { .. }) {
            break status;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    // Config code proves real main admission succeeded; admission/lease failure
    // cannot masquerade as this path. No reply epoch was manufactured.
    let SupervisorStatus::Exited { code, reason } = status else {
        unreachable!()
    };
    assert_eq!(code, WorkerExit::Config as u32);
    // Std handles can close before the process object becomes signaled. Keep
    // that independently observed channel error; it is not admission failure.
    if let Some(reason) = reason {
        let keyweave_windows::TerminationReason::Channel(failure) = reason else {
            panic!("unexpected terminal reason: {reason:?}");
        };
        assert!(matches!(
            failure.kind,
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::BrokenPipe
        ));
    }
    assert_eq!(supervisor.progress_epoch(), 0);
    assert!(supervisor.close_channels().unwrap().complete());
}
