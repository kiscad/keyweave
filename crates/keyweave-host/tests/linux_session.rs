// Real main-thread no-input worker. Synthetic reports prove transport only.
#![forbid(unsafe_code)]
#[cfg(target_os = "linux")]
#[allow(clippy::unwrap_used)]
fn main() {
    use keyweave_host::linux::{Session, State};
    use keyweave_linux::{
        config_artifact::ConfigArtifact,
        host_signals::StopSignal,
        progress::{CHALLENGE_BYTES, Policy, StopRequest},
        runtime::{StopReason, TerminationReport},
        supervisor::{Supervisor, SupervisorStatus, TerminationReason},
        termination::ReadStatus,
        worker::{ReportingStartup, WorkerReport, finalize_worker_report},
    };
    use keyweave_types::MonoTime;
    use std::{
        io::{self, Read},
        process::Command,
        time::{Duration, Instant},
    };
    const MODE: &str = "KEYWEAVE_HOST_SESSION_FIXTURE";
    if let Ok(mode) = std::env::var(MODE) {
        let mut startup = ReportingStartup::from_supervisor_environment().unwrap();
        startup.guard().verify_isolated_group().unwrap();
        assert_eq!(
            startup.claim_config().unwrap().read_bytes().unwrap(),
            b"no input fixture"
        );
        let (_guard, client, mut terminal) = startup.into_parts();
        drop(client);
        let end = Instant::now() + Duration::from_secs(3);
        let mut bytes = [0; CHALLENGE_BYTES];
        let mut read = 0;
        while read < bytes.len() {
            match io::stdin().read(&mut bytes[read..]) {
                Ok(0) => panic!("stop endpoint closed"),
                Ok(n) => read += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(e) => panic!("{e}"),
            }
            assert!(Instant::now() < end);
        }
        assert_eq!(StopRequest::decode(&bytes).unwrap().id, 1);
        if mode != "missing" {
            let uncertain = mode == "uncertain";
            let report = TerminationReport {
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
            let sent = finalize_worker_report(
                WorkerReport {
                    outcome: Ok(StopReason::Pause),
                    termination: Some(report),
                },
                &mut terminal,
            );
            assert!(sent.delivery.frame_sent && sent.delivery.failure.is_none());
        }
        return;
    }
    for mode in ["complete", "uncertain", "missing"] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env(MODE, mode);
        let supervisor = Supervisor::spawn_reporting_config_isolated(
            command,
            57,
            Policy::default(),
            MonoTime(0),
            ConfigArtifact::create(b"no input fixture").unwrap(),
        )
        .unwrap();
        let mut session = Session::new(supervisor);
        // First event is delivered before any challenge write. All events are
        // injected; this test sends no OS/foreground-group signals.
        let state = session.poll(MonoTime(1), Some(StopSignal::Interrupt));
        if let State::Pending(status) = state {
            assert!(matches!(status, SupervisorStatus::ShutdownRequested { .. }));
        }
        let end = Instant::now() + Duration::from_secs(3);
        let report = loop {
            if let State::Exited(report) = session.poll(MonoTime(2), Some(StopSignal::Terminate)) {
                break report;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(report.status.code(), Some(0));
        assert_eq!(report.reason, Some(TerminationReason::Cooperative));
        assert_eq!(report.signal, Some(StopSignal::Interrupt));
        assert_eq!(report.progress_epoch, 0);
        assert!(report.fault.is_none());
        assert_eq!(report.successful(), mode == "complete");
        assert_eq!(report.recovery_confirmed(), mode == "complete");
        assert_eq!(
            matches!(report.terminal.status, ReadStatus::Missing),
            mode == "missing"
        );
        assert_eq!(
            session.poll(MonoTime(0), Some(StopSignal::Hangup)),
            State::Exited(report)
        );
    }
    // Real signals target only this owned no-input harness PID. The child
    // receives a private Stop frame, not a terminal foreground-group signal.
    for mode in ["complete", "missing"] {
        use keyweave_host::linux::{DriveStatus, NativeSession};
        use keyweave_linux::{host_signals::HostSignals, reactor::MonotonicClock};
        let signals = HostSignals::install().unwrap();
        let mut clock = MonotonicClock::new();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env(MODE, mode);
        signals.prepare_child(&mut command).unwrap();
        let now = clock.now().unwrap();
        let supervisor = Supervisor::spawn_reporting_config_isolated(
            command,
            58,
            Policy::default(),
            now,
            ConfigArtifact::create(b"no input fixture").unwrap(),
        )
        .unwrap();
        let mut own_signal = Command::new("/bin/kill");
        own_signal.args(["-s", "INT", &std::process::id().to_string()]);
        signals.prepare_child(&mut own_signal).unwrap();
        assert!(own_signal.status().unwrap().success());
        let mut native = NativeSession::new(Session::new(supervisor), signals, clock);
        let DriveStatus::Exited(report) = native.run() else {
            panic!("native child exit was not observed");
        };
        assert_eq!(report.worker.status.code(), Some(0));
        assert_eq!(report.worker.signal, Some(StopSignal::Interrupt));
        assert_eq!(report.worker.reason, Some(TerminationReason::Cooperative));
        assert_eq!(report.worker.progress_epoch, 0);
        assert_eq!(report.successful(), mode == "complete");
        assert!(report.signal_close_failure.is_none());
        assert_eq!(native.run(), DriveStatus::Exited(report));
    }
    println!(
        "linux_session: actual owned exit, first Stop and independent synthetic KWLT classifications passed (no input access)"
    );
}
#[cfg(not(target_os = "linux"))]
fn main() {}
