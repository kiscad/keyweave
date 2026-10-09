//! Owned config file-object transfer only. Never install Hook/SendInput.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyweave_progress::{Challenge, ProgressClient, ProgressPhase, Receipt};
    use keyweave_windows::{
        ConfigArtifact, OwnedJobProcess, PipeReader, WorkerStartup, WorkerStartupError,
    };
    use std::{
        io::{self, Read, Write},
        time::{Duration, Instant},
    };
    // Keep ordinary Rust panic formatting out of child binary stderr.
    std::panic::set_hook(Box::new(|_| {}));
    const IMAGE: &[u8] = b"synthetic owned config image\0\xff\r\n";
    let args: Vec<_> = std::env::args_os().skip(1).take(6).collect();
    if args.len() == 5 && args[3] == "--keyweave-config-handle" {
        // Initial real main: no threads/file IO/std IO/input before admission.
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let parent = args[1].to_str().ok_or("parent")?.parse()?;
            let value = args[4].to_str().ok_or("handle")?.parse()?;
            let mut startup = WorkerStartup::claim(parent, 42)?;
            if args[2] == "reject" {
                if !matches!(
                    startup.claim_config_reader(0),
                    Err(WorkerStartupError::ConfigHandle)
                ) {
                    return Err("bad config handle admitted".into());
                }
                return Ok(());
            }
            let mut config = startup.claim_config_reader(value)?;
            if !matches!(
                startup.claim_config_reader(value),
                Err(WorkerStartupError::ConfigClaimed)
            ) {
                return Err("duplicate config claim".into());
            }
            let mut bytes = Vec::new();
            (&mut config)
                .take(IMAGE.len() as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes != IMAGE {
                return Err("different file object bytes".into());
            }
            let (_guard, run, reader, writer) = startup.into_parts();
            let mut client = ProgressClient::new(reader, writer, run)?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while !client.receive(0)? {
                if Instant::now() >= deadline {
                    return Err("challenge timeout".into());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut sent = client.publish(1, ProgressPhase::Active)?;
            while !sent {
                if Instant::now() >= deadline {
                    return Err("receipt timeout".into());
                }
                sent = client.flush()?;
                std::thread::sleep(Duration::from_millis(1));
            }
            if args[2] == "stall" {
                // Reader stays owned while parent tests forced owned-Job stop.
                while Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                return Err("stall was not terminated".into());
            }
            config.close()?;
            Ok(())
        })();
        // stderr is binary; ordinary Result-main error output is forbidden here.
        std::process::exit(if result.is_ok() { 0 } else { 65 });
    }
    if !args.is_empty() {
        return Err("fixture args".into());
    }
    for mode in ["read", "reject", "stall"] {
        let artifact = ConfigArtifact::create(&std::env::temp_dir(), IMAGE)?;
        let path = artifact.path()?.to_path_buf();
        let (reader, mut parent_writer) = PipeReader::pair()?;
        let (mut parent_reader, writer) = PipeReader::pair()?;
        let mut child = OwnedJobProcess::spawn_suspended_with_config(
            &std::env::current_exe()?,
            &[
                "--worker".into(),
                std::process::id().to_string().into(),
                mode.into(),
            ],
            reader,
            writer,
            artifact,
        )?;
        if std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .err()
            .and_then(|e| e.raw_os_error())
            != Some(32)
        {
            return Err("suspended config writer was not held".into());
        }
        if child.close_config().is_ok() {
            return Err("released config before exit".into());
        }
        child.resume()?;
        let deadline = Instant::now() + Duration::from_secs(15);
        if mode != "reject" {
            let frame = Challenge { run_id: 42, id: 1 }.encode();
            let mut sent = 0;
            let mut reply = [0; 40];
            let mut received = 0;
            while received < reply.len() {
                if Instant::now() >= deadline {
                    return Err("handoff receipt timeout".into());
                }
                if sent < frame.len() {
                    match parent_writer.write(&frame[sent..]) {
                        Ok(0) => return Err("zero write".into()),
                        Ok(n) => sent += n,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                        Err(e) => return Err(e.into()),
                    }
                }
                match parent_reader.read(&mut reply[received..]) {
                    Ok(0) => return Err("config admission/read failed".into()),
                    Ok(n) => received += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                    Err(e) => return Err(e.into()),
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let receipt = Receipt::decode(&reply)?;
            if receipt.challenge.run_id != 42 || receipt.challenge.id != 1 || receipt.epoch != 1 {
                return Err("receipt identity".into());
            }
        }
        if mode == "stall" {
            child.request_stop()?;
        }
        let code = loop {
            if Instant::now() >= deadline {
                return Err("owned child exit timeout".into());
            }
            if let Some(code) = child.poll_exit(Duration::from_millis(20))? {
                break code;
            }
        };
        if (mode == "stall" && code != 3763033857) || (mode != "stall" && code != 0) {
            return Err(format!("unexpected {mode} exit {code}").into());
        }
        if !child.close_config()?.is_some_and(|r| r.complete()) {
            return Err("writer cleanup".into());
        }
        if !parent_reader.close()?.complete() || !parent_writer.close()?.complete() {
            return Err("pipe cleanup".into());
        }
        let deletion_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::metadata(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
                Ok(_) if Instant::now() >= deletion_deadline => {
                    return Err("last-close deletion timeout".into());
                }
                Ok(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        println!(
            "config_handoff={mode} observed_exit={code} exact_owned_file=true deletion=observed input_apis=none"
        );
    }
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("check_config_handoff requires Windows; no files/processes opened");
}
