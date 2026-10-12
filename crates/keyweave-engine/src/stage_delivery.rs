//! Delivery reservations outlive generation-time logical Up. No platform IO.
use crate::{
    action_plan::{ActionSlot, BatchId, PlannedAction},
    generated_stage::GeneratedError,
    timeline::StageEvent,
    *,
};
struct SlotDelivery {
    target: ActionTarget,
    down: bool,
    up_generated: bool,
    up: bool,
}
struct BatchDelivery {
    device: DeviceInstanceId,
    source: SourceToken,
    config: Arc<ConfigSnapshot>,
    slots: BTreeMap<u16, SlotDelivery>,
    extra: usize,
    scope: Option<ScopeDelivery>,
}
struct ScopeDelivery {
    slots: Vec<ActionSlot>,
    opened: bool,
    closed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryProgress {
    pub slots: usize,
    pub down_delivered: usize,
    pub up_generated: usize,
    pub up_delivered: usize,
    pub scope_reservations: usize,
}
#[derive(Default)]
pub(crate) struct DeliveryLedger {
    batches: BTreeMap<BatchId, BatchDelivery>,
    reserved: usize,
}
pub(crate) struct DeliveryAppend<'a> {
    ledger: &'a mut DeliveryLedger,
    new: BTreeMap<BatchId, BatchDelivery>,
    ups: Vec<ActionSlot>,
    reserved: usize,
}
impl DeliveryAppend<'_> {
    pub(crate) fn with_scope(
        mut self,
        id: BatchId,
        slots: Vec<ActionSlot>,
    ) -> Result<Self, GeneratedError> {
        let retained: usize = self
            .ledger
            .batches
            .values()
            .filter_map(|b| b.scope.as_ref())
            .map(|scope| scope.slots.len())
            .sum();
        if slots.len() > MAX_PERSISTENT_SUPPRESSION_REFERENCES.saturating_sub(retained) {
            return Err(EngineError::ResourceLimit.into());
        }
        let batch = self.new.get_mut(&id).ok_or(GeneratedError::Lifecycle)?;
        batch.scope = Some(ScopeDelivery {
            slots,
            opened: false,
            closed: false,
        });
        Ok(self)
    }

    pub(crate) fn commit(self) {
        for (id, batch) in self.new {
            self.ledger.batches.insert(id, batch);
        }
        for slot in self.ups {
            let Some(batch) = self.ledger.batches.get_mut(&slot.batch) else {
                unreachable!("prepared batch")
            };
            let Some(state) = batch.slots.get_mut(&slot.index) else {
                unreachable!("prepared slot")
            };
            state.up_generated = true;
        }
        self.ledger.reserved += self.reserved;
    }
}
impl BatchDelivery {
    fn matches(&self, event: &StageEvent<PlannedAction>) -> bool {
        self.device == event.device
            && self.source == event.source
            && Arc::ptr_eq(&self.config, &event.config)
    }
}
impl DeliveryLedger {
    pub(crate) fn reserved(&self) -> usize {
        self.reserved
    }
    pub(crate) fn batches(&self) -> usize {
        self.batches.len()
    }
    pub(crate) fn progress(&self, id: BatchId) -> Option<DeliveryProgress> {
        let batch = self.batches.get(&id)?;
        Some(DeliveryProgress {
            slots: batch.slots.len(),
            down_delivered: batch.slots.values().filter(|slot| slot.down).count(),
            up_generated: batch
                .slots
                .values()
                .filter(|slot| slot.up_generated)
                .count(),
            up_delivered: batch.slots.values().filter(|slot| slot.up).count(),
            scope_reservations: batch.extra,
        })
    }
    pub(crate) fn prepare(
        &mut self,
        events: &[StageEvent<PlannedAction>],
        extra: Option<(BatchId, usize)>,
    ) -> Result<DeliveryAppend<'_>, GeneratedError> {
        let mut new = BTreeMap::<BatchId, BatchDelivery>::new();
        let mut ups = Vec::new();
        let mut seen_ups = BTreeSet::new();
        let mut reserved = extra.map_or(0, |(_, count)| count);
        for event in events {
            let slot = event.payload.slot;
            match event.payload.edge {
                KeyEdge::Down => {
                    // Initial Down arrays admit a new batch. Existing batches
                    // accept only saved Up, even if their generated view is empty.
                    if self.batches.contains_key(&slot.batch) {
                        return Err(GeneratedError::Lifecycle);
                    }
                    let batch = new.entry(slot.batch).or_insert_with(|| BatchDelivery {
                        device: event.device,
                        source: event.source,
                        config: Arc::clone(&event.config),
                        slots: BTreeMap::new(),
                        scope: None,
                        extra: extra
                            .filter(|(id, _)| *id == slot.batch)
                            .map_or(0, |(_, count)| count),
                    });
                    if !batch.matches(event) {
                        return Err(GeneratedError::Provenance);
                    }
                    if batch.slots.contains_key(&slot.index) {
                        return Err(GeneratedError::Lifecycle);
                    }
                    batch.slots.insert(
                        slot.index,
                        SlotDelivery {
                            target: event.payload.target.clone(),
                            down: false,
                            up_generated: false,
                            up: false,
                        },
                    );
                    reserved += 1;
                }
                KeyEdge::Up => {
                    let batch = new
                        .get(&slot.batch)
                        .or_else(|| self.batches.get(&slot.batch))
                        .ok_or(GeneratedError::Lifecycle)?;
                    let state = batch
                        .slots
                        .get(&slot.index)
                        .ok_or(GeneratedError::Lifecycle)?;
                    if !batch.matches(event) || state.target != event.payload.target {
                        return Err(GeneratedError::Provenance);
                    }
                    if state.up_generated || !seen_ups.insert(slot) {
                        return Err(GeneratedError::Lifecycle);
                    }
                    ups.push(slot);
                }
                KeyEdge::Repeat => return Err(GeneratedError::Lifecycle),
            }
        }
        if self.batches.len().saturating_add(new.len()) > MAX_BINDINGS
            || self.reserved.saturating_add(reserved) > MAX_OUTPUT_OWNERS
        {
            return Err(EngineError::ResourceLimit.into());
        }
        Ok(DeliveryAppend {
            ledger: self,
            new,
            ups,
            reserved,
        })
    }
    pub(crate) fn delivered(&mut self, event: &StageEvent<PlannedAction>) {
        let Some(batch) = self.batches.get_mut(&event.payload.slot.batch) else {
            unreachable!("admitted delivery batch")
        };
        let Some(state) = batch.slots.get_mut(&event.payload.slot.index) else {
            unreachable!("admitted delivery slot")
        };
        match event.payload.edge {
            KeyEdge::Down => {
                debug_assert!(!state.down);
                state.down = true;
            }
            KeyEdge::Up => {
                debug_assert!(state.down && state.up_generated && !state.up);
                state.up = true;
            }
            KeyEdge::Repeat => unreachable!("admission rejects repeat"),
        }
    }
    pub(crate) fn pending_scope_sources(&self) -> BTreeSet<ActionSlot> {
        self.batches
            .values()
            .filter_map(|batch| batch.scope.as_ref())
            .filter(|scope| !scope.closed)
            .flat_map(|scope| scope.slots.iter().copied())
            .collect()
    }
    pub(crate) fn slots_for_device_removal(
        &self,
        device: DeviceInstanceId,
        removed: bool,
    ) -> Vec<ActionSlot> {
        self.batches
            .iter()
            .filter(|(_, batch)| {
                batch.device.id == device.id
                    && (batch.device.generation < device.generation
                        || (removed && batch.device.generation == device.generation))
            })
            .flat_map(|(&batch, state)| {
                state
                    .slots
                    .keys()
                    .map(move |&index| ActionSlot { batch, index })
            })
            .collect()
    }
    pub(crate) fn scope_complete(&self, id: BatchId) -> bool {
        self.batches
            .get(&id)
            .is_some_and(|batch| batch.scope.as_ref().is_none_or(|scope| scope.closed))
    }
    pub(crate) fn delivered_scope(
        &mut self,
        event: &StageEvent<crate::generated_stage::ScopeCommand>,
    ) -> Result<(), GeneratedError> {
        let command = &event.payload;
        let batch = self
            .batches
            .get_mut(&command.batch)
            .ok_or(GeneratedError::Lifecycle)?;
        if event.stage != crate::timeline::Stage::Complex
            || batch.device != event.device
            || batch.source != event.source
            || !Arc::ptr_eq(&batch.config, &event.config)
        {
            return Err(GeneratedError::Provenance);
        }
        let scope = batch.scope.as_mut().ok_or(GeneratedError::Lifecycle)?;
        if scope.slots != command.slots || scope.closed || (command.opening == scope.opened) {
            return Err(GeneratedError::Lifecycle);
        }
        if command.opening {
            scope.opened = true;
        } else {
            scope.closed = true;
        }
        Ok(())
    }
    pub(crate) fn retire(&mut self, id: BatchId) -> Result<(), GeneratedError> {
        let batch = self.batches.get(&id).ok_or(GeneratedError::Lifecycle)?;
        if batch.scope.as_ref().is_some_and(|scope| !scope.closed)
            || batch
                .slots
                .values()
                .any(|slot| !slot.down || !slot.up_generated || !slot.up)
        {
            return Err(GeneratedError::Lifecycle);
        }
        self.cancel(id);
        Ok(())
    }
    pub(crate) fn cancel(&mut self, id: BatchId) {
        if let Some(batch) = self.batches.remove(&id) {
            self.reserved -= batch.slots.len() + batch.extra;
        }
    }
    pub(crate) fn remove_device(&mut self, device: DeviceInstanceId, removed: bool) {
        let ids: Vec<_> = self
            .batches
            .iter()
            .filter(|(_, batch)| {
                batch.device.id == device.id
                    && (batch.device.generation < device.generation
                        || (removed && batch.device.generation == device.generation))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }
}
