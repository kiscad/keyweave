use super::*;
/// Actor-local identity, not an owner SourceToken or native permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyProjectionId(u64);
pub(super) struct KeyProjection {
    pub(super) origin: action_plan::PlanOrigin,
    owners: Vec<(KeyId, SourceToken)>,
    active: bool,
    close_queued: bool,
}
impl TerminalActor {
    pub fn suppression_references(&self) -> usize {
        self.core.suppression_references
    }
    pub fn projection_is_active(&self, id: KeyProjectionId) -> bool {
        self.projections
            .get(&id)
            .is_some_and(|projection| projection.active)
    }
    /// Capture exact original key sources and queue a FIFO visibility control.
    /// Variables are cold rejected; actual owner IDs and bindings are never recreated.
    pub fn begin_key_projection(
        &mut self,
        batch: BatchId,
        at: MonoTime,
    ) -> Result<Option<KeyProjectionId>, ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        self.check_time(at)?;
        let saved = self
            .batches
            .get(&batch)
            .ok_or(ActorError::InvalidProjection)?;
        let mut owners = BTreeSet::new();
        for event in saved
            .plan
            .events()
            .iter()
            .filter(|event| event.payload.edge == KeyEdge::Down)
        {
            let action = &event.payload;
            let retained = self.bindings.live(action.slot).is_some()
                || self.bindings.check_down(action.slot).is_ok();
            if !retained {
                continue;
            }
            match &action.target {
                ActionTarget::Key(key) if key.usage != 0 => {
                    owners.insert((
                        *key,
                        SourceToken(saved.base_source + u64::from(action.slot.index)),
                    ));
                }
                ActionTarget::SetVariable(_) => {
                    return Err(ActorError::UnsupportedVariableProjection);
                }
                _ => {}
            }
        }
        if let Some(scope) = &saved.scope {
            if let Some(active) = &scope.active {
                owners.extend(active.temporary_owners());
            }
            if let Some(pending) = &scope.pending
                && pending.policy()
                    == rule_selection::BranchModifierPolicy::DownSnapshotMinusConsumed
            {
                owners.extend(
                    pending
                        .snapshot()
                        .iter()
                        .enumerate()
                        .map(|(index, key)| (*key, SourceToken(scope.source + index as u64))),
                );
            }
        }
        if owners.is_empty() {
            return Ok(None);
        }
        if self.projections.len() >= MAX_BINDINGS
            || owners.len()
                > MAX_PERSISTENT_SUPPRESSION_REFERENCES
                    .saturating_sub(self.core.suppression_references)
        {
            return Err(ActorError::Capacity);
        }
        let id = KeyProjectionId(
            self.next_projection
                .checked_add(1)
                .ok_or(EngineError::ResourceLimit)?,
        );
        let origin = saved.plan.origin.clone();
        let event = timeline::StageEvent {
            stage: Stage::Output,
            at_ns: at,
            device: origin.device,
            source: origin.source,
            config: origin.config.clone(),
            payload: TerminalWork::ProjectionDown(id, batch),
        };
        self.timeline
            .prepare_enqueue_at(vec![event], at)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?
            .commit();
        self.core.suppression_references += owners.len();
        self.projections.insert(
            id,
            KeyProjection {
                origin,
                owners: owners.into_iter().collect(),
                active: false,
                close_queued: false,
            },
        );
        self.next_projection = id.0;
        self.core.now = at;
        Ok(Some(id))
    }
    /// FIFO close is queued once; rejection leaves the same token usable for retry.
    pub fn close_key_projection(
        &mut self,
        id: KeyProjectionId,
        at: MonoTime,
    ) -> Result<(), ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        self.check_time(at)?;
        let projection = self
            .projections
            .get_mut(&id)
            .ok_or(ActorError::InvalidProjection)?;
        if projection.close_queued {
            return Err(ActorError::InvalidProjection);
        }
        let origin = &projection.origin;
        let event = timeline::StageEvent {
            stage: Stage::Output,
            at_ns: at,
            device: origin.device,
            source: origin.source,
            config: origin.config.clone(),
            payload: TerminalWork::ProjectionUp(id, origin.batch),
        };
        self.timeline
            .prepare_enqueue_at(vec![event], at)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?
            .commit();
        projection.close_queued = true;
        self.core.now = at;
        Ok(())
    }
    fn finish_key_projection(&mut self, id: KeyProjectionId) -> Result<Vec<KeyId>, ActorError> {
        let projection = self
            .projections
            .remove(&id)
            .ok_or(ActorError::InvalidProjection)?;
        if projection.active {
            for owner in &projection.owners {
                if let Some(count) = self.core.hidden.get_mut(owner) {
                    if *count > 1 {
                        *count -= 1;
                    } else {
                        self.core.hidden.remove(owner);
                    }
                }
            }
        }
        self.core.suppression_references = self
            .core
            .suppression_references
            .checked_sub(projection.owners.len())
            .ok_or(ActorError::InvalidProjection)?;
        Ok(projection.owners.into_iter().map(|(key, _)| key).collect())
    }
    pub(super) fn cancel_key_projections(&mut self, batch: BatchId) -> Result<(), ActorError> {
        let ids: Vec<_> = self
            .projections
            .iter()
            .filter(|(_, projection)| projection.origin.batch == batch)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.finish_key_projection(id)?;
        }
        self.timeline.discard_where(|event| matches!(event.payload, TerminalWork::ProjectionDown(_, target) | TerminalWork::ProjectionUp(_, target) if target == batch));
        Ok(())
    }
    pub(super) fn deliver_key_projection(
        &mut self,
        id: KeyProjectionId,
        opening: bool,
    ) -> Result<TerminalDelivery, ActorError> {
        let mut outputs = Vec::new();
        let keys = if opening {
            let projection = self
                .projections
                .get_mut(&id)
                .ok_or(ActorError::InvalidProjection)?;
            if projection.active {
                self.failed = true;
                return Err(ActorError::InvalidProjection);
            }
            for owner in &projection.owners {
                *self.core.hidden.entry(*owner).or_default() += 1;
            }
            projection.active = true;
            projection
                .owners
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>()
        } else {
            self.finish_key_projection(id)?
        };
        for key in keys.into_iter().collect::<BTreeSet<_>>() {
            if !key.is_modifier() {
                self.core.sync_key(key, &mut outputs);
            }
        }
        Ok(TerminalDelivery {
            step: ActorStep {
                outputs,
                halt_requested: false,
                completed_batch: None,
            },
            action: None,
            pointer: None,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn actor() -> TerminalActor {
        let mut actor =
            TerminalActor::new(Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))).unwrap();
        actor
            .observe_device(
                DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                false,
                MonoTime(0),
            )
            .unwrap();
        actor
    }
    fn plan(actor: &TerminalActor, id: u64, tap: bool) -> ActionPlan {
        action_plan::plan_actions(
            &vec![
                Action {
                    hold_down_ns: 1,
                    .."x".parse::<KeyId>().unwrap().into()
                };
                512
            ],
            action_plan::PlanOrigin {
                batch: BatchId(id),
                stage: Stage::Output,
                start: MonoTime(0),
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                source: SourceToken(id),
                config: actor.snapshot().clone(),
            },
            tap,
        )
        .unwrap()
    }
    #[test]
    fn queued_projection_references_are_bounded_before_visibility_changes() {
        let mut actor = actor();
        actor.enqueue(plan(&actor, 1, false)).unwrap();
        for _ in 0..32 {
            actor
                .begin_key_projection(BatchId(1), MonoTime(0))
                .unwrap()
                .unwrap();
        }
        assert_eq!(
            actor.suppression_references(),
            MAX_PERSISTENT_SUPPRESSION_REFERENCES
        );
        let queued = actor.queued_events();
        let cursor = actor.next_projection;
        assert!(matches!(
            actor.begin_key_projection(BatchId(1), MonoTime(100)),
            Err(ActorError::Capacity)
        ));
        assert_eq!(actor.next_projection, cursor);
        assert_eq!(actor.queued_events(), queued);
        assert_eq!(actor.core.now, MonoTime(0));
        assert!(actor.reset(MonoTime(0)).unwrap().is_empty());
        assert_eq!(actor.suppression_references(), 0);
    }
    #[test]
    fn full_fifo_rejection_preserves_projection_identity_references_and_clock() {
        let mut actor = actor();
        for id in 1..=8 {
            actor.enqueue(plan(&actor, id, true)).unwrap();
        }
        assert_eq!(actor.queued_events(), timeline::MAX_STAGE_EVENTS);
        assert!(matches!(
            actor.begin_key_projection(BatchId(1), MonoTime(100)),
            Err(ActorError::Timeline(TimelineError::Capacity))
        ));
        assert_eq!(actor.next_projection, 0);
        assert_eq!(actor.suppression_references(), 0);
        assert_eq!(actor.core.now, MonoTime(0));
        assert!(actor.projections.is_empty());
    }
    #[test]
    fn close_rejection_keeps_token_retryable_until_one_fifo_slot_is_freed() {
        let mut actor = actor();
        actor.enqueue(plan(&actor, 1, true)).unwrap();
        let scope = actor
            .begin_key_projection(BatchId(1), MonoTime(0))
            .unwrap()
            .unwrap();
        for id in 2..=7 {
            actor.enqueue(plan(&actor, id, true)).unwrap();
        }
        actor.enqueue(plan(&actor, 8, false)).unwrap();
        assert_eq!(actor.queued_events(), timeline::MAX_STAGE_EVENTS);
        assert!(matches!(
            actor.close_key_projection(scope, MonoTime(100)),
            Err(ActorError::Timeline(TimelineError::Capacity))
        ));
        assert!(!actor.projections[&scope].close_queued);
        assert_eq!(actor.core.now, MonoTime(0));
        actor.poll(MonoTime(0)).unwrap().unwrap();
        actor.close_key_projection(scope, MonoTime(100)).unwrap();
        assert!(actor.projections[&scope].close_queued);
        assert_eq!(actor.suppression_references(), 512);
    }
}
