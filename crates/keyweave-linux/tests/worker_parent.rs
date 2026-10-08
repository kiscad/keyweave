// Custom harness: child mode must execute in the actual process main thread,
// not a libtest thread whose parent-death setting is separate.
#![forbid(unsafe_code)]
#[cfg(target_os = "linux")]
#[allow(clippy::unwrap_used)]
fn main() {
    use keyweave_linux::{
        progress::{CHALLENGE_BYTES, Challenge, Policy, Receipt},
        runtime::RuntimePhase,
        supervisor::{Supervisor, SupervisorStatus},
        worker::WorkerParentGuard,
    };
    use keyweave_types::MonoTime;
    use std::{
        io::{self, Read, Write},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    const MODE: &str = "KEYWEAVE_PARENT_TEST_MODE";
    const PARENT: &str = "KEYWEAVE_SUPERVISOR_PID";
    if let Ok(mode) = std::env::var(MODE) {
        if mode != "verified" {
            let error = WorkerParentGuard::from_supervisor_environment().unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(if mode == "missing" {
                    libc::EPERM
                } else {
                    libc::EINVAL
                })
            );
            return;
        }
        let guard = WorkerParentGuard::from_supervisor_environment().unwrap();
        let parent = guard.parent_pid();
        assert_eq!(parent.to_string(), std::env::var(PARENT).unwrap());
        assert_eq!(
            WorkerParentGuard::verify(0).unwrap_err().raw_os_error(),
            Some(libc::EINVAL)
        );
        assert_eq!(
            WorkerParentGuard::verify(parent + 1)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ECANCELED)
        );
        let errno = std::thread::spawn(move || {
            WorkerParentGuard::verify(parent)
                .unwrap_err()
                .raw_os_error()
        })
        .join()
        .unwrap();
        assert_eq!(errno, Some(libc::EPERM));
        let mut bytes = [0; CHALLENGE_BYTES];
        let mut read = 0;
        let end = Instant::now() + Duration::from_secs(3);
        while read < bytes.len() {
            match io::stdin().read(&mut bytes[read..]) {
                Ok(0) => panic!("parent closed test channel"),
                Ok(count) => read += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("{error}"),
            }
            assert!(Instant::now() < end);
        }
        let receipt = Receipt {
            challenge: Challenge::decode(&bytes).unwrap(),
            baseline: 0,
            epoch: 1,
            phase: RuntimePhase::Stopped,
        };
        io::stderr().write_all(&receipt.encode()).unwrap();
        return;
    }
    let fixture = |mode: &str| {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env(MODE, mode).stdout(Stdio::null());
        command
    };
    let mut command = fixture("verified");
    // Supervisor replaces stale/untrusted inherited values.
    command.env(PARENT, "0");
    let mut supervisor = Supervisor::spawn(command, 57, Policy::default(), MonoTime(0)).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let SupervisorStatus::Exited { status, reason } = supervisor.poll(MonoTime(0)).unwrap() {
            assert_eq!(reason, None);
            break status;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(status.success(), "worker main-thread verification failed");
    assert!(
        fixture("missing")
            .env(PARENT, std::process::id().to_string())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        fixture("malformed")
            .env(PARENT, "+1")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        fixture("absent")
            .env_remove(PARENT)
            .status()
            .unwrap()
            .success()
    );
    println!(
        "worker_parent: verified main-thread guard, missing protection, malformed and absent identity passed (no device access)"
    );
}
#[cfg(not(target_os = "linux"))]
fn main() {}
