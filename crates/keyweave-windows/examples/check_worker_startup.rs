//! Real protected child admission and protocol transport. No input APIs;
//! progress epochs are explicit fixture values, not CaptureSession liveness.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyweave_progress::{
        Challenge, Policy, ProgressClient, ProgressPhase, ProgressWatchdog, Receipt,
    };
    use keyweave_types::MonoTime;
    use keyweave_windows::{OwnedJobProcess, PipeReader, WorkerStartup, WorkerStartupError};
    use std::{
        io::{self, Read, Write},
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 3 && matches!(args[0].to_str(), Some("--worker" | "--direct-denied")) {
        let parent = args[1]
            .to_str()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let run = args[2]
            .to_str()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        // Real main: no input, std IO or other executing threads before claim.
        let startup = match WorkerStartup::claim(parent, run) {
            Ok(v) => v,
            Err(_) => std::process::exit(17),
        };
        if args[0] == "--direct-denied" {
            std::process::exit(18);
        }
        if !matches!(
            WorkerStartup::claim(parent, run),
            Err(WorkerStartupError::AlreadyClaimed)
        ) {
            std::process::exit(18);
        }
        let (_guard, run, reader, writer) = startup.into_parts();
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut client = ProgressClient::new(reader, writer, run)?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while !client.receive(10)? {
                if Instant::now() >= deadline {
                    return Err("receive timeout".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if client.publish(10, ProgressPhase::Active)? {
                return Err("frozen epoch published".into());
            }
            let mut sent = client.publish(11, ProgressPhase::Active)?;
            while !sent {
                if Instant::now() >= deadline {
                    return Err("send timeout".into());
                }
                sent = client.flush()?;
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        })();
        // stderr is binary protocol: exit codes, not runtime error formatting.
        std::process::exit(if result.is_ok() { 259 } else { 18 });
    }
    if !args.is_empty() {
        return Err("invalid fixture args".into());
    }
    keyweave_windows::check_synchronous_progress_pipe_rejected()?;
    let executable = std::env::current_exe()?;
    let parent = std::process::id();
    let run = 42;
    let (reader, mut parent_writer) = PipeReader::pair()?;
    let (mut parent_reader, writer) = PipeReader::pair()?;
    let mut child = OwnedJobProcess::spawn_suspended_with_progress(
        &executable,
        &[
            "--worker".into(),
            parent.to_string().into(),
            run.to_string().into(),
        ],
        reader,
        writer,
    )?;
    child.resume()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let challenge = Challenge { run_id: run, id: 1 }.encode();
    let mut sent = 0;
    let mut bytes = [0; 40];
    let mut read = 0;
    while sent < challenge.len() || read < bytes.len() {
        if Instant::now() >= deadline {
            return Err("protected child protocol timed out".into());
        }
        if sent < challenge.len() {
            match parent_writer.write(&challenge[sent..]) {
                Ok(0) => return Err("zero write".into()),
                Ok(n) => sent += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) => return Err(e.into()),
            }
        }
        if read < bytes.len() {
            match parent_reader.read(&mut bytes[read..]) {
                Ok(0) => {
                    return Err("child exited without receipt (startup/transport rejection)".into());
                }
                Ok(n) => read += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) => return Err(e.into()),
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut watchdog = ProgressWatchdog::new(run, Policy::default(), MonoTime(0))?;
    watchdog.observe(Receipt::decode(&bytes)?, MonoTime(1))?;
    if watchdog.epoch() != 11 {
        return Err("bad fixture epoch".into());
    }
    loop {
        if Instant::now() >= deadline {
            return Err("child exit timed out".into());
        }
        if let Some(code) = child.poll_exit(Duration::from_millis(20))? {
            if code != 259 {
                return Err(format!("child exit {code}").into());
            }
            break;
        }
    }
    loop {
        match parent_reader.read(&mut [0u8; 40]) {
            Ok(0) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("EOF timed out".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            _ => return Err("unexpected EOF bytes".into()),
        }
    }
    if !parent_reader.close()?.complete() || !parent_writer.close()?.complete() {
        return Err("parent cleanup failed".into());
    }
    // Incorrect creating-parent argument must fail even with correct Job/pipes.
    let (reader, mut wrong_writer) = PipeReader::pair()?;
    let (mut wrong_reader, writer) = PipeReader::pair()?;
    let wrong_parent = parent.checked_add(1).ok_or("PID bound")?;
    let mut wrong = OwnedJobProcess::spawn_suspended_with_progress(
        &executable,
        &[
            "--worker".into(),
            wrong_parent.to_string().into(),
            run.to_string().into(),
        ],
        reader,
        writer,
    )?;
    wrong.resume()?;
    let wrong_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if Instant::now() >= wrong_deadline {
            return Err("wrong-parent child timeout".into());
        }
        if let Some(code) = wrong.poll_exit(Duration::from_millis(20))? {
            if code != 17 {
                return Err("wrong parent admitted".into());
            }
            break;
        }
    }
    if !wrong_reader.close()?.complete() || !wrong_writer.close()?.complete() {
        return Err("wrong-parent cleanup failed".into());
    }
    // Own ordinary process launch with NUL std streams, no protected contract.
    let mut direct = std::process::Command::new(&executable)
        .args(["--direct-denied", &parent.to_string(), &run.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let direct_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = direct.try_wait()? {
            if status.code() != Some(17) {
                return Err("direct launch admitted".into());
            }
            break;
        }
        if Instant::now() >= direct_deadline {
            direct.kill()?;
            direct.wait()?;
            return Err("direct child timeout".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    println!(
        "worker_startup=passed protected_child=admitted cross_process_receipt=40 fixture_epoch=11 frozen=rejected duplicate_claim=rejected synchronous=rejected wrong_parent=rejected direct_launch=rejected exit_259=observed eof=observed input_apis=none"
    );
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("check_worker_startup requires Windows; no child created");
}
