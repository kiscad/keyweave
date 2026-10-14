//! Owned disposable metadata job. No capture/consumer grant or arbitrary PID control.
use keyweave_types::MonoTime;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    io::{self, PipeReader, Read},
    os::{fd::AsFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
pub const MAX_RESPONSE_BYTES: usize = 196608;
pub const MAX_PROBE_NS: u64 = 500_000_000;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeTarget {
    display: String,
    node: PathBuf,
    major: u32,
    minor: u32,
}
impl ProbeTarget {
    pub fn new(display: &str, node: &Path, major: u32, minor: u32) -> io::Result<Self> {
        let value = display
            .strip_prefix(':')
            .ok_or(io::ErrorKind::InvalidInput)?;
        let mut parts = value.split('.');
        let number = parts.next().unwrap_or("");
        let screen = parts.next();
        let digits = |value: &str| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u16>().is_ok()
        };
        let event = node
            .file_name()
            .and_then(|value| value.to_str())
            .and_then(|value| value.strip_prefix("event"));
        if display.len() > 64
            || !digits(number)
            || screen.is_some_and(|screen| !digits(screen))
            || parts.next().is_some()
            || node.parent() != Some(Path::new("/dev/input"))
            || !event.is_some_and(|event| {
                !event.is_empty()
                    && event.len() <= 10
                    && event.bytes().all(|byte| byte.is_ascii_digit())
            })
            || major != 13
            || minor == 0
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self {
            display: display.into(),
            node: node.into(),
            major,
            minor,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Keyboard,
    Pointer,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceMetadata {
    pub id: i32,
    pub attachment: i32,
    pub kind: DeviceKind,
    pub keys: Vec<i32>,
    pub buttons: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeReport {
    schema_version: u32,
    display: String,
    node: PathBuf,
    major: u32,
    minor: u32,
    pub devices: Vec<DeviceMetadata>,
}
pub fn decode_report(bytes: &[u8], target: &ProbeTarget) -> io::Result<ProbeReport> {
    if bytes.is_empty() || bytes.len() > MAX_RESPONSE_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let report: ProbeReport =
        serde_json::from_slice(bytes).map_err(|_| io::ErrorKind::InvalidData)?;
    if report.schema_version != 1
        || report.display != target.display
        || report.node != target.node
        || report.major != target.major
        || report.minor != target.minor
        || report.devices.len() > 16
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut ids = BTreeSet::new();
    for device in &report.devices {
        if device.id <= 0
            || device.attachment <= 0
            || device.id == device.attachment
            || !ids.insert(device.id)
            || device.keys.len() > 768
            || device.buttons > 256
            || device.keys.iter().any(|key| !(0..=65535).contains(key))
            || device.keys.windows(2).any(|keys| keys[0] >= keys[1])
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    Ok(report)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    Clock,
    Timeout,
    Requested,
    OutputLimit,
    Read,
    Exit,
    Protocol,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbePhase {
    Running,
    StopRequested,
    Exited,
}
pub struct ProbeJob {
    child: Child,
    reader: Option<PipeReader>,
    target: ProbeTarget,
    deadline: MonoTime,
    last: MonoTime,
    buffer: Vec<u8>,
    eof: bool,
    exit: Option<ExitStatus>,
    failure: Option<ProbeFailure>,
    kill_sent: bool,
    finished: bool,
    report: Option<ProbeReport>,
}
impl ProbeJob {
    /// Explicit trusted executable, no shell. Caller retains this owner through exit.
    pub fn spawn(
        image: &Path,
        target: ProbeTarget,
        started: MonoTime,
        timeout_ns: u64,
    ) -> io::Result<Self> {
        if !image.is_absolute() || timeout_ns == 0 || timeout_ns > MAX_PROBE_NS {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let deadline = started
            .checked_add(timeout_ns)
            .ok_or(io::ErrorKind::InvalidInput)?;
        let image = fs::canonicalize(image)?;
        let metadata = fs::metadata(&image)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let (reader, writer) = std::io::pipe()?;
        crate::ioctl::nonblocking(reader.as_fd())?;
        let mut command = Command::new(image);
        command
            .args(["--display", &target.display, "--node"])
            .arg(&target.node)
            .arg("--major")
            .arg(target.major.to_string())
            .arg("--minor")
            .arg(target.minor.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer))
            .stderr(Stdio::null());
        crate::ioctl::command_parent_death(&mut command, std::process::id())?;
        let child = command.spawn()?;
        Ok(Self {
            child,
            reader: Some(reader),
            target,
            deadline,
            last: started,
            buffer: vec![],
            eof: false,
            exit: None,
            failure: None,
            kill_sent: false,
            finished: false,
            report: None,
        })
    }
    pub fn phase(&self) -> ProbePhase {
        if self.finished {
            ProbePhase::Exited
        } else if self.failure.is_some() {
            ProbePhase::StopRequested
        } else {
            ProbePhase::Running
        }
    }
    pub fn failure(&self) -> Option<ProbeFailure> {
        self.failure
    }
    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.exit
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        if self.finished {
            None
        } else if self.failure.is_some() {
            Some(self.last)
        } else {
            Some(self.deadline)
        }
    }
    pub fn take_report(&mut self) -> Option<ProbeReport> {
        self.report.take()
    }
    fn stop(&mut self, reason: ProbeFailure) -> io::Result<()> {
        self.failure.get_or_insert(reason);
        self.report = None;
        self.buffer.clear();
        self.reader = None;
        if self.exit.is_some() {
            self.finished = true;
        }
        if self.exit.is_none() && !self.kill_sent {
            self.child.kill()?;
            self.kill_sent = true;
        }
        Ok(())
    }
    pub fn request_stop(&mut self) -> io::Result<()> {
        self.stop(ProbeFailure::Requested)
    }
    /// One nonblocking read and one observed child status per poll; no wait/restart.
    pub fn poll(&mut self, now: MonoTime) -> io::Result<ProbePhase> {
        if self.finished {
            return Ok(ProbePhase::Exited);
        }
        if now < self.last {
            self.stop(ProbeFailure::Clock)?;
        } else {
            self.last = now;
            if now >= self.deadline && self.failure.is_none() {
                self.stop(ProbeFailure::Timeout)?;
            }
        }
        if self.failure.is_none() && !self.eof {
            let mut bytes = [0u8; 4096];
            let result = self
                .reader
                .as_mut()
                .ok_or(io::ErrorKind::BrokenPipe)?
                .read(&mut bytes);
            match result {
                Ok(0) => {
                    self.eof = true;
                    self.reader = None;
                }
                Ok(count) => {
                    if count > MAX_RESPONSE_BYTES.saturating_sub(self.buffer.len()) {
                        self.stop(ProbeFailure::OutputLimit)?;
                    } else {
                        self.buffer.extend_from_slice(&bytes[..count]);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => self.stop(ProbeFailure::Read)?,
            }
        }
        if self.exit.is_none() {
            self.exit = self.child.try_wait()?;
        }
        if let Some(status) = self.exit {
            // EOF may follow exit on a later poll; retain observed status but only
            // publish after the remaining finite pipe data has reached EOF.
            if self.failure.is_some() {
                self.finished = true;
                return Ok(ProbePhase::Exited);
            }
            if !status.success() {
                self.failure = Some(ProbeFailure::Exit);
                self.reader = None;
                self.buffer.clear();
                self.finished = true;
                return Ok(ProbePhase::Exited);
            }
            if self.eof {
                match decode_report(&self.buffer, &self.target) {
                    Ok(report) => self.report = Some(report),
                    Err(_) => self.failure = Some(ProbeFailure::Protocol),
                }
                self.buffer.clear();
                self.finished = true;
                return Ok(ProbePhase::Exited);
            }
        }
        Ok(self.phase())
    }
}
impl Drop for ProbeJob {
    fn drop(&mut self) {
        if self.exit.is_some() {
            return;
        }
        let _ = self.stop(ProbeFailure::Requested);
        let deadline = Instant::now() + Duration::from_millis(20);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.exit = Some(status);
                    break;
                }
                Ok(None) => std::thread::yield_now(),
                Err(_) => break,
            }
        }
    }
}
