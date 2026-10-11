//! Bounded saved bindings, independent of platform output and source allocation.
use crate::{
    MAX_BINDINGS, MAX_OUTPUT_OWNERS,
    action_plan::{ActionSlot, BatchId, PlanOrigin},
    timeline::Stage,
};
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
    batches: BTreeMap<BatchId, Batch<T>>,
    reserved: usize,
    live: usize,
    extra_sources: usize,
}
impl<T> Default for BindingArena<T> {
    fn default() -> Self {
        Self {
            batches: BTreeMap::new(),
            reserved: 0,
            live: 0,
            extra_sources: 0,
        }
    }
}
/// Admission holds the arena borrow until commit. Aborting leaves it unchanged.
pub struct Reservation<'a, T> {
    arena: &'a mut BindingArena<T>,
    batch: Batch<T>,
}
/// Only an exclusively prepared child registration can create this witness.
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
impl<T> Reservation<'_, T> {
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
    }
}
impl<T> BindingArena<T> {
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
            if !matches!(
                batch.slots.get(usize::from(parent.index)),
                Some(Slot::Live(_))
            ) {
                return Err(BindingError::Lifecycle);
            }
            let next = match batch.origin.stage {
                Stage::Simple => Stage::Complex,
                Stage::Complex => Stage::Output,
                Stage::Output => return Err(BindingError::Provenance),
            };
            if next != origin.stage
                || batch.origin.device != origin.device
                || batch.origin.source != origin.source
                || !std::sync::Arc::ptr_eq(&batch.origin.config, &origin.config)
            {
                return Err(BindingError::Provenance);
            }
        }
        Ok(Reservation {
            arena: self,
            batch: Batch {
                origin,
                parent,
                slots: (0..slots).map(|_| Slot::Reserved).collect(),
                children: (0..slots).map(|_| BTreeSet::new()).collect(),
                extra_sources: 0,
            },
        })
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
