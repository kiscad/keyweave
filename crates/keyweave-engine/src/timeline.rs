//! Bounded causal queues for deferred mapping-stage work.
//! Payload execution and ownership release remain the engine actor's responsibility.
use crate::{ConfigSnapshot, MAX_DEVICE_IDENTITIES};
use keyweave_types::{DeviceInstanceId, MonoTime, SourceToken};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
use thiserror::Error;

pub const MAX_STAGE_EVENTS: usize = 8192;
pub const MAX_STAGE_APPEND: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Simple,
    Complex,
    Output,
}
impl Stage {
    fn index(self) -> usize {
        match self {
            Self::Simple => 0,
            Self::Complex => 1,
            Self::Output => 2,
        }
    }
}

/// The immutable configuration and original physical source travel with work.
/// `at_ns` is an earliest execution time, not an OS submission timestamp.
#[derive(Clone, Debug)]
pub struct StageEvent<T> {
    pub stage: Stage,
    pub at_ns: MonoTime,
    pub device: DeviceInstanceId,
    pub source: SourceToken,
    pub config: Arc<ConfigSnapshot>,
    pub payload: T,
}

#[derive(Debug)]
pub struct DueEvent<T> {
    pub event: StageEvent<T>,
    pub delivered_at: MonoTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum TimelineError {
    #[error("bounded stage queue capacity exceeded")]
    Capacity,
    #[error("stage clock moved backwards")]
    TimeReversed,
    #[error("device generation is unknown, retired or removed")]
    RetiredDevice,
    #[error("bounded device identity history exhausted")]
    DeviceHistoryFull,
    #[error("stage ordering identity space exhausted")]
    IdentityExhausted,
    #[error("causal batch time shift would overflow monotonic time")]
    TimeOverflow,
}

/// Rejection returns the original owned work so callers can reconcile lifetimes.
#[derive(Debug)]
pub struct RejectedBatch<T> {
    pub error: TimelineError,
    pub events: Vec<StageEvent<T>>,
}

struct Entry<T> {
    order: u64,
    event: StageEvent<T>,
}
#[derive(Clone, Copy)]
struct Device {
    generation: u64,
    removed: bool,
}

pub struct StageTimeline<T> {
    lanes: [VecDeque<Entry<T>>; 3],
    devices: BTreeMap<u64, Device>,
    len: usize,
    next_order: u64,
    now: MonoTime,
}
/// Exclusive, validated admission. Dropping this guard leaves the timeline unchanged.
/// Commit cannot fail a business check; allocation failure/panic is not covered.
#[must_use = "commit the prepared admission or abort it without queue changes"]
pub struct PreparedStageAppend<'a, T> {
    timeline: &'a mut StageTimeline<T>,
    events: Vec<StageEvent<T>>,
    times: Vec<MonoTime>,
    advance_to: Option<MonoTime>,
}
impl<T> PreparedStageAppend<'_, T> {
    /// Return the original owned work with its original timestamps.
    pub fn abort(self) -> Vec<StageEvent<T>> {
        self.events
    }
    pub fn commit(self) {
        for (mut event, at) in self.events.into_iter().zip(self.times) {
            event.at_ns = at;
            self.timeline.lanes[event.stage.index()].push_back(Entry {
                order: self.timeline.next_order,
                event,
            });
            self.timeline.next_order += 1;
            self.timeline.len += 1;
        }
        if let Some(now) = self.advance_to {
            self.timeline.now = now;
        }
    }
}
impl<T> Default for StageTimeline<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> StageTimeline<T> {
    pub fn new() -> Self {
        Self {
            lanes: std::array::from_fn(|_| VecDeque::new()),
            devices: BTreeMap::new(),
            len: 0,
            next_order: 1,
            now: MonoTime(0),
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn known_devices(&self) -> usize {
        self.devices.len()
    }
    /// Device control precedes queued work at the same time. A returned payload
    /// was never executed; the actor must release its reservation, not invent up.
    pub fn observe_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
    ) -> Result<Vec<StageEvent<T>>, TimelineError> {
        self.check_device(device, removed)?;
        self.devices.insert(
            device.id,
            Device {
                generation: device.generation,
                removed,
            },
        );
        Ok(self.discard_where(|event| {
            event.device.id == device.id
                && (event.device.generation < device.generation
                    || (removed && event.device.generation == device.generation))
        }))
    }
    pub(crate) fn check_device(
        &self,
        device: DeviceInstanceId,
        removed: bool,
    ) -> Result<(), TimelineError> {
        if let Some(current) = self.devices.get(&device.id) {
            if device.generation < current.generation {
                return Err(TimelineError::RetiredDevice);
            }
            if device.generation == current.generation && current.removed {
                return if removed {
                    Ok(())
                } else {
                    Err(TimelineError::RetiredDevice)
                };
            }
        } else if self.devices.len() >= MAX_DEVICE_IDENTITIES {
            return Err(TimelineError::DeviceHistoryFull);
        }
        Ok(())
    }
    pub(crate) fn check_live_device(&self, device: DeviceInstanceId) -> Result<(), TimelineError> {
        if self.live(device) {
            Ok(())
        } else {
            Err(TimelineError::RetiredDevice)
        }
    }
    fn live(&self, device: DeviceInstanceId) -> bool {
        self.devices
            .get(&device.id)
            .is_some_and(|current| current.generation == device.generation && !current.removed)
    }
    pub fn enqueue(&mut self, events: Vec<StageEvent<T>>) -> Result<(), RejectedBatch<T>> {
        self.prepare_enqueue(events)?.commit();
        Ok(())
    }
    /// Preflight without queue/order/clock mutations. An exclusive borrow keeps
    /// device generations, FIFO tails and capacity stable until commit or abort.
    pub fn prepare_enqueue(
        &mut self,
        events: Vec<StageEvent<T>>,
    ) -> Result<PreparedStageAppend<'_, T>, RejectedBatch<T>> {
        let error = if events.len() > MAX_STAGE_APPEND
            || self.len.saturating_add(events.len()) > MAX_STAGE_EVENTS
        {
            Some(TimelineError::Capacity)
        } else if events.iter().any(|event| !self.live(event.device)) {
            Some(TimelineError::RetiredDevice)
        } else if self.next_order.checked_add(events.len() as u64).is_none() {
            Some(TimelineError::IdentityExhausted)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(RejectedBatch { error, events });
        }
        // Compute the whole admission before mutating queues/order IDs. Each
        // stage shifts this batch as a unit behind its existing FIFO tail.
        // Clamping each event separately would collapse a delayed down/up pair.
        let mut shifts: [Option<u64>; 3] = [None; 3];
        let mut tails: [MonoTime; 3] = std::array::from_fn(|index| {
            self.lanes[index]
                .back()
                .map_or(self.now, |entry| entry.event.at_ns.max(self.now))
        });
        let mut times = Vec::with_capacity(events.len());
        for event in &events {
            let index = event.stage.index();
            let shift =
                *shifts[index].get_or_insert_with(|| tails[index].0.saturating_sub(event.at_ns.0));
            let Some(shifted) = event.at_ns.checked_add(shift) else {
                return Err(RejectedBatch {
                    error: TimelineError::TimeOverflow,
                    events,
                });
            };
            let at = shifted.max(tails[index]);
            tails[index] = at;
            times.push(at);
        }
        Ok(PreparedStageAppend {
            timeline: self,
            events,
            times,
            advance_to: None,
        })
    }
    /// Validate an append followed by clock advancement as one transaction.
    /// Existing queued times are preserved; abort changes neither work nor clock.
    pub fn prepare_enqueue_at(
        &mut self,
        events: Vec<StageEvent<T>>,
        now: MonoTime,
    ) -> Result<PreparedStageAppend<'_, T>, RejectedBatch<T>> {
        if now < self.now {
            return Err(RejectedBatch {
                error: TimelineError::TimeReversed,
                events,
            });
        }
        let mut prepared = self.prepare_enqueue(events)?;
        prepared.advance_to = Some(now);
        Ok(prepared)
    }
    fn first(&self) -> Option<(MonoTime, u64, usize)> {
        self.lanes
            .iter()
            .enumerate()
            .filter_map(|(index, lane)| {
                lane.front()
                    .map(|entry| (entry.event.at_ns, entry.order, index))
            })
            .min()
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.first().map(|(at, _, _)| at)
    }
    pub fn advance(&mut self, now: MonoTime) -> Result<(), TimelineError> {
        if now < self.now {
            return Err(TimelineError::TimeReversed);
        }
        self.now = now;
        Ok(())
    }
    pub fn pop_due(&mut self, now: MonoTime) -> Result<Option<DueEvent<T>>, TimelineError> {
        if now < self.now {
            return Err(TimelineError::TimeReversed);
        }
        self.now = now;
        let Some((at, _, index)) = self.first() else {
            return Ok(None);
        };
        if at > now {
            return Ok(None);
        }
        if let Some(entry) = self.lanes[index].pop_front() {
            self.len -= 1;
            Ok(Some(DueEvent {
                event: entry.event,
                delivered_at: now,
            }))
        } else {
            Ok(None)
        }
    }
    /// Cancellation is exact source+device, never a key-code/time heuristic.
    pub fn discard_source(
        &mut self,
        device: DeviceInstanceId,
        source: SourceToken,
    ) -> Vec<StageEvent<T>> {
        self.discard_where(|event| event.device == device && event.source == source)
    }
    pub(crate) fn discard_where(
        &mut self,
        mut matches: impl FnMut(&StageEvent<T>) -> bool,
    ) -> Vec<StageEvent<T>> {
        let mut canceled = Vec::new();
        for lane in &mut self.lanes {
            let mut retained = VecDeque::new();
            while let Some(entry) = lane.pop_front() {
                if matches(&entry.event) {
                    canceled.push(entry);
                    self.len -= 1;
                } else {
                    retained.push_back(entry);
                }
            }
            *lane = retained;
        }
        canceled.sort_by_key(|entry| (entry.event.at_ns, entry.order));
        canceled.into_iter().map(|entry| entry.event).collect()
    }
    /// Reset preserves device history and ordering identities, returning all
    /// unexecuted payloads for explicit lifetime reconciliation by the actor.
    pub fn reset(&mut self, now: MonoTime) -> Result<Vec<StageEvent<T>>, TimelineError> {
        if now < self.now {
            return Err(TimelineError::TimeReversed);
        }
        self.now = now;
        Ok(self.discard_where(|_| true))
    }
}
