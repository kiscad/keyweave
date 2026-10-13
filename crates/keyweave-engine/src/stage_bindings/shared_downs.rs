use super::*;
pub type GroupDownInputs<T> = Vec<(DueEvent<PlannedAction>, T)>;
pub struct RejectedGroupDowns<T> {
    pub error: BindingError,
    pub inputs: GroupDownInputs<T>,
}
impl<T> std::fmt::Debug for RejectedGroupDowns<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedGroupDowns")
            .field("error", &self.error)
            .field("input_count", &self.inputs.len())
            .finish_non_exhaustive()
    }
}
/// Exclusive participant activations, single primary child and shared retention preparation.
#[must_use = "commit or abort all shared participant Downs and child registration"]
pub struct SharedGroupReservation<'a, T> {
    base: Reservation<'a, T>,
    primary: DueEvent<PlannedAction>,
    others: GroupDownInputs<T>,
    key: RetentionKey,
    members: Vec<ActionSlot>,
}
impl<T> SharedGroupReservation<'_, T> {
    pub fn abort(mut self) -> GroupDownInputs<T> {
        let (_, payload) = self
            .base
            .activation
            .take()
            .unwrap_or_else(|| unreachable!("shared group owns primary activation"));
        let mut inputs = Vec::with_capacity(self.others.len() + 1);
        inputs.push((self.primary, payload));
        inputs.extend(self.others);
        inputs
    }
    pub fn commit(self) -> SharedRetention {
        let arena = self.base.commit_into_arena();
        for (down, payload) in self.others {
            arena.activate_down(down.event.payload.slot, payload);
        }
        arena.retentions.install(self.key, self.members)
    }
    pub(crate) fn with_extra_sources(mut self, extra: usize) -> Result<Self, BindingError> {
        self.base = self.base.with_extra_sources(extra)?;
        Ok(self)
    }
    pub(crate) fn child_admission(&self) -> Result<ChildAdmission<'_>, BindingError> {
        self.base.child_admission()
    }
}
/// All participant activations and one shared hold, without a fictitious child batch.
#[must_use = "commit or abort all consumed group Downs and their shared retention"]
pub struct SharedConsumedDown<'a, T> {
    arena: &'a mut BindingArena<T>,
    inputs: GroupDownInputs<T>,
    destination: PlanOrigin,
    key: RetentionKey,
    members: Vec<ActionSlot>,
}
impl<T> SharedConsumedDown<'_, T> {
    pub fn abort(self) -> GroupDownInputs<T> {
        self.inputs
    }
    pub fn commit(self) -> SharedRetention {
        for (down, payload) in self.inputs {
            self.arena.activate_down(down.event.payload.slot, payload);
        }
        self.arena.retentions.install(self.key, self.members)
    }
    pub(crate) fn admission(&self) -> ChildAdmission<'_> {
        ChildAdmission {
            origin: &self.destination,
            parent: self.members[0],
            slots: 0,
        }
    }
}
impl<T> BindingArena<T> {
    fn check_group_downs(
        &self,
        origin: &PlanOrigin,
        inputs: &GroupDownInputs<T>,
    ) -> Result<(RetentionKey, Vec<ActionSlot>), BindingError> {
        if inputs.is_empty() || inputs.len() > MAX_RETENTION_MEMBERS {
            return Err(BindingError::Capacity);
        }
        let mut members = Vec::with_capacity(inputs.len());
        for (down, _) in inputs {
            let member = self.validate_down(down)?;
            if origin.start < down.delivered_at {
                return Err(BindingError::Provenance);
            }
            members.push(member);
        }
        let key = self.check_shared_retention(&members)?;
        Ok((key, members))
    }
    /// Consume a matched group with no main actions; jointly prepare the empty destination.
    pub fn prepare_shared_consumed_down(
        &mut self,
        destination: PlanOrigin,
        inputs: GroupDownInputs<T>,
    ) -> Result<SharedConsumedDown<'_, T>, RejectedGroupDowns<T>> {
        let checked = (|| {
            let (key, members) = self.check_group_downs(&destination, &inputs)?;
            if self.batches.contains_key(&destination.batch) {
                return Err(BindingError::Lifecycle);
            }
            self.check_child_origin(members[0], &destination)?;
            Ok((key, members))
        })();
        let (key, members) = match checked {
            Ok(prepared) => prepared,
            Err(error) => return Err(RejectedGroupDowns { error, inputs }),
        };
        Ok(SharedConsumedDown {
            arena: self,
            inputs,
            destination,
            key,
            members,
        })
    }
    /// Every payload must still be effect-free. Rejection returns every original owned input.
    pub fn prepare_shared_child_with_down(
        &mut self,
        origin: PlanOrigin,
        slots: usize,
        mut inputs: GroupDownInputs<T>,
    ) -> Result<SharedGroupReservation<'_, T>, RejectedGroupDowns<T>> {
        let checked = (|| {
            let (key, members) = self.check_group_downs(&origin, &inputs)?;
            self.check_reservation(&origin, slots, Some(members[0]), Some(members[0]))?;
            Ok((key, members))
        })();
        let (key, members) = match checked {
            Ok(prepared) => prepared,
            Err(error) => return Err(RejectedGroupDowns { error, inputs }),
        };
        let (primary, payload) = inputs.remove(0);
        let base = Reservation {
            arena: self,
            batch: Batch {
                origin,
                parent: Some(members[0]),
                slots: (0..slots).map(|_| Slot::Reserved).collect(),
                children: (0..slots).map(|_| BTreeSet::new()).collect(),
                extra_sources: 0,
            },
            activation: Some((members[0], payload)),
        };
        Ok(SharedGroupReservation {
            base,
            primary,
            others: inputs,
            key,
            members,
        })
    }
}
