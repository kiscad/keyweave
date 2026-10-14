//! Exact bounded EV_REL frames. Capabilities are metadata, not device readiness.
use crate::{
    events::{self, Event, FrameWrite, Layout, ProtocolError},
    output::WriteError,
};
use std::{
    collections::BTreeSet,
    io::{self, Write},
};
pub const EV_REL: u16 = 2;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const REL_HWHEEL: u16 = 6;
pub const REL_WHEEL: u16 = 8;
pub const REL_WHEEL_HI_RES: u16 = 11;
pub const REL_HWHEEL_HI_RES: u16 = 12;
pub const SUPPORTED_AXES: [u16; 6] = [
    REL_X,
    REL_Y,
    REL_HWHEEL,
    REL_WHEEL,
    REL_WHEEL_HI_RES,
    REL_HWHEEL_HI_RES,
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelativeAxes {
    codes: BTreeSet<u16>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RelativeError {
    #[error("relative axes must contain 1..=6 unique supported codes")]
    Axes,
    #[error("relative frame capacity exceeded")]
    Capacity,
    #[error("relative frame contains another event class or an unadvertised axis")]
    Record,
}
impl RelativeAxes {
    pub fn new(codes: &[u16]) -> Result<Self, RelativeError> {
        if codes.is_empty()
            || codes.len() > SUPPORTED_AXES.len()
            || codes.iter().any(|code| !SUPPORTED_AXES.contains(code))
        {
            return Err(RelativeError::Axes);
        }
        let unique: BTreeSet<_> = codes.iter().copied().collect();
        if unique.len() != codes.len() {
            return Err(RelativeError::Axes);
        }
        Ok(Self { codes: unique })
    }
    pub fn codes(&self) -> impl Iterator<Item = u16> + '_ {
        self.codes.iter().copied()
    }
    pub fn contains(&self, code: u16) -> bool {
        self.codes.contains(&code)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RelativeActivity {
    pub moved: bool,
    pub scrolled: bool,
}
#[derive(Clone, Debug)]
pub struct RelativeFrame {
    records: Vec<Event>,
    activity: RelativeActivity,
}
impl RelativeFrame {
    /// Keep exact records; do not synthesize or scale legacy/HI_RES wheel pairs.
    pub fn prepare(axes: &RelativeAxes, records: &[Event]) -> Result<Self, RelativeError> {
        if records.len() > events::MAX_FRAME_EVENTS {
            return Err(RelativeError::Capacity);
        }
        if records
            .iter()
            .any(|record| record.kind != EV_REL || !axes.contains(record.code))
        {
            return Err(RelativeError::Record);
        }
        let mut activity = RelativeActivity::default();
        for record in records.iter().filter(|record| record.value != 0) {
            if matches!(record.code, REL_X | REL_Y) {
                activity.moved = true;
            } else {
                activity.scrolled = true;
            }
        }
        Ok(Self {
            records: records.to_vec(),
            activity,
        })
    }
    pub fn records(&self) -> &[Event] {
        &self.records
    }
    pub fn activity(&self) -> RelativeActivity {
        self.activity
    }
    /// Output timestamps are ignored by uinput and encoded as zero, as for keys.
    /// The immutable original record metadata remains available to input owners.
    pub fn encode(&self, layout: Layout) -> Result<Vec<u8>, ProtocolError> {
        if self.records.is_empty() {
            return Ok(vec![]);
        }
        let mut bytes = Vec::with_capacity((self.records.len() + 1) * layout.record_size());
        for record in &self.records {
            bytes.extend(events::encode(
                layout,
                Event::output(EV_REL, record.code, record.value),
            )?);
        }
        bytes.extend(events::encode(
            layout,
            Event::output(events::EV_SYN, events::SYN_REPORT, 0),
        )?);
        Ok(bytes)
    }
    /// One call only: known prefixes exclude REPORT and are never retried here.
    /// Unknown acceptance requires capture stop/quarantine at the owning pump.
    pub fn write_once(
        &self,
        writer: &mut impl Write,
        layout: Layout,
    ) -> Result<FrameWrite, WriteError> {
        let bytes = self.encode(layout).map_err(WriteError::InvalidFrame)?;
        if bytes.is_empty() {
            return Ok(FrameWrite::Complete { accepted: 0 });
        }
        let count = writer
            .write(&bytes)
            .map_err(WriteError::UnknownAcceptance)?;
        events::classify_write(layout, self.records.len(), count).map_err(|error| {
            WriteError::UnknownAcceptance(io::Error::new(io::ErrorKind::InvalidData, error))
        })
    }
}
