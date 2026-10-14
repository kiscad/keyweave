//! Bounded uinput configuration and one-shot writes. No retry hides acceptance.
mod channel;
use crate::{
    events::{self, FrameWrite, Layout, NativeKey, ProtocolError},
    keys,
};
pub use channel::{OutputChannel, OutputState};
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
    relative_axes: Option<crate::relative::RelativeAxes>,
    pointing_buttons: Option<keys::PointingButtons>,
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
            relative_axes: None,
            pointing_buttons: None,
        })
    }
    /// Explicit extra relative axes; this metadata does not create a device.
    pub fn with_relative_axes(mut self, axes: crate::relative::RelativeAxes) -> Self {
        self.relative_axes = Some(axes);
        self
    }
    /// Explicit pointer output requires both motion axes for coherent advertisement.
    pub fn with_pointing_buttons(mut self, buttons: keys::PointingButtons) -> io::Result<Self> {
        if !self.relative_axes().is_some_and(|axes| {
            axes.contains(crate::relative::REL_X) && axes.contains(crate::relative::REL_Y)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pointing output requires X and Y axes",
            ));
        }
        if let Some(previous) = self.pointing_buttons.take() {
            for code in previous.codes() {
                self.codes.remove(&code);
            }
        }
        self.codes.extend(buttons.codes());
        self.pointing_buttons = Some(buttons);
        Ok(self)
    }
    pub fn pointing_buttons(&self) -> Option<&keys::PointingButtons> {
        self.pointing_buttons.as_ref()
    }
    pub fn output_code(&self, key: keyweave_types::KeyId) -> Result<u16, keys::KeyError> {
        let code = if self.pointing_buttons.is_some() {
            keys::pointing_output_code(key)?
        } else {
            keys::output_code(key)?
        };
        Ok(code)
    }
    pub fn relative_axes(&self) -> Option<&crate::relative::RelativeAxes> {
        self.relative_axes.as_ref()
    }
    pub fn validate_shape(&self) -> io::Result<()> {
        if self.pointing_buttons.is_some()
            && !self.relative_axes().is_some_and(|axes| {
                axes.contains(crate::relative::REL_X) && axes.contains(crate::relative::REL_Y)
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pointing output requires X and Y axes",
            ));
        }
        Ok(())
    }
    pub fn validate_relative(&self, frame: &crate::relative::RelativeFrame) -> io::Result<()> {
        self.validate_shape()?;
        if frame.records().is_empty() {
            return Ok(());
        }
        let axes = self.relative_axes().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "relative output not advertised")
        })?;
        if frame
            .records()
            .iter()
            .any(|record| !axes.contains(record.code))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unadvertised relative axis",
            ));
        }
        Ok(())
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
        self.validate_shape()?;
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
