//! Exact packet ownership until the corresponding stage receipt, not native auth.
use crate::relative::RelativeFrame;
use keyweave_engine::{ConfigSnapshot, pointer_work::PointerMotion, timeline::DueEvent};
use keyweave_types::*;
use std::{collections::BTreeMap, sync::Arc};
pub const MAX_PENDING_FRAMES: usize = 4096;
pub const MAX_PENDING_RECORDS: usize = 8192;
struct Pending {
    device: DeviceInstanceId,
    original_at: MonoTime,
    config: Arc<ConfigSnapshot>,
    frame: RelativeFrame,
}
#[derive(Default)]
pub struct PendingRelativeFrames {
    frames: BTreeMap<SourceToken, Pending>,
    records: usize,
    last_source: Option<SourceToken>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PendingError {
    #[error("empty or over-budget relative packet cache")]
    Capacity,
    #[error("relative pointer source is invalid or reused")]
    Source,
    #[error("relative receipt metadata does not match its saved packet")]
    Receipt,
}
impl PendingRelativeFrames {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn pending_frames(&self) -> usize {
        self.frames.len()
    }
    pub fn pending_records(&self) -> usize {
        self.records
    }
    pub fn can_accept(&self, frame: &RelativeFrame) -> Result<(), PendingError> {
        if frame.records().is_empty()
            || self.frames.len() >= MAX_PENDING_FRAMES
            || frame.records().len() > MAX_PENDING_RECORDS.saturating_sub(self.records)
        {
            return Err(PendingError::Capacity);
        }
        Ok(())
    }
    pub fn input_event(frame: &RelativeFrame) -> InputEvent {
        let activity = frame.activity();
        InputEvent::PointerActivity {
            moved: activity.moved,
            scrolled: activity.scrolled,
        }
    }
    /// Call after cold can_accept and successful actual Physical registration.
    pub fn register(
        &mut self,
        source: SourceToken,
        device: DeviceInstanceId,
        at: MonoTime,
        config: Arc<ConfigSnapshot>,
        frame: RelativeFrame,
    ) -> Result<(), PendingError> {
        self.can_accept(&frame)?;
        if source.0 <= 1u64 << 63 || self.last_source.is_some_and(|last| source <= last) {
            return Err(PendingError::Source);
        }
        self.records += frame.records().len();
        self.frames.insert(
            source,
            Pending {
                device,
                original_at: at,
                config,
                frame,
            },
        );
        self.last_source = Some(source);
        Ok(())
    }
    pub fn take(
        &mut self,
        receipt: &DueEvent<PointerMotion>,
    ) -> Result<RelativeFrame, PendingError> {
        let pending = self
            .frames
            .get(&receipt.event.source)
            .ok_or(PendingError::Receipt)?;
        let activity = pending.frame.activity();
        if receipt.event.device != pending.device
            || !Arc::ptr_eq(&receipt.event.config, &pending.config)
            || receipt.event.at_ns < pending.original_at
            || receipt.delivered_at < receipt.event.at_ns
            || receipt.event.payload
                != (PointerMotion::Activity {
                    moved: activity.moved,
                    scrolled: activity.scrolled,
                })
        {
            return Err(PendingError::Receipt);
        }
        let pending = self
            .frames
            .remove(&receipt.event.source)
            .ok_or(PendingError::Receipt)?;
        self.records -= pending.frame.records().len();
        Ok(pending.frame)
    }
    fn discard(&mut self, remove: impl Fn(DeviceInstanceId) -> bool) {
        self.frames.retain(|_, frame| !remove(frame.device));
        self.records = self
            .frames
            .values()
            .map(|frame| frame.frame.records().len())
            .sum();
    }
    pub fn discard_device(&mut self, device: DeviceInstanceId) {
        self.discard(|saved| saved == device);
    }
    pub fn discard_older(&mut self, device: DeviceInstanceId) {
        self.discard(|saved| saved.id == device.id && saved.generation < device.generation);
    }
    /// Reset preserves the monotonic source cursor; it never authorizes reuse.
    pub fn discard_all(&mut self) {
        self.frames.clear();
        self.records = 0;
    }
}
