//! Actual-main no-input endpoint inheritance harness. Only owned child/pipes.
#![forbid(unsafe_code)]
#[cfg(target_os = "linux")]
#[allow(clippy::unwrap_used)]
fn main() {
    use keyweave_linux::{
        config_artifact::ConfigArtifact,
        progress::Policy,
        supervisor::{Supervisor, SupervisorStatus},
        worker::ReportingStartup,
    };
    use keyweave_progress::reload::{
        Client, Outcome, Worker,
        transport::{AckWriter, Parent, ReceiveStep, Receiver, WriteStep},
    };
    use keyweave_types::{ConfigGeneration, MonoTime};
    use std::{
        os::unix::process::ExitStatusExt,
        process::Command,
        time::{Duration, Instant},
    };
    const MODE: &str = "KEYWEAVE_RELOAD_TEST_MODE";
    if let Ok(mode) = std::env::var(MODE) {
        let mut startup = ReportingStartup::from_supervisor_environment().unwrap();
        let config = startup.claim_config().unwrap();
        assert_eq!(config.read_bytes().unwrap(), b"cold immutable bytes");
        if mode == "absent" {
            for env in ["KEYWEAVE_RELOAD_REQUEST_FD", "KEYWEAVE_RELOAD_ACK_FD"] {
                assert!(std::env::var_os(env).is_none());
            }
            assert_eq!(
                startup.claim_reload().err().unwrap().kind(),
                std::io::ErrorKind::InvalidInput
            );
            assert_eq!(
                startup.claim_reload().err().unwrap().kind(),
                std::io::ErrorKind::AlreadyExists
            );
            return;
        }
        startup.guard().verify_isolated_group().unwrap();
        let endpoints = startup.claim_reload().unwrap();
        assert_eq!(
            startup.claim_reload().err().unwrap().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        let (reader, writer) = endpoints.into_parts();
        let mut rx = Receiver::new(
            reader,
            Worker::new(startup.run_id(), ConfigGeneration(1)).unwrap(),
        );
        let until = Instant::now() + Duration::from_secs(5);
        while rx.poll().unwrap() != ReceiveStep::Ready {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        let received = rx.take().unwrap();
        assert_eq!(received.body, vec![b'x'; 64 * 1024 + 17]);
        // No actor is created by this transport fixture: only Rejected at base.
        let ack = rx.finish(Outcome::Rejected, ConfigGeneration(1)).unwrap();
        let mut writer = AckWriter::new(writer, ack).unwrap();
        loop {
            if let Some(sent) = writer.poll().unwrap() {
                rx.ack_sent(sent).unwrap();
                break;
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        return;
    }
    for reload in [true, false] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.env(MODE, if reload { "reload" } else { "absent" });
        // Legacy mode must remove these inherited/stale selectors.
        command.env("KEYWEAVE_RELOAD_REQUEST_FD", "999999");
        command.env("KEYWEAVE_RELOAD_ACK_FD", "999998");
        let config = ConfigArtifact::create(b"cold immutable bytes").unwrap();
        let mut owner = if reload {
            Supervisor::spawn_reporting_config_reload(
                command,
                42,
                Policy::default(),
                MonoTime(0),
                config,
            )
            .unwrap()
        } else {
            Supervisor::spawn_reporting_config(command, 42, Policy::default(), MonoTime(0), config)
                .unwrap()
        };
        let endpoints = owner.take_reload_endpoints();
        assert!(owner.take_reload_endpoints().is_none());
        let started = Instant::now();
        if let Some(endpoints) = endpoints {
            let (reader, writer) = endpoints.into_parts();
            let mut parent = Parent::new(
                reader,
                Client::new(42, ConfigGeneration(1), MonoTime(0), 5_000_000_000).unwrap(),
            );
            let mut request = parent
                .start(
                    writer,
                    vec![b'x'; 64 * 1024 + 17].into_boxed_slice(),
                    MonoTime(0),
                )
                .unwrap();
            loop {
                let clock = || Ok(MonoTime(started.elapsed().as_nanos() as u64));
                if parent.write(&mut request, clock).unwrap() == WriteStep::Complete {
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::yield_now();
            }
            loop {
                if let Some(outcome) = parent
                    .poll(|| Ok(MonoTime(started.elapsed().as_nanos() as u64)))
                    .unwrap()
                {
                    assert_eq!(outcome, Outcome::Rejected);
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::yield_now();
            }
            assert_eq!(parent.active(), ConfigGeneration(1));
        } else {
            assert!(!reload);
        }
        loop {
            if let SupervisorStatus::Exited { status, .. } = owner
                .poll(MonoTime(started.elapsed().as_nanos() as u64))
                .unwrap()
            {
                assert!(status.success() || status.signal() == Some(9));
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
        assert_eq!(owner.progress_epoch(), 0);
    }
}
#[cfg(not(target_os = "linux"))]
fn main() {}
