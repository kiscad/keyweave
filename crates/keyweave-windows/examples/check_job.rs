//! No-input native process smoke: creates only this executable as a fixture.
//! Never installs a Hook, sends input, or opens a device/driver.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyweave_windows::{JobProcessState, OwnedJobProcess};
    use std::{
        ffi::OsString,
        time::{Duration, Instant},
    };

    fn expected() -> Vec<OsString> {
        [
            "--fixture-exit-259",
            "",
            "a b",
            "a\"b",
            "c\\",
            "slash\\\"quote",
            "中文🙂",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments == expected() {
        std::process::exit(259);
    }
    if arguments == [OsString::from("--fixture-idle")] {
        loop {
            std::thread::park();
        }
    }
    if arguments.len() == 2 && arguments[0] == "--fixture-ready-idle" {
        use std::io::Write;
        // Exclusive temporary marker supplied by the owning test. No overwrite.
        let mut ready = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&arguments[1])?;
        writeln!(ready, "{}", std::process::id())?;
        ready.sync_all()?;
        drop(ready);
        loop {
            std::thread::park();
        }
    }
    if arguments.len() == 2 && arguments[0] == "--fixture-parent-death" {
        let executable = std::env::current_exe()?;
        let mut child = OwnedJobProcess::spawn_suspended(
            &executable,
            &["--fixture-ready-idle".into(), arguments[1].clone()],
        )?;
        child.resume()?;
        // Test observer abruptly terminates this fixture parent. No Drop runs;
        // the kernel closes its sole Job handle and terminates the worker.
        loop {
            std::thread::park();
        }
    }
    if !arguments.is_empty() {
        return Err("invalid fixture arguments".into());
    }
    fn wait(child: &mut OwnedJobProcess) -> Result<u32, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(code) = child.poll_exit(Duration::from_millis(20))? {
                return Ok(code);
            }
        }
        child.request_stop()?;
        Err("owned child did not exit within the smoke deadline".into())
    }
    let executable = std::env::current_exe()?;
    let mut child = OwnedJobProcess::spawn_suspended(&executable, &expected())?;
    if child.state() != JobProcessState::Suspended || child.poll_exit(Duration::ZERO)?.is_some() {
        return Err("fixture was not suspended".into());
    }
    child.resume()?;
    if wait(&mut child)? != 259 {
        return Err("argv/exit fixture failed".into());
    }
    drop(child);

    let mut child = OwnedJobProcess::spawn_suspended(&executable, &["--fixture-idle".into()])?;
    child.resume()?;
    child.request_stop()?;
    if child.state() != JobProcessState::StopRequested {
        return Err("termination request was confused with exit".into());
    }
    if wait(&mut child)? != 0xe04b5701 {
        return Err("own Job termination failed".into());
    }
    println!(
        "job_smoke=passed argv=roundtrip exit_259=observed own_job_stop=observed input_apis=none"
    );
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("check_job requires Windows; no process fixture was created");
}
