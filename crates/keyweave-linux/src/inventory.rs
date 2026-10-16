//! Bounded metadata snapshot; never a capture/readiness/physical-universe token.
use crate::{
    device::{DeviceInspection, EvdevDevice},
    device_types::DeviceTypes,
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const MAX_DIRECTORY: usize = 1024;
const MAX_NODES: usize = 256;
const LIMIT: Duration = Duration::from_secs(2);
#[derive(Debug, Serialize)]
pub struct Failure {
    pub kind: String,
    pub os_code: Option<i32>,
}
impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self {
            kind: format!("{:?}", error.kind()),
            os_code: error.raw_os_error(),
        }
    }
}
#[derive(Debug, Serialize)]
pub struct TypeFacts {
    pub is_keyboard: bool,
    pub is_pointing_device: bool,
}
impl From<DeviceTypes> for TypeFacts {
    fn from(value: DeviceTypes) -> Self {
        Self {
            is_keyboard: value.is_keyboard,
            is_pointing_device: value.is_pointing_device,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct Metadata {
    pub bus: u16,
    pub vendor_id: u16,
    pub product_id: u16,
    pub version: u16,
    pub device_number: (u32, u32),
    pub name: Option<String>,
    pub name_was_lossy: bool,
    pub types: Option<TypeFacts>,
    pub advertised_key_count: usize,
    pub keyboard_capability_issues: Vec<String>,
    pub keyboard_issues_truncated: bool,
    pub relative_capability_issues: Vec<String>,
    pub relative_issues_truncated: bool,
}
impl From<DeviceInspection> for Metadata {
    fn from(value: DeviceInspection) -> Self {
        let probe = value.probe;
        let (name, name_was_lossy) = probe.name.map_or((None, false), |bytes| {
            let text = String::from_utf8_lossy(&bytes);
            let lossy = matches!(text, std::borrow::Cow::Owned(_));
            (Some(text.into_owned()), lossy)
        });
        let keyboard = probe.capabilities.keyboard_issues();
        let relative = probe.capabilities.relative_issues();
        Self {
            bus: probe.id.bus,
            vendor_id: probe.id.vendor,
            product_id: probe.id.product,
            version: probe.id.version,
            device_number: value.device_number,
            name,
            name_was_lossy,
            types: value.device_types.map(Into::into),
            advertised_key_count: probe.capabilities.keys.codes().count(),
            keyboard_issues_truncated: keyboard.len() > 16,
            keyboard_capability_issues: keyboard
                .into_iter()
                .take(16)
                .map(|v| v.to_string())
                .collect(),
            relative_issues_truncated: relative.len() > 16,
            relative_capability_issues: relative
                .into_iter()
                .take(16)
                .map(|v| v.to_string())
                .collect(),
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Entry {
    Observed { path: PathBuf, metadata: Metadata },
    Unavailable { path: PathBuf, failure: Failure },
}
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub schema_version: u8,
    pub scope: &'static str,
    pub scan_complete: bool,
    pub probes_complete: bool,
    pub directory_entries_seen: usize,
    pub issues: Vec<String>,
    pub entries: Vec<Entry>,
}
fn empty() -> Snapshot {
    Snapshot {
        schema_version: 1,
        scope: "canonical_evdev_nodes",
        scan_complete: true,
        probes_complete: true,
        directory_entries_seen: 0,
        issues: vec![],
        entries: vec![],
    }
}
fn number(name: &std::ffi::OsStr) -> Option<u32> {
    let name = name.to_str()?;
    if name.len() > 15 {
        return None;
    }
    let digits = name.strip_prefix("event")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value = digits.parse::<u32>().ok()?;
    if value.to_string() != digits {
        return None;
    }
    Some(value)
}
fn scan_with(
    names: impl IntoIterator<Item = io::Result<OsString>>,
    mut probe: impl FnMut(&Path) -> io::Result<Metadata>,
    mut clock: impl FnMut() -> Duration,
) -> Snapshot {
    let mut result = empty();
    let start = clock();
    let Some(until) = start.checked_add(LIMIT) else {
        result.scan_complete = false;
        result.probes_complete = false;
        result.issues.push("clock_overflow".into());
        return result;
    };
    let mut last = start;
    let mut valid_clock = true;
    let mut within = || {
        if !valid_clock {
            return false;
        }
        let now = clock();
        valid_clock = now >= last && now < until;
        if valid_clock {
            last = now;
        }
        valid_clock
    };
    let mut candidates = BTreeSet::new();
    for name in names {
        if !within() {
            result.scan_complete = false;
            result.issues.push("scan_clock_or_deadline".into());
            break;
        }
        if result.directory_entries_seen == MAX_DIRECTORY {
            result.scan_complete = false;
            result.issues.push("directory_limit".into());
            break;
        }
        result.directory_entries_seen += 1;
        let name = match name {
            Ok(name) => name,
            Err(error) => {
                result.scan_complete = false;
                result
                    .issues
                    .push(format!("directory_entry: {:?}", Failure::from(error)));
                continue;
            }
        };
        if let Some(number) = number(&name) {
            if candidates.contains(&number) {
                result.scan_complete = false;
                result.issues.push("duplicate_candidate".into());
                continue;
            }
            if candidates.len() == MAX_NODES {
                result.scan_complete = false;
                result.issues.push("candidate_limit".into());
                break;
            }
            candidates.insert(number);
        }
    }
    // These are sorted canonical paths, not a stable physical device identity.
    for number in candidates {
        if !within() {
            result.probes_complete = false;
            result.issues.push("probe_clock_or_deadline".into());
            break;
        }
        let path = Path::new("/dev/input").join(format!("event{number}"));
        match probe(&path) {
            Ok(metadata) => result.entries.push(Entry::Observed { path, metadata }),
            Err(error) => {
                result.probes_complete = false;
                result.entries.push(Entry::Unavailable {
                    path,
                    failure: error.into(),
                });
            }
        }
        if !within() {
            result.probes_complete = false;
            result.issues.push("probe_completed_after_budget".into());
            break;
        }
    }
    result
}
/// Explicit on-demand metadata access. No event read, state query, grab,
/// uinput, worker creation or runtime-context publication occurs here.
pub fn scan() -> Snapshot {
    let start = Instant::now();
    let names = match fs::read_dir("/dev/input") {
        Ok(names) => names,
        Err(error) => {
            let mut result = empty();
            result.scan_complete = false;
            result.probes_complete = false;
            result
                .issues
                .push(format!("input_directory: {:?}", Failure::from(error)));
            return result;
        }
    };
    scan_with(
        names.map(|entry| entry.map(|entry| entry.file_name())),
        |path| EvdevDevice::inspect(path).map(Into::into),
        || start.elapsed(),
    )
}
#[cfg(test)]
mod tests;
