//! Own protected fixture processes only; no Hook/SendInput. Epochs are synthetic.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyweave_progress::{
        Challenge, Policy, ProgressClient, ProgressError, ProgressPhase, Receipt,
    };
    use keyweave_types::MonoTime;
    use keyweave_windows::{
        Supervisor, SupervisorStatus, TerminationReason, WORKER_ARGUMENT, WorkerStartup,
    };
    use std::{
        io::{self, Read, Write},
        time::{Duration, Instant},
    };
    fn start_background() {
        // Thread is deliberately independent of the blocked fixture event loop;
        // it never publishes epochs or touches the progress channel.
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let background = alive.clone();
        let _thread = std::thread::spawn(move || {
            loop {
                background.store(true, std::sync::atomic::Ordering::Release);
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        while !alive.load(std::sync::atomic::Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
    fn stall() -> ! {
        loop {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 4 && args[0] == WORKER_ARGUMENT {
        let parent = args[1].to_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        let run = args[2].to_str().and_then(|s| s.parse().ok()).unwrap_or(0);
        // Admission before threads, input or any binary std IO.
        let startup = match WorkerStartup::claim(parent, run) {
            Ok(v) => v,
            Err(_) => std::process::exit(17),
        };
        let (_guard, run, mut reader, mut writer) = startup.into_parts();
        let mode = args[3].to_str().unwrap_or("");
        if mode != "healthy" {
            start_background();
        }
        if mode == "startup_stall" {
            stall();
        }
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let deadline = Instant::now() + Duration::from_secs(10);
            if mode == "wrong_run" {
                let mut request = [0; 24];
                let mut n = 0;
                while n < request.len() {
                    if Instant::now() >= deadline {
                        return Err("child receive timeout".into());
                    }
                    match reader.read(&mut request[n..]) {
                        Ok(0) => return Err("child EOF".into()),
                        Ok(count) => n += count,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                        Err(e) => return Err(e.into()),
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                let challenge = Challenge::decode(&request)?;
                let bytes = Receipt {
                    challenge: Challenge {
                        run_id: challenge.run_id + 1,
                        ..challenge
                    },
                    baseline: 10,
                    epoch: 11,
                    phase: ProgressPhase::Active,
                }
                .encode();
                let mut n = 0;
                while n < bytes.len() {
                    if Instant::now() >= deadline {
                        return Err("child send timeout".into());
                    }
                    match writer.write(&bytes[n..]) {
                        Ok(0) => return Err("child zero write".into()),
                        Ok(count) => n += count,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                        Err(e) => return Err(e.into()),
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                stall();
            }
            let mut client = ProgressClient::new(reader, writer, run)?;
            for baseline in [10, 11] {
                while !client.receive(baseline)? {
                    if Instant::now() >= deadline {
                        return Err("child challenge timeout".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                let phase = if mode == "stopped_stall" || baseline == 11 {
                    ProgressPhase::Stopped
                } else {
                    ProgressPhase::Active
                };
                let mut sent = client.publish(baseline + 1, phase)?;
                while !sent {
                    if Instant::now() >= deadline {
                        return Err("child receipt timeout".into());
                    }
                    sent = client.flush()?;
                    std::thread::sleep(Duration::from_millis(1));
                }
                if mode != "healthy" {
                    stall();
                }
            }
            Ok(())
        })();
        // Protocol stderr must never contain error formatting.
        std::process::exit(if result.is_ok() { 259 } else { 18 });
    }
    if !args.is_empty() {
        return Err("invalid fixture arguments".into());
    }
    let executable = std::env::current_exe()?;
    for mode in [
        "healthy",
        "stall",
        "startup_stall",
        "wrong_run",
        "stopped_stall",
        "requested",
    ] {
        let start = Instant::now();
        let mut supervisor = Supervisor::spawn(
            &executable,
            &[mode.into()],
            42,
            Policy {
                startup_ns: 3_000_000_000,
                lease_ns: 200_000_000,
            },
            MonoTime(0),
        )?;
        let end = start + Duration::from_secs(12);
        let status = loop {
            if Instant::now() >= end {
                return Err(
                    format!("fixture {mode} did not exit: {:?}", supervisor.status()).into(),
                );
            }
            let now = MonoTime(u64::try_from(start.elapsed().as_nanos())?);
            let status = supervisor.poll(now)?;
            if mode == "requested"
                && supervisor.progress_epoch() > 0
                && status == SupervisorStatus::Running
            {
                supervisor.request_stop()?;
            }
            if matches!(status, SupervisorStatus::Exited { .. }) {
                break status;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let SupervisorStatus::Exited { code, reason } = status else {
            return Err("missing exit".into());
        };
        let expected = match mode {
            "healthy" => None,
            "wrong_run" => Some(TerminationReason::Progress(ProgressError::Identity)),
            "requested" => Some(TerminationReason::Requested),
            _ => Some(TerminationReason::Progress(ProgressError::Expired)),
        };
        if reason != expected || (mode == "healthy" && code != 259) {
            return Err(format!("fixture {mode}: {status:?}").into());
        }
        if mode == "startup_stall" && supervisor.progress_epoch() != 0 {
            return Err("startup stall minted progress".into());
        }
        if matches!(mode, "stall" | "stopped_stall" | "requested")
            && supervisor.progress_epoch() != 11
        {
            return Err("first actual fixture reply missing".into());
        }
        if !supervisor.close_channels()?.complete() {
            return Err("supervisor channel cleanup incomplete".into());
        }
        supervisor.request_stop()?;
        if supervisor.poll(MonoTime(0))? != status {
            return Err("cached exit changed".into());
        }
        println!(
            "supervisor_case={mode} passed=true observed_exit={code} fixture_epoch={} input_apis=none",
            supervisor.progress_epoch()
        );
    }
    println!(
        "supervisor=passed owned_job=true cross_process=true stalled_loop_with_background_thread=terminated stopped_not_exit=true stop_confirmed=true input_apis=none"
    );
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("check_supervisor requires Windows; no child created");
}
