//! Actual-main owned processes/pipes/signals and synthetic engine inputs only.
//! No Runtime, console provider, native input/output or recovery report.
#![forbid(unsafe_code)]
#[cfg(target_os = "linux")]
#[allow(clippy::unwrap_used)]
fn main() {
    use keyweave_engine::Engine;
    use keyweave_host::linux::{DriveStatus, NativeSession, ReloadState, Session};
    use keyweave_linux::{
        config_artifact::ConfigArtifact, host_signals::HostSignals, progress::Policy,
        reactor::MonotonicClock, supervisor::Supervisor, termination::ReadStatus,
        worker::ReportingStartup,
    };
    use keyweave_progress::reload::Outcome;
    use keyweave_progress::{ProgressClient, ProgressPhase, RequestEvent};
    use keyweave_types::{
        ConfigGeneration, DeviceInstanceId, InputEnvelope, InputEvent, KeyEdge, KeyId, MonoTime,
        Provenance,
    };
    use keyweave_worker::{
        linux_preparation::{PreparedConfig, default_output_spec},
        reload_control::{Controller, Evidence, Step, reap_after_shutdown},
        reload_preparation::Target,
    };
    use std::{
        fs::File,
        io,
        os::fd::AsFd,
        process::Command,
        time::{Duration, Instant},
    };
    const MODE: &str = "KEYWEAVE_NATIVE_RELOAD_FIXTURE";
    const OLD:&[u8]=br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
    const NEW:&[u8]=br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"c"}]}]}]}"#;
    fn event(seq: u64, at: MonoTime, edge: KeyEdge) -> InputEnvelope {
        InputEnvelope {
            seq,
            at_ns: at,
            device: DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: KeyId::new(7, 4),
                edge,
            },
        }
    }
    if let Ok(mode) = std::env::var(MODE) {
        std::panic::set_hook(Box::new(|_| {}));
        let mut startup = ReportingStartup::from_supervisor_environment().unwrap();
        startup.guard().verify_isolated_group().unwrap();
        assert!(startup.reload_requested().unwrap());
        let bytes = startup.claim_config().unwrap().read_bytes().unwrap();
        assert_eq!(bytes, OLD);
        let prepared = PreparedConfig::from_bytes(bytes, default_output_spec().unwrap()).unwrap();
        let mut actor = Engine::new(prepared.snapshot().clone()).unwrap();
        let (reader, writer) = startup.claim_reload().unwrap().into_parts();
        let mut reload = Controller::new(
            reader,
            writer,
            startup.run_id(),
            ConfigGeneration(1),
            5_000_000_000,
            Target::Linux(prepared.output_spec().clone()),
        )
        .unwrap();
        let (guard, original, _terminal) = startup.into_parts();
        drop(original);
        let mut progress = ProgressClient::new(
            File::from(io::stdin().as_fd().try_clone_to_owned().unwrap()),
            File::from(io::stderr().as_fd().try_clone_to_owned().unwrap()),
            57,
        )
        .unwrap();
        let mut clock = MonotonicClock::new();
        let first = clock.now().unwrap();
        let mut parent_until = first.checked_add(2_000_000_000).unwrap();
        // Synthetic session authority: no console/consumer readiness claim.
        let session_until = first.checked_add(10_000_000_000).unwrap();
        let mut seq = 1;
        let mut epoch = 0;
        assert_eq!(
            actor
                .handle(event(seq, first, KeyEdge::Down))
                .unwrap()
                .outputs[0]
                .key,
            KeyId::new(7, 5)
        );
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            guard.verify_isolated_group().unwrap();
            let before = clock.now().unwrap();
            assert!(before < parent_until);
            let request = progress.receive_command(epoch).unwrap();
            if request == RequestEvent::Stop {
                assert_eq!(mode, "accepted");
                assert_eq!(actor.config_generation(), ConfigGeneration(2));
                seq += 1;
                assert_eq!(
                    actor
                        .handle(event(seq, before, KeyEdge::Up))
                        .unwrap()
                        .outputs[0]
                        .key,
                    KeyId::new(7, 5)
                );
                seq += 1;
                assert_eq!(
                    actor
                        .handle(event(seq, before, KeyEdge::Down))
                        .unwrap()
                        .outputs[0]
                        .key,
                    KeyId::new(7, 6)
                );
                seq += 1;
                actor.handle(event(seq, before, KeyEdge::Up)).unwrap();
                reap_after_shutdown(&mut reload).unwrap();
                return;
            }
            let after = clock.now().unwrap();
            assert!(after < parent_until);
            if request == RequestEvent::Challenge {
                parent_until = before.checked_add(2_000_000_000).unwrap();
                seq += 1;
                let step = actor.handle(event(seq, after, KeyEdge::Repeat)).unwrap();
                assert!(
                    step.outputs
                        .iter()
                        .all(|output| output.key == KeyId::new(7, 5))
                );
                epoch += 1; // actual synthetic Engine iteration, not device progress
            }
            progress.publish(epoch, ProgressPhase::Active).unwrap();
            reload
                .tick(&mut actor, || {
                    clock.now().map(|at| Evidence {
                        at,
                        parent_until,
                        session_until,
                        stop: false,
                    })
                })
                .unwrap();
            if mode == "lost_ack" && reload.step() == Step::Acknowledging {
                assert_eq!(actor.config_generation(), ConfigGeneration(2));
                assert_eq!(actor.state().binding_generations, vec![ConfigGeneration(1)]);
                // Commit occurred, no ack write. Original selected FDs remain
                // until actual exit; parent must keep observed generation=1.
                reap_after_shutdown(&mut reload).unwrap();
                return;
            }
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    for mode in ["accepted", "lost_ack"] {
        let signals = HostSignals::install().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env(MODE, mode);
        signals.prepare_child(&mut command).unwrap();
        let mut clock = MonotonicClock::new();
        let now = clock.now().unwrap();
        let owner = Supervisor::spawn_reporting_config_reload(
            command,
            57,
            Policy::default(),
            now,
            ConfigArtifact::create(OLD).unwrap(),
        )
        .unwrap();
        let mut session = NativeSession::new(Session::new(owner), signals, clock);
        assert_eq!(
            session.request_reload(NEW.into()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let started = Instant::now();
        loop {
            assert!(matches!(session.poll_once(), DriveStatus::Pending(_)));
            match session.request_reload(Box::new([])) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
                    break;
                }
                Ok(()) => panic!("empty candidate accepted"),
            }
            assert!(started.elapsed() < Duration::from_secs(5));
        }
        let mut body = b"/* private padding ".to_vec();
        body.extend(vec![b'x'; 96 * 1024]);
        body.extend_from_slice(b" */\n");
        body.extend_from_slice(NEW);
        session.request_reload(body.into_boxed_slice()).unwrap();
        assert_eq!(
            session.request_reload(NEW.into()).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        if mode == "accepted" {
            loop {
                assert!(matches!(session.poll_once(), DriveStatus::Pending(_)));
                if session.reload_state() == Some(ReloadState::Idle)
                    && session.observed_config_generation() == Some(ConfigGeneration(2))
                {
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(5));
            }
            // Confirmation result must be taken before the next raw candidate.
            assert_eq!(
                session.request_reload(NEW.into()).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
            assert_eq!(session.take_reload_outcome(), Some(Outcome::Accepted));
            session
                .request_reload(Box::new(*b"invalid private configuration"))
                .unwrap();
            loop {
                assert!(matches!(session.poll_once(), DriveStatus::Pending(_)));
                if let Some(outcome) = session.take_reload_outcome() {
                    assert_eq!(outcome, Outcome::Rejected);
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(5));
            }
            assert_eq!(
                session.observed_config_generation(),
                Some(ConfigGeneration(2))
            );
            // Only this own parent PID, never foreground group or another app.
            assert!(
                Command::new("kill")
                    .args(["-INT", &std::process::id().to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let report = loop {
            match session.poll_once() {
                DriveStatus::Exited(report) => break report,
                DriveStatus::Pending(_) => {}
                DriveStatus::ObservationExpired => panic!("same child observation expired"),
            }
            assert!(started.elapsed() < Duration::from_secs(10));
        };
        assert!(report.signal_close_failure.is_none());
        if mode == "accepted" {
            assert!(report.worker.fault.is_none());
            assert_eq!(
                report.worker.reason,
                Some(keyweave_linux::supervisor::TerminationReason::Cooperative)
            );
            assert_eq!(
                report.worker.signal,
                Some(keyweave_linux::host_signals::StopSignal::Interrupt)
            );
        }
        assert_eq!(report.worker.status.code(), Some(0));
        assert!(report.worker.progress_epoch > 0);
        assert_eq!(report.worker.terminal.status, ReadStatus::Missing);
        assert!(!report.successful());
        assert!(!report.worker.recovery_confirmed());
        assert_eq!(session.reload_state(), Some(ReloadState::Closed));
        assert_eq!(
            session.observed_config_generation(),
            Some(ConfigGeneration(if mode == "accepted" { 2 } else { 1 }))
        );
        if mode == "lost_ack" {
            assert_eq!(session.take_reload_outcome(), None);
        }
        assert_eq!(
            session.request_reload(NEW.into()).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        assert!(matches!(session.poll_once(),DriveStatus::Exited(cached) if cached==report));
    }
    println!(
        "linux_reload: owned actual-main accepted/rejected/lost-ack/Stop/progress passed; synthetic Engine only, no device/recovery claims"
    );
}
#[cfg(not(target_os = "linux"))]
fn main() {}
