//! Owned process/transport fixture only; no Hook/SendInput. Epochs are synthetic.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyweave_progress::{Policy, ProgressClient, ProgressPhase, RequestEvent};
    use keyweave_types::MonoTime;
    use keyweave_windows::{
        ConfigArtifact, Supervisor, SupervisorStatus, TerminationReason, WORKER_ARGUMENT,
        WorkerStartup,
    };
    use std::{
        io::Read,
        time::{Duration, Instant},
    };
    const IMAGE: &[u8] = b"owned supervised fixture image\0\xff";
    std::panic::set_hook(Box::new(|_| {}));
    fn stall() -> ! {
        loop {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let args: Vec<_> = std::env::args_os().skip(1).take(7).collect();
    if args.first().is_some_and(|a| a == WORKER_ARGUMENT) {
        let transfer = args.len() == 6 && args[4] == "--keyweave-config-handle";
        if args.len() != 4 && !transfer {
            std::process::exit(20);
        }
        let parent = args[1].to_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        let run = args[2].to_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        // Real main admission before threads/input or binary standard IO.
        let mut startup = match WorkerStartup::claim(parent, run) {
            Ok(v) => v,
            Err(_) => std::process::exit(20),
        };
        let mut config = if transfer {
            let value = args[5].to_str().and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut config = match startup.claim_config_reader(value) {
                Ok(config) => config,
                Err(_) => std::process::exit(20),
            };
            let mut bytes = Vec::new();
            if (&mut config)
                .take(IMAGE.len() as u64 + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes != IMAGE
            {
                std::process::exit(20);
            }
            Some(config)
        } else {
            None
        };
        let (_guard, run, reader, writer) = startup.into_parts();
        let mode = args[3].to_str().unwrap_or("");
        if mode == "startup_stall" {
            stall();
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut client = ProgressClient::new(reader, writer, run)?;
            let mut epoch = 10;
            loop {
                match client.receive_command(epoch)? {
                    RequestEvent::Stop => {
                        if mode == "stop_stall" {
                            stall();
                        }
                        if let Some(config) = config.as_mut() {
                            config.close()?;
                        }
                        return Ok(());
                    }
                    RequestEvent::Challenge => {
                        if mode != "standby" {
                            epoch += 1;
                        }
                    }
                    RequestEvent::Pending => (),
                }
                if mode != "standby" {
                    client.publish(epoch, ProgressPhase::Active)?;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })();
        // Never format errors on protocol stderr; Stop mints no fixture epoch.
        std::process::exit(if result.is_ok() { 0 } else { 21 });
    }
    let transfer = args.len() == 1 && args[0] == "--config-transfer";
    if !args.is_empty() && !transfer {
        return Err("invalid fixture args".into());
    }
    let executable = std::env::current_exe()?;
    for mode in [
        "initial",
        "active",
        "standby",
        "stop_stall",
        "startup_stall",
    ] {
        let start = Instant::now();
        let policy = Policy {
            startup_ns: 3_000_000_000,
            lease_ns: 200_000_000,
        };
        let (mut supervisor, config_path) = if transfer {
            let config = ConfigArtifact::create(&std::env::temp_dir(), IMAGE)?;
            let path = config.path()?.to_path_buf();
            (
                Supervisor::spawn_with_config(
                    &executable,
                    &[mode.into()],
                    43,
                    policy,
                    MonoTime(0),
                    config,
                )?,
                Some(path),
            )
        } else {
            (
                Supervisor::spawn(&executable, &[mode.into()], 43, policy, MonoTime(0))?,
                None,
            )
        };
        let mut requested = false;
        if mode == "initial" {
            supervisor.request_shutdown(MonoTime(0))?;
            requested = true;
        }
        let status = loop {
            if start.elapsed() > Duration::from_secs(15) {
                return Err(format!("{mode} did not exit: {:?}", supervisor.status()).into());
            }
            let now = MonoTime(u64::try_from(start.elapsed().as_nanos())?);
            let status = supervisor.poll(now)?;
            if matches!(status, SupervisorStatus::Exited { .. }) {
                break status;
            }
            if !requested
                && (supervisor.progress_epoch() > 0
                    || matches!(mode, "standby" | "startup_stall")
                        && start.elapsed() >= Duration::from_millis(100))
            {
                supervisor.request_shutdown(now)?;
                requested = true;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let SupervisorStatus::Exited { code, reason } = status else {
            return Err("missing exit".into());
        };
        let forced = matches!(mode, "stop_stall" | "startup_stall");
        let expected = if forced {
            Some(TerminationReason::Progress(
                keyweave_progress::ProgressError::Expired,
            ))
        } else {
            Some(TerminationReason::Cooperative)
        };
        if !requested || reason != expected || code != if forced { 0xe04b5701 } else { 0 } {
            return Err(format!("{mode}: {status:?} requested={requested}").into());
        }
        if matches!(mode, "initial" | "standby" | "startup_stall")
            && supervisor.progress_epoch() != 0
        {
            return Err("stop minted progress".into());
        }
        let report = supervisor.close_resources()?;
        if !report.complete() {
            return Err("incomplete resource cleanup".into());
        }
        if let Some(path) = config_path {
            if !report.config?.is_some_and(|r| r.complete()) {
                return Err("config writer cleanup absent".into());
            }
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match std::fs::metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) => return Err(error.into()),
                    Ok(_) => {
                        if Instant::now() >= deadline {
                            return Err("config deletion timeout".into());
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
        println!(
            "cooperative_case={mode} config_transfer={transfer} passed=true observed_exit={code} fixture_epoch={} input_apis=none",
            supervisor.progress_epoch()
        );
    }
    println!(
        "cooperative=passed owned_job=true actual_exit_confirmed=true lease_not_extended=true input_apis=none"
    );
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("check_cooperative requires Windows; no child created");
}
