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
#[cfg(windows)]
#[test]
fn inherited_invalid_config_exits_after_file_admission_before_input() {
    use keyweave_windows::{ConfigArtifact, OwnedJobProcess, PipeReader};
    use std::time::{Duration, Instant};
    for bytes in [&b"\xff"[..], &b"{synthetic invalid JSON"[..]] {
        let config = ConfigArtifact::create(&std::env::temp_dir(), bytes).unwrap();
        let path = config.path().unwrap().to_path_buf();
        let (reader, mut parent_writer) = PipeReader::pair().unwrap();
        let (mut parent_reader, writer) = PipeReader::pair().unwrap();
        let mut args: Vec<OsString> = vec![
            WORKER_ARGUMENT.into(),
            std::process::id().to_string().into(),
            "42".into(),
        ];
        // This path is diagnostic only and is deliberately different from the
        // handed-off artifact. Both images always fail before capture install.
        let absent = std::env::temp_dir().join(format!(
            "keyweave-handoff-absent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!absent.exists());
        args.extend(extra(absent.join("config.jsonc").into()));
        let mut child = OwnedJobProcess::spawn_suspended_with_config(
            std::path::Path::new(env!("CARGO_BIN_EXE_keyweave-worker")),
            &args,
            reader,
            writer,
            config,
        )
        .unwrap();
        child.resume().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let code = loop {
            assert!(Instant::now() < deadline, "config worker exit timeout");
            if let Some(code) = child.poll_exit(Duration::from_millis(20)).unwrap() {
                break code;
            }
        };
        assert_eq!(code, WorkerExit::Config as u32);
        assert!(child.close_config().unwrap().unwrap().complete());
        assert!(parent_reader.close().unwrap().complete());
        assert!(parent_writer.close().unwrap().complete());
        let deletion_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => panic!("deletion observation failed: {error}"),
                Ok(_) => {
                    assert!(
                        Instant::now() < deletion_deadline,
                        "config deletion timeout"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }
}
