//! Bounded saved bindings, independent of platform output and source allocation.
mod retentions;
mod shared_downs;
use crate::{
    MAX_BINDINGS, MAX_OUTPUT_OWNERS,
    action_plan::{ActionSlot, BatchId, PlanOrigin, PlannedAction},
    timeline::{DueEvent, Stage},
};
use retentions::RetentionState;
pub use retentions::{
    MAX_RETENTION_GROUPS, MAX_RETENTION_MEMBERS, MAX_RETENTION_REFERENCES, PreparedSharedRetention,
    RejectedSharedRetention, RetentionKey, SharedRetention,
};
pub use shared_downs::{GroupDownInputs, RejectedGroupDowns, SharedGroupReservation};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    #[error("binding capacity exceeded")]
    Capacity,
    #[error("invalid binding lifecycle")]
    Lifecycle,
    #[error("invalid child binding provenance")]
    Provenance,
    #[error("binding still has child work")]
    ChildPending,
}
enum Slot<T> {
    Reserved,
    Live(T),
    Released,
}
struct Batch<T> {
    origin: PlanOrigin,
    parent: Option<ActionSlot>,
    slots: Vec<Slot<T>>,
    children: Vec<BTreeSet<BatchId>>,
    extra_sources: usize,
}
pub enum BindingState<'a, T> {
    Reserved,
    Live(&'a T),
    Released,
}
/// Borrowed metadata is inspection, not authority to allocate or release owners.
pub struct BindingNode<'a, T> {
    pub slot: ActionSlot,
    pub origin: &'a PlanOrigin,
    pub parent: Option<ActionSlot>,
    pub state: BindingState<'a, T>,
}
pub struct CanceledSlot<T> {
    pub slot: ActionSlot,
    pub payload: T,
}
/// Owned cleanup recipe. The arena does not call platform recovery; Drop is not proof.
pub struct CanceledBatch<T> {
    pub origin: PlanOrigin,
    pub parent: Option<ActionSlot>,
    pub payloads: Vec<CanceledSlot<T>>,
    /// Released accounting units, not evidence of actual owner/OS cleanup.
    pub extra_sources: usize,
}
/// Payloads represent executed downs only. Dropping this arena is not OS recovery.
pub struct BindingArena<T> {
    retentions: RetentionState,
    batches: BTreeMap<BatchId, Batch<T>>,
    reserved: usize,
    live: usize,
    extra_sources: usize,
}
impl<T> Default for BindingArena<T> {
    fn default() -> Self {
        Self {
            retentions: RetentionState::default(),
            batches: BTreeMap::new(),
            reserved: 0,
            live: 0,
            extra_sources: 0,
        }
    }
}
/// Admission holds the arena borrow until commit, including optional parent Down activation.
/// Aborting leaves it unchanged; pending payloads must have no executed effects.
pub struct Reservation<'a, T> {
    arena: &'a mut BindingArena<T>,
    batch: Batch<T>,
    activation: Option<(ActionSlot, T)>,
}
/// Activate a consumed producer Down without creating any downstream identity slot.
#[must_use = "commit or discard the consumed producer Down"]
pub struct PreparedConsumedDown<'a, T> {
    arena: &'a mut BindingArena<T>,
    slot: ActionSlot,
    payload: T,
    destination: PlanOrigin,
}
impl<T> PreparedConsumedDown<'_, T> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.arena.activate_down(self.slot, self.payload);
    }
    pub(crate) fn admission(&self) -> ChildAdmission<'_> {
        ChildAdmission {
            origin: &self.destination,
            parent: self.slot,
            slots: 0,
        }
    }
}
/// Only an exclusive child registration or consumed-Down activation creates this witness.
/// Parent is Live or its validated Down activation is owned by the same reservation.
/// It is semantic binding provenance, not native/session authorization.
pub(crate) struct ChildAdmission<'a> {
    origin: &'a PlanOrigin,
    parent: ActionSlot,
    slots: usize,
}
impl ChildAdmission<'_> {
    pub(crate) fn matches(&self, origin: &PlanOrigin, slots: usize) -> bool {
        self.slots == slots
            && self.origin.batch == origin.batch
            && self.origin.stage == origin.stage
            && self.origin.start == origin.start
            && self.origin.device == origin.device
            && self.origin.source == origin.source
            && std::sync::Arc::ptr_eq(&self.origin.config, &origin.config)
    }
    pub(crate) fn parent(&self) -> ActionSlot {
        self.parent
    }
}
impl<'a, T> Reservation<'a, T> {
    /// Add source units to this uncommitted batch. Failure/drop changes no arena state.
    /// Retain them until whole-batch retirement/cancellation, not individual slot Up.
    pub fn with_extra_sources(mut self, additional: usize) -> Result<Self, BindingError> {
        let available = MAX_OUTPUT_OWNERS
            .saturating_sub(self.arena.reserved_sources())
            .saturating_sub(self.batch.slots.len())
            .saturating_sub(self.batch.extra_sources);
        if additional > available {
            return Err(BindingError::Capacity);
        }
        self.batch.extra_sources += additional;
        Ok(self)
    }
    pub(crate) fn child_admission(&self) -> Result<ChildAdmission<'_>, BindingError> {
        Ok(ChildAdmission {
            origin: &self.batch.origin,
            parent: self.batch.parent.ok_or(BindingError::Provenance)?,
            slots: self.batch.slots.len(),
        })
    }
    pub fn commit(self) {
        let _ = self.commit_into_arena();
    }
    fn commit_into_arena(self) -> &'a mut BindingArena<T> {
        if let Some((slot, payload)) = self.activation {
            self.arena.activate_down(slot, payload);
        }
        if let Some(parent) = self.batch.parent {
            self.arena
                .batches
                .get_mut(&parent.batch)
                .unwrap_or_else(|| unreachable!("exclusive reservation retains parent"))
                .children[usize::from(parent.index)]
            .insert(self.batch.origin.batch);
        }
        self.arena.reserved += self.batch.slots.len();
        self.arena.extra_sources += self.batch.extra_sources;
        self.arena
            .batches
            .insert(self.batch.origin.batch, self.batch);
        self.arena
    }
}
impl<T> BindingArena<T> {
    pub fn has_child_work(&self, slot: ActionSlot) -> Result<bool, BindingError> {
        let batch = self
            .batches
            .get(&slot.batch)
            .ok_or(BindingError::Lifecycle)?;
        Ok(!batch
            .children
            .get(usize::from(slot.index))
            .ok_or(BindingError::Lifecycle)?
            .is_empty())
    }
    pub fn live_slots(&self) -> usize {
        self.live
    }
    pub fn reserved_slots(&self) -> usize {
        self.reserved
    }
    pub fn extra_source_reservations(&self) -> usize {
        self.extra_sources
    }
    /// Per-arena tree budget: identity slots and explicit extra source units.
    pub fn reserved_sources(&self) -> usize {
        self.reserved + self.extra_sources
    }
    pub fn prepare(
        &mut self,
        origin: PlanOrigin,
        slots: usize,
        parent: Option<ActionSlot>,
    ) -> Result<Reservation<'_, T>, BindingError> {
        self.prepare_inner(origin, slots, parent, None)
    }
    /// Atomically activate an already-delivered Reserved producer Down and admit its child.
    /// Payload must have no executed effects yet. Drop/rejection never creates a Live parent.
    /// Receipt consistency is not native authentication or proof of queue ownership.
    pub fn prepare_child_with_down(
        &mut self,
        origin: PlanOrigin,
        slots: usize,
        down: DueEvent<PlannedAction>,
        payload: T,
    ) -> Result<Reservation<'_, T>, BindingError> {
        let parent = self.validate_down(&down)?;
        if origin.start < down.delivered_at {
            return Err(BindingError::Provenance);
        }
        self.prepare_inner(origin, slots, Some(parent), Some((parent, payload)))
    }
    /// Prepare a consumed Down whose downstream main plan has no actions or releases.
    /// No child batch/edge/source reservation is created. Combine with empty destination admission.
    pub fn prepare_consumed_down(
        &mut self,
        destination: PlanOrigin,
        down: DueEvent<PlannedAction>,
        payload: T,
    ) -> Result<PreparedConsumedDown<'_, T>, BindingError> {
        let slot = self.validate_down(&down)?;
        if self.batches.contains_key(&destination.batch) {
            return Err(BindingError::Lifecycle);
        }
        if destination.start < down.delivered_at {
            return Err(BindingError::Provenance);
        }
        self.check_child_origin(slot, &destination)?;
        Ok(PreparedConsumedDown {
            arena: self,
            slot,
            payload,
            destination,
        })
    }
    fn validate_down(&self, down: &DueEvent<PlannedAction>) -> Result<ActionSlot, BindingError> {
        let event = &down.event;
        let slot = event.payload.slot;
        self.check_down(slot)?;
        let saved = &self
            .batches
            .get(&slot.batch)
            .ok_or(BindingError::Lifecycle)?
            .origin;
        if event.payload.edge != keyweave_types::KeyEdge::Down {
            return Err(BindingError::Lifecycle);
        }
        if event.stage != saved.stage
            || event.device != saved.device
            || event.source != saved.source
            || !std::sync::Arc::ptr_eq(&event.config, &saved.config)
            || event.at_ns < saved.start
            || down.delivered_at < event.at_ns
        {
            return Err(BindingError::Provenance);
        }
        Ok(slot)
    }
    fn check_child_origin(
        &self,
        parent: ActionSlot,
        origin: &PlanOrigin,
    ) -> Result<(), BindingError> {
        let saved = &self
            .batches
            .get(&parent.batch)
            .ok_or(BindingError::Lifecycle)?
            .origin;
        let next = match saved.stage {
            Stage::Simple => Stage::Complex,
            Stage::Complex => Stage::Output,
            Stage::Output => return Err(BindingError::Provenance),
        };
        if next != origin.stage
            || saved.device != origin.device
            || saved.source != origin.source
            || !std::sync::Arc::ptr_eq(&saved.config, &origin.config)
        {
            return Err(BindingError::Provenance);
        }
        Ok(())
    }
    fn activate_down(&mut self, slot: ActionSlot, payload: T) {
        self.batches
            .get_mut(&slot.batch)
            .unwrap_or_else(|| unreachable!("exclusive preparation retains reserved parent"))
            .slots[usize::from(slot.index)] = Slot::Live(payload);
        self.live += 1;
    }
    fn prepare_inner(
        &mut self,
        origin: PlanOrigin,
        slots: usize,
        parent: Option<ActionSlot>,
        activation: Option<(ActionSlot, T)>,
    ) -> Result<Reservation<'_, T>, BindingError> {
        self.check_reservation(
            &origin,
            slots,
            parent,
            activation.as_ref().map(|(slot, _)| *slot),
        )?;
        Ok(Reservation {
            arena: self,
            batch: Batch {
                origin,
                parent,
                slots: (0..slots).map(|_| Slot::Reserved).collect(),
                children: (0..slots).map(|_| BTreeSet::new()).collect(),
                extra_sources: 0,
            },
            activation,
        })
    }
    fn check_reservation(
        &self,
        origin: &PlanOrigin,
        slots: usize,
        parent: Option<ActionSlot>,
        activation: Option<ActionSlot>,
    ) -> Result<(), BindingError> {
        if self.batches.contains_key(&origin.batch) {
            return Err(BindingError::Lifecycle);
        }
        if slots == 0
            || slots > usize::from(u16::MAX) + 1
            || self.batches.len() >= MAX_BINDINGS
            || slots > MAX_OUTPUT_OWNERS.saturating_sub(self.reserved_sources())
        {
            return Err(BindingError::Capacity);
        }
        if let Some(parent) = parent {
            let batch = self
                .batches
                .get(&parent.batch)
                .ok_or(BindingError::Lifecycle)?;
            let valid_parent = match batch.slots.get(usize::from(parent.index)) {
                Some(Slot::Live(_)) => activation.is_none(),
                Some(Slot::Reserved) => activation == Some(parent),
                _ => false,
            };
            if !valid_parent {
                return Err(BindingError::Lifecycle);
            }
            self.check_child_origin(parent, origin)?;
        }
        Ok(())
    }
    /// Read an executed payload without changing its lifecycle or cloning it.
    /// Reserved/released/unknown slots have no actual payload.
    pub fn live(&self, slot: ActionSlot) -> Option<&T> {
        match self
            .batches
            .get(&slot.batch)?
            .slots
            .get(usize::from(slot.index))?
        {
            Slot::Live(payload) => Some(payload),
            Slot::Reserved | Slot::Released => None,
        }
    }
    pub fn check_down(&self, slot: ActionSlot) -> Result<(), BindingError> {
        match self
            .batches
            .get(&slot.batch)
            .and_then(|batch| batch.slots.get(usize::from(slot.index)))
        {
            Some(Slot::Reserved) => Ok(()),
            _ => Err(BindingError::Lifecycle),
        }
    }
    pub fn insert(&mut self, slot: ActionSlot, payload: T) -> Result<(), BindingError> {
        self.check_down(slot)?;
        self.batches
            .get_mut(&slot.batch)
            .ok_or(BindingError::Lifecycle)?
            .slots[usize::from(slot.index)] = Slot::Live(payload);
        self.live += 1;
        Ok(())
    }
    pub fn take(&mut self, slot: ActionSlot) -> Result<T, BindingError> {
        if self.retentions.has_slot(slot) {
            return Err(BindingError::ChildPending);
        }
        let batch = self
            .batches
            .get_mut(&slot.batch)
            .ok_or(BindingError::Lifecycle)?;
        let index = usize::from(slot.index);
        if !batch
            .children
            .get(index)
            .ok_or(BindingError::Lifecycle)?
            .is_empty()
        {
            return Err(BindingError::ChildPending);
        }
        let saved = batch.slots.get_mut(index).ok_or(BindingError::Lifecycle)?;
        if !matches!(saved, Slot::Live(_)) {
            return Err(BindingError::Lifecycle);
        }
        let Slot::Live(payload) = std::mem::replace(saved, Slot::Released) else {
            unreachable!()
        };
        self.live -= 1;
        Ok(payload)
    }
    /// Returns only executed downs, in reverse slot order. The caller must discard
    /// queued work before executing cleanup or resuming the actor.
    /// Parents must be canceled after their children.
    pub fn cancel(&mut self, id: BatchId) -> Result<Vec<T>, BindingError> {
        Ok(self
            .remove_batch(id)?
            .payloads
            .into_iter()
            .map(|slot| slot.payload)
            .collect())
    }
    fn remove_batch(&mut self, id: BatchId) -> Result<CanceledBatch<T>, BindingError> {
        if self.retentions.has_batch(id) {
            return Err(BindingError::ChildPending);
        }
        let batch = self.batches.get(&id).ok_or(BindingError::Lifecycle)?;
        if batch.children.iter().any(|children| !children.is_empty()) {
            return Err(BindingError::ChildPending);
        }
        let batch = self.batches.remove(&id).ok_or(BindingError::Lifecycle)?;
        if let Some(parent) = batch.parent {
            self.batches
                .get_mut(&parent.batch)
                .unwrap_or_else(|| unreachable!("children prevent parent retirement"))
                .children[usize::from(parent.index)]
            .remove(&id);
        }
        self.reserved -= batch.slots.len();
        self.extra_sources -= batch.extra_sources;
        let payloads: Vec<_> = batch
            .slots
            .into_iter()
            .enumerate()
            .rev()
            .filter_map(|(index, slot)| match slot {
                Slot::Live(payload) => Some(CanceledSlot {
                    slot: ActionSlot {
                        batch: id,
                        index: index as u16,
                    },
                    payload,
                }),
                _ => None,
            })
            .collect();
        self.live -= payloads.len();
        Ok(CanceledBatch {
            origin: batch.origin,
            parent: batch.parent,
            payloads,
            extra_sources: batch.extra_sources,
        })
    }
    /// Inspect one saved subtree without cloning payloads or changing lifecycle.
    /// Root must be Live; descendants include Reserved/Released slots so callers
    /// distinguish future work from actual bindings. Callback cannot mutate arena.
    pub fn visit_subtree(
        &self,
        root: ActionSlot,
        mut visit: impl FnMut(BindingNode<'_, T>),
    ) -> Result<(), BindingError> {
        let batch = self
            .batches
            .get(&root.batch)
            .ok_or(BindingError::Lifecycle)?;
        if !matches!(
            batch.slots.get(usize::from(root.index)),
            Some(Slot::Live(_))
        ) {
            return Err(BindingError::Lifecycle);
        }
        self.visit_node(root, &mut visit)
    }
    fn visit_node(
        &self,
        slot: ActionSlot,
        visit: &mut impl FnMut(BindingNode<'_, T>),
    ) -> Result<(), BindingError> {
        let batch = self
            .batches
            .get(&slot.batch)
            .ok_or(BindingError::Lifecycle)?;
        let index = usize::from(slot.index);
        let state = match batch.slots.get(index).ok_or(BindingError::Lifecycle)? {
            Slot::Reserved => BindingState::Reserved,
            Slot::Live(payload) => BindingState::Live(payload),
            Slot::Released => BindingState::Released,
        };
        visit(BindingNode {
            slot,
            origin: &batch.origin,
            parent: batch.parent,
            state,
        });
        for child in &batch.children[index] {
            let child_batch = self.batches.get(child).ok_or(BindingError::Lifecycle)?;
            for index in 0..child_batch.slots.len() {
                self.visit_node(
                    ActionSlot {
                        batch: *child,
                        index: index as u16,
                    },
                    visit,
                )?;
            }
        }
        Ok(())
    }
    fn cancellation_order(
        &self,
        id: BatchId,
        order: &mut Vec<BatchId>,
    ) -> Result<(), BindingError> {
        let batch = self.batches.get(&id).ok_or(BindingError::Lifecycle)?;
        for children in batch.children.iter().rev() {
            for child in children.iter().rev() {
                self.cancellation_order(*child, order)?;
            }
        }
        order.push(id);
        Ok(())
    }
    /// Detach a whole batch subtree, children first. Each returned record retains
    /// old provenance and only actual Live payloads. Caller must discard queued
    /// work for ALL returned batches before executing cleanup/resuming the actor.
    /// This does not execute output recovery or generate extra actions.
    pub fn cancel_subtree(&mut self, id: BatchId) -> Result<Vec<CanceledBatch<T>>, BindingError> {
        let mut order = Vec::new();
        self.cancellation_order(id, &mut order)?;
        if order.iter().any(|&id| self.retentions.has_batch(id)) {
            return Err(BindingError::ChildPending);
        }
        // Private strict next-stage edges make depth <= 3 and each batch has one
        // parent. Preflight completes before mutation; removal cannot fail after
        // its children have been detached under this exclusive borrow.
        let mut canceled = Vec::with_capacity(order.len());
        for id in order {
            canceled.push(
                self.remove_batch(id)
                    .unwrap_or_else(|_| unreachable!("preflighted postorder")),
            );
        }
        Ok(canceled)
    }
    pub fn retire(&mut self, id: BatchId) -> Result<(), BindingError> {
        let batch = self.batches.get(&id).ok_or(BindingError::Lifecycle)?;
        if batch
            .slots
            .iter()
            .any(|slot| !matches!(slot, Slot::Released))
        {
            return Err(BindingError::Lifecycle);
        }
        self.cancel(id)?;
        Ok(())
    }
}

impl BindingArena<crate::producer_decision::ProducerDecision> {
    /// Mark only a saved Live producer's original Up, independently of child completion.
    pub fn prepare_producer_up(
        &mut self,
        up: &DueEvent<PlannedAction>,
    ) -> Result<
        crate::producer_decision::PreparedProducerUp<'_>,
        crate::producer_decision::ProducerUpError,
    > {
        let slot = up.event.payload.slot;
        let saved = self
            .batches
            .get_mut(&slot.batch)
            .and_then(|batch| batch.slots.get_mut(usize::from(slot.index)));
        let Some(Slot::Live(decision)) = saved else {
            return Err(crate::producer_decision::ProducerUpError::Lifecycle);
        };
        decision.prepare_up(up)
    }
    /// Normal original-Up retirement: retained children must already be completed and detached.
    pub fn finish_producer(
        &mut self,
        slot: ActionSlot,
    ) -> Result<crate::producer_decision::ProducerDecision, crate::producer_decision::ProducerUpError>
    {
        if self
            .live(slot)
            .and_then(|decision| decision.up_delivered_at())
            .is_none()
        {
            return Err(crate::producer_decision::ProducerUpError::Lifecycle);
        }
        Ok(self.take(slot)?)
    }
}
