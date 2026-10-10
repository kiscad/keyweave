//! Read-only text VT/keyboard-handler observations, not delivery acknowledgements.
#![allow(unsafe_code)]
use crate::{
    reactor::MonotonicClock,
    uinput::{OutputState, UinputDevice},
    worker::WorkerParentGuard,
};
use keyweave_types::MonoTime;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    marker::PhantomData,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd},
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    rc::Rc,
};
pub const CONSOLE_LIFETIME_NS: u64 = 250_000_000;
mod lease;
pub use lease::ConsoleLeaseError;
pub(crate) use lease::Lease;
const MAX_INVENTORY: usize = 1024 * 1024;
#[repr(C)]
#[derive(Default)]
struct VtState {
    active: u16,
    signal: u16,
    state: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Snapshot {
    active: u16,
    display: i32,
    keyboard: i32,
    tty_session: i32,
    foreground: i32,
    parent_session: i32,
    parent_group: i32,
    worker_session: i32,
}
impl Snapshot {
    fn eligible(self, vt: u16, anchor: (i32, i32)) -> bool {
        self.active == vt
            && self.display == 0
            && matches!(self.keyboard, 1 | 3)
            && self.tty_session > 0
            && self.foreground > 0
            && self.tty_session == self.parent_session
            && self.tty_session == self.worker_session
            && self.foreground == self.parent_group
            && (self.tty_session, self.foreground) == anchor
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn scalar(fd: BorrowedFd<'_>, command: libc::Ioctl) -> io::Result<i32> {
    let mut value = 0i32;
    // SAFETY: caller supplies only KDGETMODE/KDGKBMODE; initialized aligned
    // int output, live uniquely borrowed VT FD, synchronous no retained pointer.
    if unsafe { libc::ioctl(fd.as_raw_fd(), command, &mut value) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}
fn snapshot(fd: BorrowedFd<'_>, parent: u32) -> io::Result<Snapshot> {
    let parent = libc::pid_t::try_from(parent).map_err(|_| invalid("invalid parent"))?;
    let mut state = VtState::default();
    // SAFETY: VT_GETSTATE writes exactly the audited 6-byte aligned vt_stat.
    if unsafe { libc::ioctl(fd.as_raw_fd(), 0x5603 as libc::Ioctl, &mut state) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let display = scalar(fd, 0x4b3b as libc::Ioctl)?;
    let keyboard = scalar(fd, 0x4b44 as libc::Ioctl)?;
    // SAFETY: read-only job-control/session queries of the live VT descriptor
    // and verified parent/self PID. No signal, group, session or VT mutation.
    let tty_session = positive_pid(unsafe { libc::tcgetsid(fd.as_raw_fd()) })?;
    // SAFETY: read-only query, same live borrowed VT descriptor.
    let foreground = positive_pid(unsafe { libc::tcgetpgrp(fd.as_raw_fd()) })?;
    // SAFETY: read-only session query for verified positive parent PID.
    let parent_session = positive_pid(unsafe { libc::getsid(parent) })?;
    // SAFETY: read-only group query for verified positive parent PID.
    let parent_group = positive_pid(unsafe { libc::getpgid(parent) })?;
    // SAFETY: PID zero requests the current worker's session only.
    let worker_session = positive_pid(unsafe { libc::getsid(0) })?;
    Ok(Snapshot {
        active: state.active,
        display,
        keyboard,
        tty_session,
        foreground,
        parent_session,
        parent_group,
        worker_session,
    })
}
fn positive_pid(value: libc::pid_t) -> io::Result<i32> {
    if value < 0 {
        return Err(io::Error::last_os_error());
    }
    if value == 0 {
        return Err(invalid("console session metadata unavailable"));
    }
    Ok(value)
}
fn vt_number(path: &Path) -> io::Result<u16> {
    let text = path.to_str().ok_or_else(|| invalid("invalid VT path"))?;
    let number = text
        .strip_prefix("/dev/tty")
        .filter(|v| !v.is_empty() && !v.starts_with('0') && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|v| (1..=63).contains(v));
    number.ok_or_else(|| invalid("expected explicit /dev/tty1 through /dev/tty63"))
}
fn validate_file(file: &File, vt: u16) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.file_type().is_char_device()
        || libc::major(m.rdev() as libc::dev_t) != 4
        || libc::minor(m.rdev() as libc::dev_t) != u32::from(vt)
    {
        return Err(invalid("descriptor is not the selected kernel VT"));
    }
    Ok(())
}
fn read_bounded(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let mut bytes = Vec::new();
    file.take((max + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        return Err(invalid("console metadata exceeds limit"));
    }
    Ok(bytes)
}
fn has_handler(bytes: &[u8], syspath: &Path) -> io::Result<bool> {
    if bytes.len() > MAX_INVENTORY
        || bytes.contains(&0)
        || (!bytes.is_empty() && !bytes.ends_with(b"\n\n"))
    {
        return Err(invalid("invalid complete input inventory"));
    }
    let sys = syspath
        .to_str()
        .and_then(|p| p.strip_prefix("/sys"))
        .filter(|p| {
            p.strip_prefix("/devices/virtual/input/input")
                .is_some_and(|n| {
                    !n.is_empty()
                        && (n == "0" || !n.starts_with('0'))
                        && n.bytes().all(|b| b.is_ascii_digit())
                })
        })
        .ok_or_else(|| invalid("invalid owned output identity"))?;
    let mut matched = None;
    let mut sysfs = None;
    let mut handlers = None;
    for line in bytes.split(|b| *b == b'\n') {
        if line.is_empty() {
            if sysfs == Some(sys.as_bytes()) {
                if matched.is_some() {
                    return Err(invalid("ambiguous output inventory"));
                }
                let h = handlers.ok_or_else(|| invalid("missing output handlers"))?;
                matched = Some(h);
            }
            sysfs = None;
            handlers = None;
        } else if let Some(value) = line.strip_prefix(b"S: Sysfs=") {
            if sysfs.replace(value).is_some() {
                return Err(invalid("duplicate sysfs metadata"));
            }
        } else if let Some(value) = line.strip_prefix(b"H: Handlers=")
            && handlers
                .replace(
                    value
                        .split(|b| b.is_ascii_whitespace())
                        .any(|v| v == b"kbd"),
                )
                .is_some()
        {
            return Err(invalid("duplicate handler metadata"));
        }
    }
    Ok(matched.unwrap_or(false))
}
/// Short-lived observation for one owned virtual output. It is not an input
/// delivery guarantee, a supervision lease, or graphical desktop readiness.
pub struct ConsoleReady {
    output: PathBuf,
    observed: MonoTime,
    until: MonoTime,
}
impl std::fmt::Debug for ConsoleReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleReady")
            .field("observed", &self.observed)
            .field("until", &self.until)
            .finish_non_exhaustive()
    }
}
impl ConsoleReady {
    #[cfg(test)]
    pub(crate) fn for_test(output: PathBuf, observed: MonoTime, until: MonoTime) -> Self {
        Self {
            output,
            observed,
            until,
        }
    }
    pub fn fresh_for(&self, output: &Path, now: MonoTime) -> bool {
        self.output == output && self.observed <= now && now < self.until
    }
    pub fn valid_until(&self) -> MonoTime {
        self.until
    }
}
pub struct ConsoleProvider {
    file: File,
    vt: u16,
    parent: u32,
    anchor: (i32, i32),
    _thread: PhantomData<Rc<()>>,
}
impl ConsoleProvider {
    /// Verified worker main only. No controlling terminal acquisition or read.
    pub fn open(path: &Path, guard: &WorkerParentGuard) -> io::Result<Self> {
        let _verified = WorkerParentGuard::verify(guard.parent_pid())?;
        let vt = vt_number(path)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)?;
        validate_file(&file, vt)?;
        let initial = snapshot(file.as_fd(), guard.parent_pid())?;
        let anchor = (initial.tty_session, initial.foreground);
        if !initial.eligible(vt, anchor) {
            return Err(invalid(
                "console is not an eligible foreground text session",
            ));
        }
        Ok(Self {
            file,
            vt,
            parent: guard.parent_pid(),
            anchor,
            _thread: PhantomData,
        })
    }
    pub fn observe(
        &self,
        output: &UinputDevice,
        clock: &mut MonotonicClock,
    ) -> io::Result<Option<ConsoleReady>> {
        let _verified = WorkerParentGuard::verify(self.parent)?;
        if output.state() != OutputState::Ready {
            return Ok(None);
        }
        let before = clock.now()?;
        let first = snapshot(self.file.as_fd(), self.parent)?;
        let path = output.input_syspath();
        let enabled = read_bounded(&path.join("inhibited"), 8)? == b"0\n";
        let handlers = has_handler(
            &read_bounded(Path::new("/proc/bus/input/devices"), MAX_INVENTORY)?,
            path,
        )?;
        let enabled_after = read_bounded(&path.join("inhibited"), 8)? == b"0\n";
        let last = snapshot(self.file.as_fd(), self.parent)?;
        let after = clock.now()?;
        grant(
            path,
            self.vt,
            self.anchor,
            Observation {
                before,
                after,
                first,
                last,
                handler: enabled && enabled_after && handlers,
            },
        )
    }
}
struct Observation {
    before: MonoTime,
    after: MonoTime,
    first: Snapshot,
    last: Snapshot,
    handler: bool,
}
fn grant(
    path: &Path,
    vt: u16,
    anchor: (i32, i32),
    observation: Observation,
) -> io::Result<Option<ConsoleReady>> {
    let Observation {
        before,
        after,
        first,
        last,
        handler,
    } = observation;
    let until = before
        .checked_add(CONSOLE_LIFETIME_NS)
        .ok_or_else(|| invalid("console time overflow"))?;
    if after < before {
        return Err(invalid("console clock moved backwards"));
    }
    if after >= until || !handler || first != last || !first.eligible(vt, anchor) {
        return Ok(None);
    }
    Ok(Some(ConsoleReady {
        output: path.into(),
        observed: before,
        until,
    }))
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
