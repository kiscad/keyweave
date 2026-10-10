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
        worker::{WorkerParentGuard, WorkerStartup},
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
        if mode == "reporting"
            || mode == "reporting_uncertain"
            || mode == "sealed_config"
            || mode == "isolated_config"
            || mode == "masked_isolated_config"
        {
            use keyweave_linux::{
                runtime::{StopReason, TerminationReport},
                worker::ReportingStartup,
            };
            let mut startup = ReportingStartup::from_supervisor_environment().unwrap();
            if mode == "sealed_config"
                || mode == "isolated_config"
                || mode == "masked_isolated_config"
            {
                let config = startup.claim_config().unwrap();
                assert_eq!(config.read_bytes().unwrap(), b"exact\0sealed config");
                assert_eq!(
                    startup.claim_config().unwrap_err().kind(),
                    io::ErrorKind::AlreadyExists
                );
                drop(config);
            } else {
                assert!(std::env::var("KEYWEAVE_CONFIG_FD").is_err());
                assert_eq!(
                    startup.claim_config().unwrap_err().kind(),
                    io::ErrorKind::InvalidInput
                );
                assert_eq!(
                    startup.claim_config().unwrap_err().kind(),
                    io::ErrorKind::AlreadyExists
                );
            }
            let (guard, client, mut terminal) = startup.into_parts();
            if mode == "isolated_config" || mode == "masked_isolated_config" {
                if mode == "isolated_config" {
                    guard.verify_isolated_group().unwrap();
                } else {
                    // Deliberately omitted prepare_child: child inherits blocked
                    // signals. The guard must refuse this console contract.
                    assert_eq!(
                        guard.verify_isolated_group().unwrap_err().raw_os_error(),
                        Some(1)
                    );
                }
                for entry in std::fs::read_dir("/proc/self/fd").unwrap() {
                    if let Ok(target) = std::fs::read_link(entry.unwrap().path()) {
                        assert_ne!(target.to_string_lossy(), "anon_inode:[signalfd]");
                    }
                }
                assert_eq!(
                    std::env::var("KEYWEAVE_WORKER_ISOLATED_GROUP").unwrap(),
                    "1"
                );
            } else {
                assert!(guard.verify_isolated_group().is_err());
                assert!(std::env::var("KEYWEAVE_WORKER_ISOLATED_GROUP").is_err());
            }
            assert_eq!(
                guard.parent_pid().to_string(),
                std::env::var(PARENT).unwrap()
            );
            assert_eq!(
                ReportingStartup::from_supervisor_environment()
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::AlreadyExists
            );
            assert_eq!(
                WorkerStartup::from_supervisor_environment()
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::AlreadyExists
            );
            // Synthetic no-input facts only: transport validation is not a
            // real Runtime recovery or native keyboard acceptance test.
            let uncertain = mode == "reporting_uncertain";
            let facts = TerminationReport {
                reason: StopReason::Pause,
                input_owners_released: true,
                engine_reset_confirmed: true,
                recovery_failed: false,
                recovery_budget_exhausted: uncertain,
                submitted_remaining: false,
                report_pending: uncertain,
                acceptance_unknown: false,
                output_recovery_confirmed: !uncertain,
                output_owner_dropped: true,
                backend_transition_confirmed: true,
            };
            let sent = keyweave_linux::worker::finalize_worker_report(
                keyweave_linux::worker::WorkerReport {
                    outcome: Ok(StopReason::Pause),
                    termination: Some(facts),
                },
                &mut terminal,
            );
            assert!(sent.delivery.frame_sent && sent.delivery.failure.is_none());
            assert!(matches!(sent.worker.outcome, Ok(StopReason::Pause)));
            let end = Instant::now() + Duration::from_secs(3);
            drop((terminal, client)); // original stdio remains until real exit
            let mut bytes = [0; CHALLENGE_BYTES];
            let mut read = 0;
            while read < bytes.len() {
                match io::stdin().read(&mut bytes[read..]) {
                    Ok(0) => panic!("report fixture channel closed"),
                    Ok(n) => read += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("{e}"),
                };
                assert!(Instant::now() < end);
            }
            io::stderr()
                .write_all(
                    &Receipt {
                        challenge: Challenge::decode(&bytes).unwrap(),
                        baseline: 0,
                        epoch: 1,
                        phase: RuntimePhase::Stopped,
                    }
                    .encode(),
                )
                .unwrap();
            return;
        }
        if mode == "relay_bad_run" {
            // Test-only normal exec before startup handoff or device opening.
            // Preserve the inherited kernel guard while replacing one env value
            // without unsafe process-wide environment mutation.
            use std::os::unix::process::CommandExt;
            let error = Command::new(std::env::current_exe().unwrap())
                .env(MODE, "bad_run")
                .env("KEYWEAVE_PROGRESS_RUN_ID", "+1")
                .exec();
            panic!("test exec failed: {error}");
        }
        if mode != "verified" && mode != "bad_run" {
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
            let guard_error = WorkerParentGuard::verify(parent)
                .unwrap_err()
                .raw_os_error();
            let startup_error = WorkerStartup::from_supervisor_environment()
                .err()
                .unwrap()
                .raw_os_error();
            (guard_error, startup_error)
        })
        .join()
        .unwrap();
        assert_eq!(errno, (Some(libc::EPERM), Some(libc::EPERM)));
        if mode == "verified" {
            let startup = WorkerStartup::from_supervisor_environment().unwrap();
            assert_eq!(startup.guard().parent_pid(), parent);
            assert_eq!(startup.run_id(), 57);
            assert_eq!(
                WorkerStartup::from_supervisor_environment()
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::AlreadyExists
            );
            let (owned_guard, client) = startup.into_parts();
            assert_eq!(owned_guard.parent_pid(), parent);
            drop(client);
            assert_eq!(
                WorkerStartup::from_supervisor_environment()
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::AlreadyExists
            );
            // The raw protocol fixture below deliberately exercises original
            // stdio after dropping copies; production uses only ProgressClient.
        } else {
            let error = WorkerStartup::from_supervisor_environment().err().unwrap();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("KEYWEAVE_PROGRESS_RUN_ID"));
        }
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
    for mode in [
        "reporting",
        "reporting_uncertain",
        "sealed_config",
        "isolated_config",
        "masked_isolated_config",
    ] {
        use keyweave_linux::termination::ReadStatus;
        let mut command = fixture(mode);
        command.env("KEYWEAVE_WORKER_ISOLATED_GROUP", "stale");
        command.env("KEYWEAVE_CONFIG_FD", "1"); // overwritten or removed
        let mut signals = if mode == "isolated_config" || mode == "masked_isolated_config" {
            use keyweave_linux::host_signals::HostSignals;
            let guard = HostSignals::install().unwrap();
            assert!(HostSignals::install().is_err());
            if mode == "isolated_config" {
                guard.prepare_child(&mut command).unwrap();
            }
            Some(guard)
        } else {
            None
        };
        let mut supervisor = if mode == "isolated_config" || mode == "masked_isolated_config" {
            Supervisor::spawn_reporting_config_isolated(
                command,
                57,
                Policy::default(),
                MonoTime(0),
                keyweave_linux::config_artifact::ConfigArtifact::create(b"exact\0sealed config")
                    .unwrap(),
            )
            .unwrap()
        } else if mode == "sealed_config" {
            Supervisor::spawn_reporting_config(
                command,
                57,
                Policy::default(),
                MonoTime(0),
                keyweave_linux::config_artifact::ConfigArtifact::create(b"exact\0sealed config")
                    .unwrap(),
            )
            .unwrap()
        } else {
            Supervisor::spawn_reporting(command, 57, Policy::default(), MonoTime(0)).unwrap()
        };
        assert_eq!(supervisor.poll_terminal(), ReadStatus::NotExited);
        assert_eq!(supervisor.collect_terminal().status, ReadStatus::NotExited);
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            if let SupervisorStatus::Exited { status, reason } =
                supervisor.poll(MonoTime(0)).unwrap()
            {
                assert!(status.success());
                assert_eq!(reason, None);
                break;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
        if let Some(signals) = signals.as_mut() {
            use keyweave_linux::host_signals::StopSignal;
            assert_eq!(signals.poll().unwrap(), None);
            for (name, expected) in [
                ("INT", StopSignal::Interrupt),
                ("TERM", StopSignal::Terminate),
                ("HUP", StopSignal::Hangup),
            ] {
                // Sends only to this owned, separate no-input harness PID, never
                // the terminal foreground group or an arbitrary external PID.
                let mut own_signal = Command::new("/bin/kill");
                own_signal.args(["-s", name, &std::process::id().to_string()]);
                signals.prepare_child(&mut own_signal).unwrap();
                assert!(own_signal.status().unwrap().success());
                signals.wait(Duration::from_millis(20)).unwrap();
                assert_eq!(signals.poll().unwrap(), Some(expected));
                assert_eq!(signals.poll().unwrap(), None);
            }
        }
        if let Some(signals) = signals {
            signals.close().unwrap();
        }
        let epoch = supervisor.progress_epoch();
        let deadline = supervisor.deadline();
        let observed = supervisor.collect_terminal();
        assert!(observed.failure.is_none());
        assert_eq!(supervisor.collect_terminal(), observed);
        let ReadStatus::Complete(frame) = observed.status else {
            panic!("unexpected observation {observed:?}");
        };
        assert_eq!(frame.run_id, 57);
        assert_eq!(
            frame.report.output_recovery_confirmed,
            mode != "reporting_uncertain"
        );
        assert_eq!(frame.report.report_pending, mode == "reporting_uncertain");
        assert_eq!(
            frame.report.recovery_budget_exhausted,
            mode == "reporting_uncertain"
        );
        assert_eq!(supervisor.poll_terminal(), ReadStatus::Complete(frame));
        assert_eq!(supervisor.progress_epoch(), epoch);
        assert_eq!(supervisor.deadline(), deadline);
    }
    let mut command = fixture("verified");
    // Supervisor replaces stale/untrusted inherited values.
    command.env(PARENT, "0");
    let mut supervisor = Supervisor::spawn(command, 57, Policy::default(), MonoTime(0)).unwrap();
    assert_eq!(
        supervisor.poll_terminal(),
        keyweave_linux::termination::ReadStatus::Unsupported
    );
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
    let mut supervisor =
        Supervisor::spawn(fixture("relay_bad_run"), 57, Policy::default(), MonoTime(0)).unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        if let SupervisorStatus::Exited { status, reason } = supervisor.poll(MonoTime(0)).unwrap() {
            assert!(
                status.success(),
                "bad run ID was not rejected before handoff"
            );
            assert_eq!(reason, None);
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
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
        "worker_parent: guard/startup/report channels and synthetic terminal EOF, duplicate claim, malformed run ID, missing protection and malformed/absent parent identity passed (no device access)"
    );
}
#[cfg(not(target_os = "linux"))]
fn main() {}
