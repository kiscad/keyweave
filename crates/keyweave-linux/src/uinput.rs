//! Owned virtual keyboard. Creation alone does not authorize physical capture.
use crate::{
    events::{FrameWrite, Layout, NativeKey, native_layout},
    ioctl,
    output::{self, DeviceSpec, RepeatPolicy, WriteError},
    read::{self, ReadBatch},
};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsFd,
        unix::fs::{FileTypeExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputState {
    Ready,
    ReportPending,
    Recovery,
    Quarantined,
}
pub struct UinputDevice {
    file: File,
    layout: Layout,
    spec: DeviceSpec,
    input_syspath: PathBuf,
    state: OutputState,
}
impl UinputDevice {
    /// Opens only an explicit uinput node. Does not install, grab, wait for desktop
    /// readiness or change system permissions. Failed construction closes the FD.
    pub fn create(path: &Path, spec: DeviceSpec) -> io::Result<Self> {
        let layout = native_layout().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "unaudited native uinput ABI")
        })?;
        let path = fs::canonicalize(path)?;
        if ![Path::new("/dev/uinput"), Path::new("/dev/input/uinput")].contains(&path.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected an explicit uinput node",
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)?;
        if !file.metadata()?.file_type().is_char_device() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "uinput node is not a character device",
            ));
        }
        if ioctl::uinput_version(file.as_fd())? != 5 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "unsupported uinput protocol version",
            ));
        }
        ioctl::uinput_bit(file.as_fd(), 100, 1)?;
        for code in spec.codes() {
            ioctl::uinput_bit(file.as_fd(), 101, code)?;
        }
        if spec.repeat() == RepeatPolicy::Kernel {
            ioctl::uinput_bit(file.as_fd(), 100, 20)?;
        }
        if spec.leds() {
            ioctl::uinput_bit(file.as_fd(), 100, 17)?;
            for bit in 0..=2 {
                ioctl::uinput_bit(file.as_fd(), 105, bit)?;
            }
        }
        ioctl::uinput_setup(file.as_fd(), spec.setup_bytes())?;
        ioctl::uinput_lifecycle(file.as_fd(), true)?;
        // Install the RAII owner immediately after creation, before fallible identity queries.
        let mut device = Self {
            file,
            layout,
            spec,
            input_syspath: PathBuf::new(),
            state: OutputState::Ready,
        };
        let name = ioctl::uinput_sysname(device.file.as_fd())?;
        device.input_syspath =
            fs::canonicalize(Path::new("/sys/devices/virtual/input").join(name))?;
        Ok(device)
    }
    /// Compare this canonical input syspath to evdev probe.input_syspath to exclude
    /// our output. Names and vendor IDs alone are insufficient.
    pub fn input_syspath(&self) -> &Path {
        &self.input_syspath
    }
    pub fn state(&self) -> OutputState {
        self.state
    }
    pub fn emit(&mut self, edges: &[NativeKey]) -> io::Result<Result<FrameWrite, WriteError>> {
        if self.state != OutputState::Ready {
            return Err(io::Error::other("normal output is paused"));
        }
        self.write(edges)
    }
    /// Only key-ups are permitted after a known short write and report flush.
    /// Runtime supplies the accepted-prefix ledger; no guessed releases here.
    pub fn release_submitted(
        &mut self,
        edges: &[NativeKey],
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        if self.state != OutputState::Recovery || edges.iter().any(|edge| edge.value != 0) {
            return Err(io::Error::other("invalid output recovery"));
        }
        self.write(edges)
    }
    fn write(&mut self, edges: &[NativeKey]) -> io::Result<Result<FrameWrite, WriteError>> {
        self.spec.validate(edges)?;
        let result = output::write_frame(&mut self.file, self.layout, edges);
        match &result {
            Ok(FrameWrite::Incomplete { .. }) => self.state = OutputState::ReportPending,
            Err(WriteError::UnknownAcceptance(_)) => self.state = OutputState::Quarantined,
            _ => {}
        }
        Ok(result)
    }
    pub fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        if self.state != OutputState::ReportPending {
            return Err(io::Error::other("no known pending report"));
        }
        let result = output::flush_report(&mut self.file, self.layout);
        match &result {
            Ok(true) => self.state = OutputState::Recovery,
            Err(WriteError::UnknownAcceptance(_)) => self.state = OutputState::Quarantined,
            _ => {}
        }
        Ok(result)
    }
    /// Read kernel feedback (including LEDs). Physical LED synchronization remains
    /// a runtime responsibility; these records are never fed to the rule engine.
    pub fn read_feedback(&mut self) -> io::Result<ReadBatch> {
        read::read_batch(&mut self.file, self.layout)
    }
}
impl Drop for UinputDevice {
    fn drop(&mut self) {
        // Explicit destroy is best effort; owned FD close also destroys the device.
        let _ = ioctl::uinput_lifecycle(self.file.as_fd(), false);
    }
}
