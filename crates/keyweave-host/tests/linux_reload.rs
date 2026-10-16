//! Actual-main owned processes/pipes/signals and synthetic engine inputs only.
//! No Runtime, console provider, native input/output or recovery report.
#![forbid(unsafe_code)]
#[cfg(target_os = "linux")]
#[allow(clippy::unwrap_used)]
fn main() {
    use keyweave_engine::Engine;
    use keyweave_host::linux::watcher::{ConfigWatcher, WatchEvent};
    use keyweave_host::linux::{DriveStatus, NativeSession, ReloadState, Session};
    use keyweave_linux::{
        config_artifact::ConfigArtifact, host_signals::HostSignals, progress::Policy,
        reactor::MonotonicClock, supervisor::Supervisor, termination::ReadStatus,
        worker::ReportingStartup,
    };
    use keyweave_progress::reload::Outcome;
    use keyweave_progress::session_apply::{
        Outcome as ApplyOutcome, Request as ApplyRequest, Transaction,
    };
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
                assert!(mode == "accepted" || mode == "watcher" || mode == "apply");
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
            if mode.ends_with("lost_ack") && reload.step() == Step::Acknowledging {
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
    for mode in ["accepted", "lost_ack", "watcher", "apply", "apply_lost_ack"] {
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
        let mut watcher = None;
        if mode.starts_with("apply") {
            let mut coordinator = ConfigWatcher::apply_only(std::sync::Arc::from(OLD)).unwrap();
            assert!(matches!(
                coordinator.poll(&mut session).unwrap(),
                DriveStatus::Pending(_)
            ));
            let mut foreign = ConfigWatcher::apply_only(std::sync::Arc::from(OLD)).unwrap();
            assert_eq!(
                foreign.poll(&mut session),
                Err(keyweave_host::linux::watcher::WatchError::Identity)
            );
            assert!(session.fault().is_none());
            let started_apply = Instant::now();
            let request = ApplyRequest {
                id: 1,
                session: 99,
                base: 1,
                target: 2,
                byte_length: body.len() as u64,
            };
            let mut transaction = Transaction::new(
                ApplyRequest::decode(&request.encode().unwrap()).unwrap(),
                body.into_boxed_slice(),
                MonoTime(0),
                5_000_000_000,
            )
            .unwrap();
            let mut claimed = None;
            loop {
                let status = coordinator.poll(&mut session).unwrap();
                let snapshot = session.control_snapshot(99);
                let result = {
                    let mut backend = coordinator.apply_backend(&mut session);
                    let result = transaction.step(snapshot, &mut backend, || {
                        MonoTime(started_apply.elapsed().as_nanos() as u64)
                    });
                    if let Some(ticket) = backend.queued_ticket() {
                        assert!(claimed.replace(ticket).is_none());
                    }
                    result
                };
                if claimed.is_some()
                    && mode == "apply"
                    && transaction.may_have_applied()
                    && session.observed_config_generation() == Some(ConfigGeneration(1))
                {
                    assert_eq!(
                        session.request_reload(NEW.into()).unwrap_err().kind(),
                        io::ErrorKind::PermissionDenied
                    );
                    // Drive the original owner until its actual ACK is queued,
                    // then prove the public raw consumer cannot steal it.
                    loop {
                        assert!(matches!(session.poll_once(), DriveStatus::Pending(_)));
                        if session.reload_state() == Some(ReloadState::Idle)
                            && session.observed_config_generation() == Some(ConfigGeneration(2))
                        {
                            break;
                        }
                        assert!(started_apply.elapsed() < Duration::from_secs(5));
                    }
                    assert_eq!(session.take_reload_outcome(), None);
                }
                match result {
                    Ok(Some(reply)) => {
                        assert_eq!(mode, "apply");
                        assert!(reply.matches(request));
                        assert_eq!(reply.outcome, ApplyOutcome::Committed);
                        assert_eq!(reply.observed, Some(2));
                        assert_eq!(
                            keyweave_progress::session_apply::Reply::decode(
                                &reply.encode().unwrap()
                            )
                            .unwrap(),
                            reply
                        );
                        break;
                    }
                    Err(failure) => {
                        assert_eq!(mode, "apply_lost_ack");
                        assert!(failure.may_have_applied);
                        assert!(matches!(status, DriveStatus::Exited(_)));
                        coordinator.abandon_apply(&claimed.unwrap()).unwrap();
                        assert_eq!(
                            session.observed_config_generation(),
                            Some(ConfigGeneration(1))
                        );
                        break;
                    }
                    Ok(None) => {}
                }
                assert!(started_apply.elapsed() < Duration::from_secs(5));
            }
            assert!(claimed.is_some());
            if mode == "apply" {
                let bytes = b"invalid private configuration";
                let request = ApplyRequest {
                    id: 2,
                    session: 99,
                    base: 2,
                    target: 3,
                    byte_length: bytes.len() as u64,
                };
                let mut rejected =
                    Transaction::new(request, Box::new(*bytes), MonoTime(0), 5_000_000_000)
                        .unwrap();
                let started_rejected = Instant::now();
                loop {
                    assert!(matches!(
                        coordinator.poll(&mut session).unwrap(),
                        DriveStatus::Pending(_)
                    ));
                    let snapshot = session.control_snapshot(99);
                    if let Some(reply) = rejected
                        .step(
                            snapshot,
                            &mut coordinator.apply_backend(&mut session),
                            || MonoTime(started_rejected.elapsed().as_nanos() as u64),
                        )
                        .unwrap()
                    {
                        assert_eq!(reply.outcome, ApplyOutcome::Rejected);
                        assert_eq!(reply.observed, Some(2));
                        break;
                    }
                    assert!(started_rejected.elapsed() < Duration::from_secs(5));
                }
                assert_eq!(
                    session.observed_config_generation(),
                    Some(ConfigGeneration(2))
                );
            }
            watcher = Some(coordinator);
        } else if mode == "watcher" {
            // Own newly created fixture file only; no input/session provider.
            let path = std::env::temp_dir()
                .join(format!("keyweave-watcher-fixture-{}", std::process::id()));
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .unwrap();
            std::io::Write::write_all(&mut file, OLD).unwrap();
            drop(file);
            let mut watch =
                ConfigWatcher::new(path.clone(), std::sync::Arc::from(OLD), 60_000_000_000)
                    .unwrap();
            fn observe(watch: &mut ConfigWatcher, session: &mut NativeSession) -> WatchEvent {
                let end = Instant::now() + Duration::from_secs(5);
                loop {
                    assert!(matches!(
                        watch.poll(session).unwrap(),
                        DriveStatus::Pending(_)
                    ));
                    if let Some(event) = watch.take_event()
                        && event != WatchEvent::Submitted
                    {
                        return event;
                    }
                    assert!(Instant::now() < end, "same watcher transaction timed out");
                }
            }
            assert_eq!(observe(&mut watch, &mut session), WatchEvent::Unchanged);
            let replacement = path.with_extension("replacement");
            let mut replacement_file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&replacement)
                .unwrap();
            std::io::Write::write_all(&mut replacement_file, &body).unwrap();
            drop(replacement_file);
            std::fs::rename(replacement, &path).unwrap();
            assert_eq!(
                observe(&mut watch, &mut session),
                WatchEvent::Accepted(ConfigGeneration(2))
            );
            assert!(watch.notification_error().is_none());
            std::fs::write(&path, b"invalid private configuration").unwrap();
            assert_eq!(
                observe(&mut watch, &mut session),
                WatchEvent::Rejected(ConfigGeneration(2))
            );
            // Already rejected bytes cannot produce another wire transaction.
            watch.request_read().unwrap();
            assert_eq!(observe(&mut watch, &mut session), WatchEvent::Unchanged);
            // Restore actual accepted bytes: no unnecessary generation3.
            std::fs::write(&path, &body).unwrap();
            assert_eq!(observe(&mut watch, &mut session), WatchEvent::Unchanged);
            std::fs::remove_file(&path).unwrap();
            assert!(matches!(
                observe(&mut watch, &mut session),
                WatchEvent::ReadFailed(_)
            ));
            assert_eq!(
                session.observed_config_generation(),
                Some(ConfigGeneration(2))
            );
            watcher = Some(watch);
        } else {
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
        }
        if mode == "watcher" || mode == "apply" {
            assert!(
                Command::new("kill")
                    .args(["-INT", &std::process::id().to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let report = loop {
            let status = if let Some(watch) = watcher.as_mut() {
                watch.poll(&mut session).unwrap()
            } else {
                session.poll_once()
            };
            match status {
                DriveStatus::Exited(report) => break report,
                DriveStatus::Pending(_) => {}
                DriveStatus::ObservationExpired => panic!("same child observation expired"),
            }
            assert!(started.elapsed() < Duration::from_secs(10));
        };
        if let Some(watch) = watcher.as_mut() {
            assert_eq!(watch.take_event(), Some(WatchEvent::Closed));
            let end = Instant::now() + Duration::from_secs(5);
            while !watch.reap_closed().unwrap() {
                assert!(Instant::now() < end);
                std::thread::yield_now();
            }
        }
        assert!(report.signal_close_failure.is_none());
        if !mode.ends_with("lost_ack") {
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
            Some(ConfigGeneration(if !mode.ends_with("lost_ack") {
                2
            } else {
                1
            }))
        );
        if mode.ends_with("lost_ack") {
            assert_eq!(session.take_reload_outcome(), None);
        }
        assert_eq!(
            session.request_reload(NEW.into()).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        assert!(matches!(session.poll_once(),DriveStatus::Exited(cached) if cached==report));
    }
    println!(
        "linux_reload: owned actual-main accepted/rejected/lost-ack/watcher-notifications-debounce/public-apply-actual-ACK/public-apply-lost-ACK/Stop/progress passed; synthetic Engine only, no device/recovery claims"
    );
}
#[cfg(not(target_os = "linux"))]
fn main() {}
