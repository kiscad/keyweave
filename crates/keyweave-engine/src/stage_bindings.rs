//! Bounded saved bindings, independent of platform output and source allocation.
use crate::{
    MAX_BINDINGS, MAX_OUTPUT_OWNERS,
    action_plan::{ActionSlot, BatchId, PlanOrigin},
    timeline::Stage,
};
use std::collections::BTreeMap;

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
    children: Vec<usize>,
}
/// Payloads represent executed downs only. Dropping this arena is not OS recovery.
pub struct BindingArena<T> {
    batches: BTreeMap<BatchId, Batch<T>>,
    reserved: usize,
    live: usize,
}
impl<T> Default for BindingArena<T> {
    fn default() -> Self {
        Self {
            batches: BTreeMap::new(),
            reserved: 0,
            live: 0,
        }
    }
}
/// Admission holds the arena borrow until commit. Aborting leaves it unchanged.
pub struct Reservation<'a, T> {
    arena: &'a mut BindingArena<T>,
    batch: Batch<T>,
}
impl<T> Reservation<'_, T> {
    pub fn commit(self) {
        if let Some(parent) = self.batch.parent {
            self.arena
                .batches
                .get_mut(&parent.batch)
                .unwrap_or_else(|| unreachable!("exclusive reservation retains parent"))
                .children[usize::from(parent.index)] += 1;
        }
        self.arena.reserved += self.batch.slots.len();
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
            || slots > MAX_OUTPUT_OWNERS.saturating_sub(self.reserved)
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
                children: vec![0; slots],
            },
        })
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
        if batch
            .children
            .get(index)
            .copied()
            .ok_or(BindingError::Lifecycle)?
            != 0
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
        let batch = self.batches.get(&id).ok_or(BindingError::Lifecycle)?;
        if batch.children.iter().any(|count| *count != 0) {
            return Err(BindingError::ChildPending);
        }
        let batch = self.batches.remove(&id).ok_or(BindingError::Lifecycle)?;
        if let Some(parent) = batch.parent {
            self.batches
                .get_mut(&parent.batch)
                .unwrap_or_else(|| unreachable!("children prevent parent retirement"))
                .children[usize::from(parent.index)] -= 1;
        }
        self.reserved -= batch.slots.len();
        let payloads: Vec<_> = batch
            .slots
            .into_iter()
            .rev()
            .filter_map(|slot| match slot {
                Slot::Live(payload) => Some(payload),
                _ => None,
            })
            .collect();
        self.live -= payloads.len();
        Ok(payloads)
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
