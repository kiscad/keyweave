//! Own private console + protocol fixture only. No Hook/SendInput/real keyboard.
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use keyweave_windows::{keys::IdentityPolicy, with_private_console_event};
    use keyweave_worker::preparation::PreparedConfig;
    use std::{path::PathBuf, sync::atomic::AtomicBool};
    let mut args: Vec<_> = std::env::args_os().skip(1).take(7).collect();
    let detached = args.first().is_some_and(|a| a == "--detached-probe");
    if detached {
        args.remove(0);
    }
    if args.len() != 5 {
        return std::process::ExitCode::from(20);
    }
    let event = match args[0].to_str() {
        Some("0") => 0,
        Some("1") => 1,
        _ => return std::process::ExitCode::from(21),
    };
    let host_fixture = PathBuf::from(&args[1]);
    let config = PathBuf::from(&args[2]);
    let directory = PathBuf::from(&args[3]);
    let report_path = PathBuf::from(&args[4]);
    if ![&host_fixture, &config, &directory, &report_path]
        .into_iter()
        .all(|p| p.is_absolute())
    {
        return std::process::ExitCode::from(22);
    }
    // This fixed example is the only executable accepted, never capture worker.
    if host_fixture.file_name() != Some(std::ffi::OsStr::new("check_host_worker.exe")) {
        return std::process::ExitCode::from(23);
    }
    if !detached {
        // CREATE_NO_WINDOW may still have a hidden console. A self-spawn with
        // DETACHED_PROCESS starts the actual probe with no console; no existing
        // console is ever freed, borrowed or broadcast to by this launcher.
        return match launch_detached(&args) {
            Ok(code) => std::process::ExitCode::from(code),
            Err(error) => {
                let _ = std::fs::write(&report_path, format!("detached launcher: {error}"));
                std::process::ExitCode::from(26)
            }
        };
    }
    let prepared = match std::fs::File::open(&config)
        .map_err(|e| e.to_string())
        .and_then(|f| PreparedConfig::read(f).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::write(&report_path, e);
            return std::process::ExitCode::from(24);
        }
    };
    let probe = with_private_console_event(event, |console| {
        keyweave_host::windows::run_with_console(
            &host_fixture,
            &config,
            &directory,
            prepared,
            IdentityPolicy::StrictUnique,
            &AtomicBool::new(false),
            console,
        )
    });
    let success = probe.as_ref().is_ok_and(|p| {
        p.complete()
            && p.body
                .as_ref()
                .is_ok_and(|r| r.successful() && !r.startup_cancelled && r.worker_exit == Some(0))
    });
    let text = match &probe {
        Ok(p) => format!(
            "event={event}; delivery={:?}; unregister={:?}; release={:?}; body={:?}",
            p.delivery, p.unregister, p.release, p.body
        ),
        Err(e) => format!("event={event}; admission={e}"),
    };
    if std::fs::write(&report_path, text).is_err() {
        return std::process::ExitCode::from(25);
    }
    std::process::ExitCode::from(if success { 0 } else { 1 })
}
#[cfg(windows)]
fn launch_detached(args: &[std::ffi::OsString]) -> std::io::Result<u8> {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--detached-probe")
        .args(args)
        .creation_flags(8)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    let mut failure = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if failure.is_none() => {
                return Ok(status
                    .code()
                    .and_then(|c| u8::try_from(c).ok())
                    .unwrap_or(1));
            }
            Ok(Some(_)) => return Err(failure.unwrap_or_else(|| std::io::ErrorKind::Other.into())),
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
            Ok(None) => (),
        }
        if failure.is_some() || start.elapsed() >= Duration::from_secs(10) {
            if failure.is_none() {
                failure = Some(std::io::ErrorKind::TimedOut.into());
            }
            // Child's retained process handle, never an arbitrary PID. Its own
            // kill-on-close Job contains the detached no-input protocol worker.
            let _ = child.kill();
            if start.elapsed() >= Duration::from_secs(12) {
                return Err(failure.unwrap_or_else(|| std::io::ErrorKind::TimedOut.into()));
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
