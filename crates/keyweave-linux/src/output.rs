//! Bounded uinput configuration and one-shot writes. No retry hides acceptance.
use crate::{
    events::{self, FrameWrite, Layout, NativeKey, ProtocolError},
    keys,
};
use std::{
    collections::BTreeSet,
    io::{self, Write},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatPolicy {
    Kernel,
    External,
}
#[derive(Clone, Debug)]
pub struct DeviceSpec {
    name: String,
    codes: BTreeSet<u16>,
    repeat: RepeatPolicy,
    leds: bool,
}
impl DeviceSpec {
    pub fn new(name: &str, codes: &[u16], repeat: RepeatPolicy, leds: bool) -> io::Result<Self> {
        if name.is_empty()
            || name.len() > 79
            || name.as_bytes().contains(&0)
            || codes.is_empty()
            || codes.len() > 154
            || codes
                .iter()
                .any(|code| !keys::native_codes().any(|known| known == *code))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid virtual keyboard specification",
            ));
        }
        Ok(Self {
            name: name.into(),
            codes: codes.iter().copied().collect(),
            repeat,
            leds,
        })
    }
    pub fn codes(&self) -> impl Iterator<Item = u16> + '_ {
        self.codes.iter().copied()
    }
    pub fn repeat(&self) -> RepeatPolicy {
        self.repeat
    }
    pub fn leds(&self) -> bool {
        self.leds
    }
    /// Native UAPI input_id, name[80], ff_effects_max; no struct pointer casts.
    pub fn setup_bytes(&self) -> [u8; 92] {
        let mut bytes = [0; 92];
        bytes[..2].copy_from_slice(&6u16.to_ne_bytes()); // BUS_VIRTUAL
        bytes[6..8].copy_from_slice(&1u16.to_ne_bytes());
        bytes[8..8 + self.name.len()].copy_from_slice(self.name.as_bytes());
        bytes
    }
    pub fn validate(&self, edges: &[NativeKey]) -> io::Result<()> {
        if edges.len() > events::MAX_FRAME_EVENTS
            || edges.iter().any(|edge| {
                !self.codes.contains(&edge.code)
                    || !(0..=2).contains(&edge.value)
                    || (edge.value == 2 && self.repeat == RepeatPolicy::Kernel)
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unadvertised key, invalid value or conflicting repeat source",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("invalid output frame: {0}")]
    InvalidFrame(ProtocolError),
    #[error("output acceptance is unknown: {0}")]
    UnknownAcceptance(#[source] io::Error),
}
/// Even errno can follow injected records in uinput. Never retry the whole frame.
pub fn write_frame(
    writer: &mut impl Write,
    layout: Layout,
    edges: &[NativeKey],
) -> Result<FrameWrite, WriteError> {
    let bytes = events::output_frame(layout, edges).map_err(WriteError::InvalidFrame)?;
    if bytes.is_empty() {
        return Ok(FrameWrite::Complete { accepted: 0 });
    }
    let written = writer
        .write(&bytes)
        .map_err(WriteError::UnknownAcceptance)?;
    events::classify_write(layout, edges.len(), written).map_err(|issue| {
        WriteError::UnknownAcceptance(io::Error::new(io::ErrorKind::InvalidData, issue))
    })
}
/// Finish a known prefix before releasing its submitted keys. Zero is retryable;
/// any non-aligned result or errno requires destruction/quarantine.
pub fn flush_report(writer: &mut impl Write, layout: Layout) -> Result<bool, WriteError> {
    let bytes = events::encode(
        layout,
        events::Event::output(events::EV_SYN, events::SYN_REPORT, 0),
    )
    .map_err(WriteError::InvalidFrame)?;
    match writer
        .write(&bytes)
        .map_err(WriteError::UnknownAcceptance)?
    {
        0 => Ok(false),
        count if count == bytes.len() => Ok(true),
        _ => Err(WriteError::UnknownAcceptance(io::Error::new(
            io::ErrorKind::InvalidData,
            "ambiguous report write",
        ))),
    }
}
