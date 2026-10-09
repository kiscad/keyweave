//! Real no-input transport/protocol roundtrip. Epochs are test fixture values;
//! this does not install CaptureSession or prove a runtime is making progress.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyweave_progress::{
        Challenge, Policy, ProgressClient, ProgressPhase, ProgressWatchdog, Receipt,
    };
    use keyweave_types::MonoTime;
    use keyweave_windows::PipeReader;
    use std::{
        io::{self, Read, Write},
        time::{Duration, Instant},
    };
    let (mut worker_reader, mut parent_writer) = PipeReader::pair()?;
    let (mut parent_reader, mut worker_writer) = PipeReader::pair()?;
    let mut watchdog = ProgressWatchdog::new(42, Policy::default(), MonoTime(0))?;
    let challenge = Challenge { run_id: 42, id: 1 }.encode();
    let deadline = Instant::now() + Duration::from_secs(5);
    {
        let mut client = ProgressClient::new(&mut worker_reader, &mut worker_writer, 42)?;
        let mut written = 0;
        let mut received = false;
        while written < challenge.len() || !received {
            if Instant::now() >= deadline {
                return Err("challenge roundtrip timed out".into());
            }
            if written < challenge.len() {
                match parent_writer.write(&challenge[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                    Err(e) => return Err(e.into()),
                }
            }
            if !received {
                received = client.receive(10)?;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        if client.publish(10, ProgressPhase::Active)? {
            return Err("fixture minted frozen progress".into());
        }
        let mut flushed = client.publish(11, ProgressPhase::Active)?;
        let mut bytes = [0u8; 40];
        let mut read = 0;
        while !flushed || read < bytes.len() {
            if Instant::now() >= deadline {
                return Err("receipt roundtrip timed out".into());
            }
            if !flushed {
                flushed = client.flush()?;
            }
            if read < bytes.len() {
                match parent_reader.read(&mut bytes[read..]) {
                    Ok(0) => return Err("premature receipt EOF".into()),
                    Ok(n) => read += n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                    Err(e) => return Err(e.into()),
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        watchdog.observe(Receipt::decode(&bytes)?, MonoTime(1))?;
        if watchdog.epoch() != 11 {
            return Err("receipt progress mismatch".into());
        }
    }
    for report in [
        worker_reader.close()?,
        worker_writer.close()?,
        parent_reader.close()?,
        parent_writer.close()?,
    ] {
        if !report.complete() {
            return Err(format!("pipe cleanup unresolved: {report:?}").into());
        }
    }
    let (mut reader, mut writer) = PipeReader::pair()?;
    match reader.read(&mut [0u8; 24]) {
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
        Err(e) => return Err(e.into()),
        Ok(_) => return Err("empty pipe did not pend".into()),
    }
    if !writer.close()?.complete() {
        return Err("EOF writer close failed".into());
    }
    loop {
        if Instant::now() >= deadline {
            return Err("EOF was not observed".into());
        }
        match reader.read(&mut [0u8; 24]) {
            Ok(0) => break,
            Ok(_) => return Err("unexpected EOF bytes".into()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(e) => return Err(e.into()),
        }
    }
    if !reader.close()?.complete() {
        return Err("EOF reader close failed".into());
    }
    let (mut reader, mut writer) = PipeReader::pair()?;
    match reader.read(&mut [0u8; 24]) {
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
        Err(e) => return Err(e.into()),
        Ok(_) => return Err("cancel fixture did not pend".into()),
    }
    if !reader.close()?.complete() || !writer.close()?.complete() {
        return Err("pending read cancellation unresolved".into());
    }
    println!(
        "progress_pipe=passed challenge=24 receipt=40 fixture_epoch=11 eof=observed cancel=completed input_apis=none"
    );
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("check_progress_pipe requires Windows; no native pipe was created");
}
