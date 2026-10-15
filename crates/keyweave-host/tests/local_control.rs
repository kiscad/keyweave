//! Explicit IPC-native gate. Status fixtures expose synthetic DTOs; one owned no-input worker exercises host lifecycle.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]
use keyweave_linux::session_control::Server;
use keyweave_progress::session_control::{Phase, Snapshot};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "kw-host-control-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        Self(p)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn run(stop: Option<bool>) -> (i32, String, bool, u64) {
    let dir = Directory::new();
    let path = dir.0.join("control");
    let mut server = Server::bind(&path).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_keyweave-host"));
    command
        .arg(if stop.is_some() {
            "stop-linux"
        } else {
            "status-linux"
        })
        .arg("--socket")
        .arg(&path);
    if let Some(current) = stop {
        let session = if current {
            server.session()
        } else if server.session() == 1 {
            2
        } else {
            1
        };
        command.arg("--session").arg(session.to_string());
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(4);
    loop {
        server
            .poll(Snapshot {
                session: server.session(),
                phase: Phase::Starting,
                progress_epoch: 0,
                generation: None,
                exit: None,
                recovery_confirmed: false,
                fault: false,
            })
            .unwrap();
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= end {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("owned CLI fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        server.stop_requested(),
        server.session(),
    )
}
#[test]
#[ignore = "requires an IPC-enabled environment; explicit --ignored native gate"]
fn actual_cli_status_queries_same_uid_host_without_claiming_worker_readiness() {
    let (code, text, stop, session) = run(None);
    assert_eq!(code, 0);
    assert!(!stop);
    assert!(text.contains(&format!("public_session={session}")));
    assert!(text.contains("phase=Starting"));
    assert!(text.contains("worker_exit=None"));
    assert!(text.contains("recovery_confirmed=false"));
}
#[test]
#[ignore = "requires an IPC-enabled environment; explicit --ignored native gate"]
fn actual_cli_rejects_stale_public_session_without_setting_stop() {
    let (code, text, stop, _) = run(Some(false));
    assert_eq!(code, 1);
    assert!(!stop);
    assert!(text.contains("StaleSession"));
    assert!(text.contains("worker_exit=None"));
}

#[test]
#[ignore = "requires an IPC-enabled environment; explicit --ignored native gate"]
fn actual_cli_stop_acceptance_does_not_report_exit_or_recovery() {
    let (code, text, stop, _) = run(Some(true));
    assert_eq!(code, 0);
    assert!(stop);
    assert!(text.contains("StopAccepted"));
    assert!(text.contains("phase=StopRequested"));
    assert!(text.contains("worker_exit=None"));
    assert!(text.contains("recovery_confirmed=false"));
}
#[test]
#[ignore = "requires an IPC-enabled environment and the explicit no-input fixture built by native gate"]
fn actual_host_socket_stop_observes_owned_no_input_worker_exit_without_fake_progress() {
    use keyweave_linux::session_control::Client;
    use keyweave_progress::session_control::{
        Command as ControlCommand, Exit, Outcome, Phase, Request,
    };
    let Some(worker) = std::env::var_os("KEYWEAVE_CONTROL_FIXTURE") else {
        panic!("run tools/verify_session_control_native.py");
    };
    let worker = PathBuf::from(worker);
    assert!(worker.is_absolute());
    let dir = Directory::new();
    let socket = dir.0.join("control");
    let config = dir.0.join("config.json");
    fs::write(&config, br#"{"profiles":[{}]}"#).unwrap();
    struct OwnedChild(std::process::Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let mut host = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_keyweave-host"))
            .args(["run-linux", "--config"])
            .arg(&config)
            .arg("--worker")
            .arg(worker)
            .args([
                "--session-provider",
                "none",
                "--input",
                "/keyweave-absent-input",
                "--uinput",
                "/keyweave-absent-output",
                "--control-socket",
            ])
            .arg(&socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let end = Instant::now() + Duration::from_secs(4);
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    while !fs::symlink_metadata(&socket)
        .is_ok_and(|m| m.file_type().is_socket() && m.permissions().mode() & 0o077 == 0)
    {
        assert!(Instant::now() < end);
        assert!(host.0.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(1));
    }
    fn query(
        socket: &std::path::Path,
        command: ControlCommand,
        id: u64,
    ) -> keyweave_progress::session_control::Reply {
        let mut client =
            Client::connect(socket, Request { id, command }, Duration::from_secs(2)).unwrap();
        loop {
            if let Some(reply) = client.poll().unwrap() {
                return reply;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let initial = query(&socket, ControlCommand::Status, 1);
    assert_eq!(initial.snapshot.phase, Phase::Starting);
    assert_eq!(initial.snapshot.progress_epoch, 0);
    assert_eq!(initial.snapshot.exit, None);
    let stop = query(
        &socket,
        ControlCommand::Stop {
            session: initial.snapshot.session,
        },
        2,
    );
    assert_eq!(stop.outcome, Outcome::StopAccepted);
    assert_eq!(stop.snapshot.phase, Phase::StopRequested);
    assert_eq!(stop.snapshot.exit, None);
    assert!(!stop.snapshot.recovery_confirmed);
    let mut id = 3;
    loop {
        let reply = query(&socket, ControlCommand::Status, id);
        id += 1;
        if reply.snapshot.phase == Phase::Exited {
            assert_eq!(reply.snapshot.exit, Some(Exit::Code(0)));
            assert_eq!(reply.snapshot.progress_epoch, 0);
            assert!(
                reply.snapshot.recovery_confirmed,
                "no-input fixture report, not global keyboard proof"
            );
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    while host.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(host.0.wait().unwrap().success());
    assert!(!socket.exists());
}
