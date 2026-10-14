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
    #[serde(default)]
    pub relative_motion_axes: Option<Vec<u8>>,
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
    validate_report(&report, target)?;
    Ok(report)
}
fn validate_report(report: &ProbeReport, target: &ProbeTarget) -> io::Result<()> {
    if !matches!(report.schema_version, 1 | 2)
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
            || (report.schema_version == 2 && device.relative_motion_axes.is_none())
            || device.relative_motion_axes.as_ref().is_some_and(|axes| {
                axes.len() > 2
                    || axes.iter().any(|axis| *axis > 1)
                    || axes.windows(2).any(|pair| pair[0] >= pair[1])
                    || (!axes.is_empty()
                        && (report.schema_version != 2 || device.kind != DeviceKind::Pointer))
            })
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    Ok(())
}
/// Explicit X11 numbering, not evdev/HID numbering or a native mapping proof.
#[derive(Debug)]
pub struct CoverageRequirements {
    keys: Vec<i32>,
    buttons: Vec<u32>,
    relative_motion: bool,
}
impl CoverageRequirements {
    pub fn new(keys: &[i32], buttons: &[u32]) -> io::Result<Self> {
        Self::build(keys, buttons, false)
    }
    /// Requires relative X and Y on the same pointer; no scroll claim.
    pub fn with_relative_motion(keys: &[i32], buttons: &[u32]) -> io::Result<Self> {
        Self::build(keys, buttons, true)
    }
    fn build(keys: &[i32], buttons: &[u32], relative_motion: bool) -> io::Result<Self> {
        if (keys.is_empty() && buttons.is_empty() && !relative_motion)
            || keys.len() > 248
            || buttons.len() > 256
            || keys.iter().any(|key| !(8..=255).contains(key))
            || buttons.iter().any(|button| !(1..=256).contains(button))
            || keys.windows(2).any(|pair| pair[0] >= pair[1])
            || buttons.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self {
            keys: keys.into(),
            buttons: buttons.into(),
            relative_motion,
        })
    }
}
/// A selected slave and its reported master; metadata only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoveredDevice {
    pub slave: i32,
    pub master: i32,
}
/// No lifetime, session, owned-node or capture authorization.
#[derive(Debug, PartialEq, Eq)]
pub struct MetadataCoverage {
    pub keyboard: Option<CoveredDevice>,
    pub pointer: Option<CoveredDevice>,
}
impl ProbeReport {
    pub fn check_coverage(
        &self,
        target: &ProbeTarget,
        required: &CoverageRequirements,
    ) -> io::Result<MetadataCoverage> {
        validate_report(self, target)?;
        let select = |kind, needed: bool| -> io::Result<Option<CoveredDevice>> {
            if !needed {
                return Ok(None);
            }
            let mut matches = self.devices.iter().filter(|device| {
                device.kind == kind
                    && match kind {
                        DeviceKind::Keyboard => required
                            .keys
                            .iter()
                            .all(|key| device.keys.binary_search(key).is_ok()),
                        DeviceKind::Pointer => {
                            required
                                .buttons
                                .iter()
                                .all(|button| *button <= device.buttons)
                                && (!required.relative_motion
                                    || device.relative_motion_axes.as_deref() == Some(&[0, 1]))
                        }
                    }
            });
            let device = matches.next().ok_or(io::ErrorKind::NotFound)?;
            if matches.next().is_some() {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(Some(CoveredDevice {
                slave: device.id,
                master: device.attachment,
            }))
        };
        Ok(MetadataCoverage {
            keyboard: select(DeviceKind::Keyboard, !required.keys.is_empty())?,
            pointer: select(
                DeviceKind::Pointer,
                !required.buttons.is_empty() || required.relative_motion,
            )?,
        })
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod coverage_tests {
    use super::*;
    fn target() -> ProbeTarget {
        ProbeTarget::new(":0", Path::new("/dev/input/event5"), 13, 69).unwrap()
    }
    fn report(devices: Vec<DeviceMetadata>) -> ProbeReport {
        ProbeReport {
            schema_version: 1,
            display: ":0".into(),
            node: "/dev/input/event5".into(),
            major: 13,
            minor: 69,
            devices,
        }
    }
    fn keyboard(id: i32, keys: &[i32]) -> DeviceMetadata {
        DeviceMetadata {
            id,
            attachment: 3,
            kind: DeviceKind::Keyboard,
            keys: keys.into(),
            buttons: 0,
            relative_motion_axes: None,
        }
    }
    fn pointer(id: i32, buttons: u32) -> DeviceMetadata {
        DeviceMetadata {
            id,
            attachment: 2,
            kind: DeviceKind::Pointer,
            keys: vec![],
            buttons,
            relative_motion_axes: None,
        }
    }
    #[test]
    fn combined_coverage_retains_each_attachment() {
        let r = report(vec![keyboard(8, &[8, 38, 255]), pointer(9, 12)]);
        let c = r
            .check_coverage(
                &target(),
                &CoverageRequirements::new(&[38, 255], &[1, 12]).unwrap(),
            )
            .unwrap();
        assert_eq!(
            c.keyboard,
            Some(CoveredDevice {
                slave: 8,
                master: 3
            })
        );
        assert_eq!(
            c.pointer,
            Some(CoveredDevice {
                slave: 9,
                master: 2
            })
        );
    }
    #[test]
    fn requirements_are_bounded_sorted_unique_and_nonempty() {
        for keys in [vec![], vec![7], vec![256], vec![38, 38], vec![39, 38]] {
            assert!(CoverageRequirements::new(&keys, &[]).is_err());
        }
        for buttons in [vec![0], vec![257], vec![1, 1], vec![2, 1]] {
            assert!(CoverageRequirements::new(&[], &buttons).is_err());
        }
        assert!(
            CoverageRequirements::new(
                &(8..=255).collect::<Vec<_>>(),
                &(1..=256).collect::<Vec<_>>()
            )
            .is_ok()
        );
    }
    #[test]
    fn cannot_union_partial_slaves() {
        let r = report(vec![keyboard(8, &[38]), keyboard(9, &[39])]);
        assert!(
            r.check_coverage(
                &target(),
                &CoverageRequirements::new(&[38, 39], &[]).unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn ambiguous_complete_slaves_are_rejected() {
        let r = report(vec![keyboard(8, &[38]), keyboard(9, &[38])]);
        assert!(
            r.check_coverage(&target(), &CoverageRequirements::new(&[38], &[]).unwrap())
                .is_err()
        );
        let r = report(vec![pointer(8, 12), pointer(9, 12)]);
        assert!(
            r.check_coverage(&target(), &CoverageRequirements::new(&[], &[1]).unwrap())
                .is_err()
        );
    }
    #[test]
    fn missing_kind_or_button_never_passes() {
        let need = CoverageRequirements::new(&[], &[12]).unwrap();
        for r in [
            report(vec![]),
            report(vec![keyboard(8, &[38])]),
            report(vec![pointer(8, 11)]),
        ] {
            assert!(r.check_coverage(&target(), &need).is_err());
        }
    }
    #[test]
    fn target_and_mutated_structure_are_revalidated() {
        let need = CoverageRequirements::new(&[38], &[]).unwrap();
        let mut r = report(vec![keyboard(8, &[38])]);
        let wrong = ProbeTarget::new(":1", Path::new("/dev/input/event5"), 13, 69).unwrap();
        assert!(r.check_coverage(&wrong, &need).is_err());
        r.devices[0].keys = vec![38, 38];
        assert!(r.check_coverage(&target(), &need).is_err());
        r.devices[0].keys = vec![38];
        r.devices[0].attachment = 8;
        assert!(r.check_coverage(&target(), &need).is_err());
    }
    #[test]
    fn single_kind_does_not_require_unrequested_kind() {
        let r = report(vec![pointer(8, 3)]);
        let c = r
            .check_coverage(&target(), &CoverageRequirements::new(&[], &[1, 3]).unwrap())
            .unwrap();
        assert!(c.keyboard.is_none());
        assert_eq!(c.pointer.unwrap().slave, 8);
    }
    #[test]
    fn motion_requires_v2_both_axes_on_one_pointer() {
        let need = CoverageRequirements::with_relative_motion(&[], &[]).unwrap();
        let mut r = report(vec![pointer(8, 3)]);
        assert!(r.check_coverage(&target(), &need).is_err());
        r.schema_version = 2;
        r.devices[0].relative_motion_axes = Some(vec![0]);
        assert!(r.check_coverage(&target(), &need).is_err());
        r.devices[0].relative_motion_axes = Some(vec![0, 1]);
        assert_eq!(
            r.check_coverage(&target(), &need)
                .unwrap()
                .pointer
                .unwrap()
                .slave,
            8
        );
    }
    #[test]
    fn motion_cannot_be_split_between_devices_or_button_coverage() {
        let need = CoverageRequirements::with_relative_motion(&[], &[12]).unwrap();
        let mut r = report(vec![pointer(8, 12), pointer(9, 11)]);
        r.schema_version = 2;
        r.devices[0].relative_motion_axes = Some(vec![0]);
        r.devices[1].relative_motion_axes = Some(vec![1]);
        assert!(r.check_coverage(&target(), &need).is_err());
        r.devices[0].relative_motion_axes = Some(vec![]);
        r.devices[1].relative_motion_axes = Some(vec![0, 1]);
        assert!(r.check_coverage(&target(), &need).is_err());
    }
    #[test]
    fn axis_protocol_rejects_invalid_or_missing_metadata() {
        let need = CoverageRequirements::with_relative_motion(&[], &[]).unwrap();
        let mut r = report(vec![pointer(8, 3)]);
        r.schema_version = 2;
        for axes in [
            None,
            Some(vec![1, 0]),
            Some(vec![0, 0]),
            Some(vec![2]),
            Some(vec![0, 1, 2]),
        ] {
            r.devices[0].relative_motion_axes = axes;
            assert!(r.check_coverage(&target(), &need).is_err());
        }
        r.devices[0].relative_motion_axes = Some(vec![0, 1]);
        r.devices[0].kind = DeviceKind::Keyboard;
        assert!(r.check_coverage(&target(), &need).is_err());
        r.devices[0].kind = DeviceKind::Pointer;
        r.schema_version = 1;
        assert!(r.check_coverage(&target(), &need).is_err());
    }
    #[test]
    fn schema2_roundtrip_and_schema1_decode_compatibility() {
        let raw = br#"{"schema_version":2,"display":":0","node":"/dev/input/event5","major":13,"minor":69,"devices":[{"id":8,"attachment":2,"kind":"pointer","keys":[],"buttons":3,"relative_motion_axes":[0,1]}]}"#;
        let r = decode_report(raw, &target()).unwrap();
        assert!(
            r.check_coverage(
                &target(),
                &CoverageRequirements::with_relative_motion(&[], &[1]).unwrap()
            )
            .is_ok()
        );
        let old = br#"{"schema_version":1,"display":":0","node":"/dev/input/event5","major":13,"minor":69,"devices":[{"id":8,"attachment":2,"kind":"pointer","keys":[],"buttons":3}]}"#;
        let r = decode_report(old, &target()).unwrap();
        assert!(
            r.check_coverage(&target(), &CoverageRequirements::new(&[], &[1]).unwrap())
                .is_ok()
        );
        assert!(
            r.check_coverage(
                &target(),
                &CoverageRequirements::with_relative_motion(&[], &[]).unwrap()
            )
            .is_err()
        );
    }
}
