//! Owned nonblocking evdev client, without any grab or input-forwarding API yet.
use crate::{
    events::{KEY_MAX, Layout, native_layout},
    ioctl,
    probe::{Bits, Capabilities, DeviceId},
    read::{ReadBatch, read_batch},
};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsFd,
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
#[derive(Clone, Debug)]
pub struct Probe {
    pub id: DeviceId,
    /// Byte string, never used alone to recognize our virtual device.
    pub name: Option<Vec<u8>>,
    pub input_syspath: PathBuf,
    pub capabilities: Capabilities,
}
pub struct EvdevDevice {
    file: File,
    layout: Layout,
    probe: Probe,
}
fn error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn query_bits(file: &File, number: u8, count: usize) -> io::Result<Bits> {
    Bits::from_native_words(
        &ioctl::mask(file.as_fd(), number, count.div_ceil(64) * 8)?,
        count,
    )
    .map_err(|issue| io::Error::new(io::ErrorKind::InvalidData, issue))
}
pub fn is_event_node(path: &Path) -> bool {
    path.parent() == Some(Path::new("/dev/input"))
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.strip_prefix("event").is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
                })
            })
}
impl EvdevDevice {
    /// Explicit path only. Symlinks resolve to /dev/input/eventN; final open is
    /// no-follow, read-only and nonblocking. This does not grab the keyboard.
    pub fn open(path: &Path) -> io::Result<Self> {
        let layout = native_layout().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "unaudited native evdev ABI")
        })?;
        let path = fs::canonicalize(path)?;
        if !is_event_node(&path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected an explicit /dev/input/eventN device",
            ));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_char_device() {
            return Err(error("input node is not a character device"));
        }
        if libc::major(metadata.rdev() as libc::dev_t) != 13 {
            return Err(error("input node has an unexpected character major"));
        }
        let version = ioctl::fixed(file.as_fd(), 1, 4)?;
        if i32::from_ne_bytes([version[0], version[1], version[2], version[3]]) != 0x010001 {
            return Err(error("unsupported evdev protocol version"));
        }
        let bytes = ioctl::fixed(file.as_fd(), 2, 8)?;
        let id = DeviceId {
            bus: u16::from_ne_bytes([bytes[0], bytes[1]]),
            vendor: u16::from_ne_bytes([bytes[2], bytes[3]]),
            product: u16::from_ne_bytes([bytes[4], bytes[5]]),
            version: u16::from_ne_bytes([bytes[6], bytes[7]]),
        };
        let events = query_bits(&file, 0x20, 32)?;
        let keys = query_bits(&file, 0x21, usize::from(KEY_MAX) + 1)?;
        let misc = if events.contains(4) {
            query_bits(&file, 0x24, 8)?
        } else {
            Bits::empty(8).map_err(|issue| error(&issue.to_string()))?
        };
        let leds = if events.contains(17) {
            query_bits(&file, 0x31, 16)?
        } else {
            Bits::empty(16).map_err(|issue| error(&issue.to_string()))?
        };
        let properties = query_bits(&file, 0x09, 32)?;
        let name = ioctl::name(file.as_fd())?;
        let dev = metadata.rdev() as libc::dev_t;
        let input_syspath = fs::canonicalize(format!(
            "/sys/dev/char/{}:{}/device",
            libc::major(dev),
            libc::minor(dev)
        ))?;
        ioctl::monotonic_clock(file.as_fd())?;
        Ok(Self {
            file,
            layout,
            probe: Probe {
                id,
                name,
                input_syspath,
                capabilities: Capabilities {
                    events,
                    keys,
                    misc,
                    leds,
                    properties,
                },
            },
        })
    }
    pub fn probe(&self) -> &Probe {
        &self.probe
    }
    pub fn pressed_keys(&self) -> io::Result<Bits> {
        query_bits(&self.file, 0x18, usize::from(KEY_MAX) + 1)
    }
    pub fn led_state(&self) -> io::Result<Bits> {
        query_bits(&self.file, 0x19, 16)
    }
    pub fn read_batch(&mut self) -> io::Result<ReadBatch> {
        read_batch(&mut self.file, self.layout)
    }
}
