use super::*;
use std::sync::Arc;
pub const MAX_RETENTION_GROUPS: usize = 4096;
pub const MAX_RETENTION_REFERENCES: usize = 16384;
pub const MAX_RETENTION_MEMBERS: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RetentionKey(u64);
/// Arena-local ownership handle. Drop does not release metadata or native output.
#[must_use = "keep the handle until explicit shared retention release"]
pub struct SharedRetention {
    identity: Arc<()>,
    key: RetentionKey,
}
impl std::fmt::Debug for SharedRetention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRetention")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}
impl SharedRetention {
    pub fn key(&self) -> RetentionKey {
        self.key
    }
}
#[derive(Debug)]
pub struct RejectedSharedRetention {
    pub error: BindingError,
    pub handle: SharedRetention,
}
pub(super) struct RetentionState {
    identity: Arc<()>,
    next: u64,
    entries: BTreeMap<RetentionKey, Vec<ActionSlot>>,
    by_slot: BTreeMap<ActionSlot, BTreeSet<RetentionKey>>,
    references: usize,
}
impl Default for RetentionState {
    fn default() -> Self {
        Self {
            identity: Arc::new(()),
            next: 0,
            entries: BTreeMap::new(),
            by_slot: BTreeMap::new(),
            references: 0,
        }
    }
}
impl RetentionState {
    pub(super) fn has_slot(&self, slot: ActionSlot) -> bool {
        self.by_slot.contains_key(&slot)
    }
    pub(super) fn has_batch(&self, id: BatchId) -> bool {
        self.batch_keys(id).next().is_some()
    }
    fn batch_keys(&self, id: BatchId) -> impl Iterator<Item = RetentionKey> + '_ {
        self.by_slot
            .range(
                ActionSlot {
                    batch: id,
                    index: 0,
                }..=ActionSlot {
                    batch: id,
                    index: u16::MAX,
                },
            )
            .flat_map(|(_, keys)| keys.iter().copied())
    }
    fn install(&mut self, key: RetentionKey, slots: Vec<ActionSlot>) -> SharedRetention {
        for &slot in &slots {
            self.by_slot.entry(slot).or_default().insert(key);
        }
        self.references += slots.len();
        self.entries.insert(key, slots);
        self.next = key.0;
        SharedRetention {
            identity: self.identity.clone(),
            key,
        }
    }
}
#[must_use = "commit or abort the complete shared retention"]
pub struct PreparedSharedRetention<'a, T> {
    arena: &'a mut BindingArena<T>,
    key: RetentionKey,
    slots: Vec<ActionSlot>,
}
impl<T> PreparedSharedRetention<'_, T> {
    pub fn abort(self) -> Vec<ActionSlot> {
        self.slots
    }
    pub fn commit(self) -> SharedRetention {
        self.arena.retentions.install(self.key, self.slots)
    }
}
impl<T> BindingArena<T> {
    /// Retain same-stage Reserved/Live participants without activating payloads or output.
    pub fn prepare_shared_retention(
        &mut self,
        slots: Vec<ActionSlot>,
    ) -> Result<PreparedSharedRetention<'_, T>, BindingError> {
        if slots.is_empty()
            || slots.len() > MAX_RETENTION_MEMBERS
            || self.retentions.entries.len() >= MAX_RETENTION_GROUPS
            || slots.len() > MAX_RETENTION_REFERENCES.saturating_sub(self.retentions.references)
        {
            return Err(BindingError::Capacity);
        }
        let key = RetentionKey(
            self.retentions
                .next
                .checked_add(1)
                .ok_or(BindingError::Capacity)?,
        );
        let mut unique = BTreeSet::new();
        let mut stage = None;
        for &slot in &slots {
            if !unique.insert(slot) {
                return Err(BindingError::Lifecycle);
            }
            let batch = self
                .batches
                .get(&slot.batch)
                .ok_or(BindingError::Lifecycle)?;
            if !matches!(
                batch.slots.get(usize::from(slot.index)),
                Some(Slot::Reserved | Slot::Live(_))
            ) {
                return Err(BindingError::Lifecycle);
            }
            if stage.is_some_and(|stage| stage != batch.origin.stage) {
                return Err(BindingError::Provenance);
            }
            stage = Some(batch.origin.stage);
        }
        Ok(PreparedSharedRetention {
            arena: self,
            key,
            slots,
        })
    }
    pub fn shared_retention_groups(&self) -> usize {
        self.retentions.entries.len()
    }
    pub fn shared_retention_references(&self) -> usize {
        self.retentions.references
    }
    /// Read-only keys are not handles and cannot grant release authority.
    pub fn blocking_retentions(&self, slot: ActionSlot) -> Vec<RetentionKey> {
        self.retentions
            .by_slot
            .get(&slot)
            .map_or_else(Vec::new, |keys| keys.iter().copied().collect())
    }
    pub fn blocking_subtree_retentions(
        &self,
        id: BatchId,
    ) -> Result<Vec<RetentionKey>, BindingError> {
        let mut order = Vec::new();
        self.cancellation_order(id, &mut order)?;
        Ok(order
            .into_iter()
            .flat_map(|id| self.retentions.batch_keys(id))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
    pub fn retained_members(
        &self,
        handle: &SharedRetention,
    ) -> Result<&[ActionSlot], BindingError> {
        if !Arc::ptr_eq(&self.retentions.identity, &handle.identity) {
            return Err(BindingError::Provenance);
        }
        self.retentions
            .entries
            .get(&handle.key)
            .map(Vec::as_slice)
            .ok_or(BindingError::Lifecycle)
    }
    /// Only metadata release. Caller must separately prove output/scopes/control cleanup.
    pub fn release_shared_retention(
        &mut self,
        handle: SharedRetention,
    ) -> Result<Vec<ActionSlot>, RejectedSharedRetention> {
        let checked = self.retained_members(&handle).and_then(|members| {
            if members.iter().any(|slot| {
                !self
                    .retentions
                    .by_slot
                    .get(slot)
                    .is_some_and(|keys| keys.contains(&handle.key))
            }) {
                return Err(BindingError::Provenance);
            }
            Ok(())
        });
        if let Err(error) = checked {
            return Err(RejectedSharedRetention { error, handle });
        }
        let slots = self
            .retentions
            .entries
            .remove(&handle.key)
            .unwrap_or_else(|| unreachable!("preflighted retention under exclusive borrow"));
        for slot in &slots {
            let keys = self
                .retentions
                .by_slot
                .get_mut(slot)
                .unwrap_or_else(|| unreachable!("preflighted reverse reference"));
            keys.remove(&handle.key);
            if keys.is_empty() {
                self.retentions.by_slot.remove(slot);
            }
        }
        self.retentions.references -= slots.len();
        Ok(slots)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn arena() -> BindingArena<u8> {
        let mut arena = BindingArena::default();
        arena
            .prepare(
                PlanOrigin {
                    batch: BatchId(1),
                    stage: Stage::Complex,
                    start: keyweave_types::MonoTime(0),
                    device: keyweave_types::DeviceInstanceId {
                        id: 1,
                        generation: 1,
                    },
                    source: keyweave_types::SourceToken(1),
                    config: Arc::new(crate::ConfigSnapshot::empty(
                        keyweave_types::ConfigGeneration(1),
                    )),
                },
                1,
                None,
            )
            .unwrap()
            .commit();
        arena
    }
    #[test]
    fn aborted_prepare_does_not_consume_id_and_counter_exhaustion_never_wraps() {
        let mut arena = arena();
        let slot = ActionSlot {
            batch: BatchId(1),
            index: 0,
        };
        arena.retentions.next = u64::MAX - 1;
        drop(arena.prepare_shared_retention(vec![slot]).unwrap());
        assert_eq!(arena.retentions.next, u64::MAX - 1);
        let handle = arena.prepare_shared_retention(vec![slot]).unwrap().commit();
        assert_eq!(handle.key(), RetentionKey(u64::MAX));
        arena.release_shared_retention(handle).unwrap();
        assert!(matches!(
            arena.prepare_shared_retention(vec![slot]),
            Err(BindingError::Capacity)
        ));
        assert_eq!(arena.shared_retention_references(), 0);
        assert_eq!(arena.retentions.next, u64::MAX);
    }
    #[test]
    fn inconsistent_reverse_link_rejection_returns_handle_before_any_metadata_change() {
        let mut arena = arena();
        let slot = ActionSlot {
            batch: BatchId(1),
            index: 0,
        };
        let handle = arena.prepare_shared_retention(vec![slot]).unwrap().commit();
        let key = handle.key();
        arena
            .retentions
            .by_slot
            .get_mut(&slot)
            .unwrap()
            .remove(&key);
        let rejected = arena.release_shared_retention(handle).unwrap_err();
        assert_eq!(rejected.error, BindingError::Provenance);
        assert_eq!(arena.shared_retention_references(), 1);
        assert_eq!(arena.shared_retention_groups(), 1);
        arena.retentions.by_slot.get_mut(&slot).unwrap().insert(key);
        arena.release_shared_retention(rejected.handle).unwrap();
        assert_eq!(arena.shared_retention_references(), 0);
    }
}
