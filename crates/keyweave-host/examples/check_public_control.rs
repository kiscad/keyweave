//! Actual host/CLI with a fixed no-input fixture. Never starts capture worker.
#![forbid(unsafe_code)]
use keyweave_host::public_control::WindowsEndpoint;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitCode, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const LIMIT: u64 = 65_536;
fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn require(value: bool, message: &str) -> io::Result<()> {
    if value { Ok(()) } else { Err(error(message)) }
}
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        // Only this owned process handle. No PID lookup/foreign process kill.
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
    }
}
fn command(executable: &Path) -> Command {
    let command = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(0x0800_0000);
        command
    }
    #[cfg(not(windows))]
    {
        command
    }
}
fn wait(child: &mut Owned, limit: Duration) -> io::Result<ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= limit {
            return Err(error(
                "owned process actual exit not observed before deadline",
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
}
fn create(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}
fn drain(
    receiver: &Receiver<io::Result<String>>,
    log: &mut File,
    text: &mut String,
) -> io::Result<bool> {
    match receiver.try_recv() {
        Ok(line) => {
            let line = line?;
            log.write_all(line.as_bytes())?;
            text.push_str(&line);
            Ok(false)
        }
        Err(mpsc::TryRecvError::Disconnected) => Ok(true),
        Err(mpsc::TryRecvError::Empty) => Ok(false),
    }
}
fn local_absolute(path: &Path) -> bool {
    #[cfg(windows)]
    {
        matches!(path.components().next(),Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(),std::path::Prefix::Disk(_)|std::path::Prefix::VerbatimDisk(_)))
            && path.is_absolute()
    }
    #[cfg(not(windows))]
    {
        path.is_absolute()
    }
}
fn metadata(line: &str) -> io::Result<(WindowsEndpoint, u64)> {
    require(line.len() <= 1024, "oversized endpoint line")?;
    let value = line
        .strip_prefix("control_endpoint=")
        .ok_or_else(|| error("missing endpoint prefix"))?;
    let (endpoint, session) = value
        .trim_end_matches(['\r', '\n'])
        .split_once(" public_session=")
        .ok_or_else(|| error("missing public session"))?;
    let endpoint = endpoint.parse().map_err(error)?;
    require(
        !session.is_empty() && session.bytes().all(|b| b.is_ascii_digit()),
        "invalid public session",
    )?;
    let session = session
        .parse::<u64>()
        .map_err(|_| error("public session overflow"))?;
    require(session != 0, "zero public session")?;
    Ok((endpoint, session))
}
fn lines(input: impl Read + Send + 'static) -> Receiver<io::Result<String>> {
    let (send, receive) = mpsc::sync_channel(16);
    thread::spawn(move || {
        let mut reader = BufReader::new(input.take(LIMIT + 1));
        let mut total = 0;
        loop {
            let mut line = String::new();
            let result = reader.read_line(&mut line);
            match result {
                Ok(0) => break,
                Ok(count) => {
                    total += count;
                    if total as u64 > LIMIT {
                        let _ = send.send(Err(error("oversized host stdout")));
                        break;
                    }
                    if send.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = send.send(Err(error));
                    break;
                }
            }
        }
    });
    receive
}
fn run_client(
    host: &Path,
    directory: &Path,
    name: &str,
    args: &[String],
    expected: i32,
) -> io::Result<String> {
    let mut out = create(&directory.join(format!("{name}.stdout.log")))?;
    let mut err = create(&directory.join(format!("{name}.stderr.log")))?;
    let mut child = Owned(
        command(host)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let out_rx = lines(
        child
            .0
            .stdout
            .take()
            .ok_or_else(|| error("missing client stdout"))?,
    );
    let err_rx = lines(
        child
            .0
            .stderr
            .take()
            .ok_or_else(|| error("missing client stderr"))?,
    );
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut status = None;
    let until = Instant::now() + Duration::from_secs(4);
    loop {
        let out_done = drain(&out_rx, &mut out, &mut stdout)?;
        let err_done = drain(&err_rx, &mut err, &mut stderr)?;
        if status.is_none() {
            status = child.0.try_wait()?;
        }
        if let Some(status) = status
            && out_done
            && err_done
        {
            require(
                status.code() == Some(expected),
                &format!(
                    "{name}: unexpected client exit {status}; stdout={stdout}; stderr={stderr}"
                ),
            )?;
            return Ok(stdout);
        }
        require(
            Instant::now() < until,
            "client exit/output EOF not observed before deadline",
        )?;
        thread::sleep(Duration::from_millis(2));
    }
}

fn status_args(endpoint: &WindowsEndpoint) -> Vec<String> {
    vec![
        "status-windows".into(),
        "--endpoint".into(),
        endpoint.to_string(),
    ]
}
fn stop_args(endpoint: &WindowsEndpoint, session: u64) -> Vec<String> {
    vec![
        "stop-windows".into(),
        "--endpoint".into(),
        endpoint.to_string(),
        "--session".into(),
        session.to_string(),
    ]
}
fn starting(text: &str) -> io::Result<()> {
    require(
        text.contains("outcome=Status")
            && text.contains("phase=Starting")
            && text.contains("progress_epoch=0,")
            && text.contains("last_observed_generation=None")
            && text.contains("worker_exit=None")
            && text.contains("recovery_confirmed=false"),
        "no-input host did not report unready/epoch0/no exit",
    )
}
fn execute(args: Vec<PathBuf>) -> io::Result<()> {
    require(
        args.len() == 3,
        "usage: check_public_control <host.exe> <check_host_worker.exe> <existing local validation directory>",
    )?;
    require(
        args.iter().all(|p| local_absolute(p)),
        "absolute fixture paths required",
    )?;
    let host = args[0].canonicalize()?;
    let fixture = args[1].canonicalize()?;
    let root = args[2].canonicalize()?;
    require(
        [&host, &fixture, &root]
            .into_iter()
            .all(|p| local_absolute(p)),
        "Windows-local drive paths required",
    )?;
    require(
        host.file_name() == Some(std::ffi::OsStr::new("keyweave-host.exe"))
            && fixture.file_name() == Some(std::ffi::OsStr::new("check_host_worker.exe")),
        "only fixed host/no-input fixture names accepted; never capture worker",
    )?;
    require(
        host.is_file() && fixture.is_file() && root.is_dir(),
        "fixture files/existing directory required",
    )?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("system clock before epoch"))?
        .as_nanos()
        ^ (u128::from(std::process::id()) << 64);
    let directory = root.join(format!("public-control-{nonce:032x}"));
    fs::create_dir(&directory)?;
    println!("PublicControlCasesRoot={}", directory.display());
    let config = directory.join("synthetic.jsonc");
    create(&config)?.write_all(
        br#"{"profiles":[{"name":"no-input fixture","selected":true,"simple_modifications":[]}]}"#,
    )?;
    let pipe = format!("\\\\.\\pipe\\keyweave-control-{nonce:032x}");
    let host_stderr = directory.join("host.stderr.log");
    let mut child = Owned(
        command(&host)
            .args(["run-windows", "--identity", "strict", "--config"])
            .arg(&config)
            .arg("--worker")
            .arg(&fixture)
            .arg("--artifact-directory")
            .arg(&directory)
            .arg("--control-pipe")
            .arg(&pipe)
            .env("KEYWEAVE_HOST_FIXTURE_MODE", "public_control")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    // Keep stdin open throughout; only the actual public Stop causes shutdown.
    let output = child
        .0
        .stdout
        .take()
        .ok_or_else(|| error("missing owned host stdout"))?;
    let receive = lines(output);
    let error_output = child
        .0
        .stderr
        .take()
        .ok_or_else(|| error("missing owned host stderr"))?;
    let error_receive = lines(error_output);
    let mut error_log = create(&host_stderr)?;
    let mut error_text = String::new();
    let until = Instant::now() + Duration::from_secs(5);
    let mut host_output = String::new();
    let mut host_output_log = create(&directory.join("host.stdout.log"))?;
    let (endpoint, session) = loop {
        let _ = drain(&error_receive, &mut error_log, &mut error_text)?;
        match receive.try_recv() {
            Ok(line) => {
                let line = line?;
                host_output.push_str(&line);
                host_output_log.write_all(line.as_bytes())?;
                if line.starts_with("control_endpoint=") {
                    break metadata(&line)?;
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(error("host stdout ended before endpoint publication"));
            }
            Err(mpsc::TryRecvError::Empty) => (),
        }
        require(
            Instant::now() < until,
            "host endpoint publication timed out",
        )?;
        require(
            child.0.try_wait()?.is_none(),
            "host exited before endpoint publication",
        )?;
        thread::sleep(Duration::from_millis(2));
    };
    require(
        endpoint.pipe() == pipe,
        "published endpoint does not match selected pipe",
    )?;
    starting(&run_client(
        &host,
        &directory,
        "status-initial",
        &status_args(&endpoint),
        0,
    )?)?;
    let changed = endpoint
        .creation_time()
        .checked_add(1)
        .ok_or_else(|| error("creation time exhausted"))?;
    let wrong =
        WindowsEndpoint::new(endpoint.pipe(), endpoint.process_id(), changed).map_err(error)?;
    let rejected = run_client(&host, &directory, "wrong-instance", &status_args(&wrong), 1)?;
    require(
        rejected.is_empty(),
        "wrong instance produced a successful observation",
    )?;
    let stale = if session == u64::MAX { 1 } else { session + 1 };
    let text = run_client(
        &host,
        &directory,
        "stale-stop",
        &stop_args(&endpoint, stale),
        1,
    )?;
    require(
        text.contains("outcome=StaleSession")
            && text.contains("phase=Starting")
            && text.contains("progress_epoch=0,"),
        "stale Stop not rejected without progress/stop",
    )?;
    starting(&run_client(
        &host,
        &directory,
        "status-after-rejections",
        &status_args(&endpoint),
        0,
    )?)?;
    require(
        child.0.try_wait()?.is_none(),
        "rejected requests terminated owned host",
    )?;
    let ack = run_client(
        &host,
        &directory,
        "valid-stop",
        &stop_args(&endpoint, session),
        0,
    )?;
    require(
        ack.contains("outcome=StopAccepted")
            && ack.contains("phase=StopRequested")
            && ack.contains("worker_exit=None")
            && ack.contains("progress_epoch=0,")
            && ack.contains("recovery_confirmed=false"),
        "Stop acceptance fabricated exit/progress/recovery",
    )?;
    let until = Instant::now() + Duration::from_secs(3);
    let mut observation = 0u32;
    loop {
        let final_status = run_client(
            &host,
            &directory,
            &format!("status-exited-{observation}"),
            &status_args(&endpoint),
            0,
        )?;
        require(
            final_status.contains("progress_epoch=0,")
                && final_status.contains("recovery_confirmed=false"),
            "final status fabricated progress/recovery",
        )?;
        if final_status.contains("phase=Exited") {
            require(
                final_status.contains("worker_exit=Some(Code(0))"),
                "final actual exit code absent",
            )?;
            break;
        }
        require(
            final_status.contains("phase=StopRequested")
                && final_status.contains("worker_exit=None"),
            "invalid intermediate Stop observation",
        )?;
        require(
            Instant::now() < until,
            "actual Exited snapshot not observed in bounded final service window",
        )?;
        observation = observation
            .checked_add(1)
            .ok_or_else(|| error("observation counter exhausted"))?;
        thread::sleep(Duration::from_millis(2));
    }
    require(
        wait(&mut child, Duration::from_secs(6))?.code() == Some(0),
        "owned host failed natural public Stop",
    )?;
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        let out_done = drain(&receive, &mut host_output_log, &mut host_output)?;
        let err_done = drain(&error_receive, &mut error_log, &mut error_text)?;
        if out_done && err_done {
            break;
        }
        require(Instant::now() < until, "host output EOF unconfirmed")?;
        thread::sleep(Duration::from_millis(2));
    }
    require(host_output.contains("worker_exit=Some(0), reason=Some(Cooperative), resources_complete=true, progress_epoch=0,"),"actual owned exit/cooperative cleanup/zero epoch not confirmed")?;
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let present = fs::read_dir(&directory)?
            .collect::<io::Result<Vec<_>>>()?
            .iter()
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("keyweave-config-")
            });
        if !present {
            break;
        }
        require(
            Instant::now() < until,
            "sealed fixture config deletion unconfirmed",
        )?;
        thread::sleep(Duration::from_millis(2));
    }
    create(&directory.join("PASSED.txt"))?.write_all(b"actual host/CLI/pipe/owned no-input worker; zero progress; synthetic fixture only, no hardware recovery proof\n")?;
    println!("Public control no-input gate passed; source directory and logs retained");
    Ok(())
}
fn main() -> ExitCode {
    if !cfg!(windows) {
        eprintln!("Windows native no-input control gate requires Windows");
        return ExitCode::from(78);
    }
    match execute(
        std::env::args_os()
            .skip(1)
            .take(4)
            .map(PathBuf::from)
            .collect(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("public control native gate failed; cleanup/exit not inferred: {error}");
            ExitCode::from(1)
        }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_parser_keeps_full_width_and_rejects_noise_zero_or_extra_fields() {
        let pipe = r"\\.\pipe\keyweave-control-0123456789abcdef0123456789abcdef";
        let line = format!(
            "control_endpoint={pipe}|4294967295|18446744073709551615 public_session=18446744073709551615\r\n"
        );
        let (endpoint, session) = metadata(&line).unwrap();
        assert_eq!(endpoint.creation_time(), u64::MAX);
        assert_eq!(session, u64::MAX);
        for bad in [
            format!("control_endpoint={pipe}|42|1 public_session=0"),
            format!("control_endpoint={pipe}|42|1 public_session=2 extra"),
            format!("control_endpoint={pipe}|42|1 public_session=+2"),
            "x".repeat(1025),
        ] {
            assert!(metadata(&bad).is_err());
        }
    }
    #[test]
    fn diagnostic_reader_is_bounded_and_preserves_lines_without_waiting_for_native_processes() {
        let receive = lines(std::io::Cursor::new(b"first\nsecond\n".to_vec()));
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap(),
            "first\n"
        );
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap(),
            "second\n"
        );
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        let receive = lines(std::io::Cursor::new(vec![b'x'; LIMIT as usize + 1]));
        assert!(
            receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("oversized")
        );
    }
}
