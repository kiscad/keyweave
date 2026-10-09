//! No-input inheritance smoke. Fixture child exits without reading progress;
//! verifies endpoint lifetime transfer, not cross-process watchdog receipts.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyweave_progress::Challenge;
    use keyweave_windows::{JobProcessState, OwnedJobProcess, PipeReader};
    use std::{
        io::{self, Read, Write},
        time::{Duration, Instant},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == [std::ffi::OsString::from("--fixture-exit")] {
        std::process::exit(259);
    }
    if !args.is_empty() {
        return Err("invalid fixture arguments".into());
    }
    let (reader, mut parent_writer) = PipeReader::pair()?;
    let (mut parent_reader, writer) = PipeReader::pair()?;
    let mut child = OwnedJobProcess::spawn_suspended_with_progress(
        &std::env::current_exe()?,
        &["--fixture-exit".into()],
        reader,
        writer,
    )?;
    if child.state() != JobProcessState::Suspended || !child.finish_channel_transfer()?.complete() {
        return Err("child not suspended after channel transfer".into());
    }
    let challenge = Challenge { run_id: 42, id: 1 }.encode();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut sent = 0;
    while sent < challenge.len() {
        if Instant::now() >= deadline {
            return Err("suspended inherited reader missing".into());
        }
        match parent_writer.write(&challenge[sent..]) {
            Ok(0) => return Err("zero-length challenge write".into()),
            Ok(n) => sent += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(e) => return Err(e.into()),
        }
    }
    // Parent worker-end owners were consumed and closed; child writer keeps the
    // reply pipe alive while its primary thread is still suspended.
    let suspended_check = Instant::now() + Duration::from_millis(50);
    while Instant::now() < suspended_check {
        match parent_reader.read(&mut [0u8; 40]) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
            _ => return Err("suspended inherited writer missing or unexpected bytes".into()),
        }
        if child.poll_exit(Duration::ZERO)?.is_some() {
            return Err("suspended child exited".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    child.resume()?;
    loop {
        if Instant::now() >= deadline {
            return Err("owned fixture exit timed out".into());
        }
        if let Some(code) = child.poll_exit(Duration::from_millis(20))? {
            if code != 259 {
                return Err(format!("fixture exit {code}").into());
            }
            break;
        }
    }
    loop {
        if Instant::now() >= deadline {
            return Err("inherited writer EOF timed out".into());
        }
        match parent_reader.read(&mut [0u8; 40]) {
            Ok(0) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1))
            }
            _ => return Err("unexpected fixture reply".into()),
        }
    }
    if !parent_reader.close()?.complete() || !parent_writer.close()?.complete() {
        return Err("parent channel cleanup incomplete".into());
    }
    println!(
        "job_progress=passed suspended_reader=alive suspended_writer=alive exit_259=observed eof=observed input_apis=none"
    );
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("check_job_progress requires Windows; no process created");
}
