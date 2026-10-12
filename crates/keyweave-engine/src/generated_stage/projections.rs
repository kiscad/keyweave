use super::*;
use crate::terminal_actor::{KeyProjectionId, ProjectionCommand};
pub(super) struct GeneratedKeyProjection {
    origin: action_plan::PlanOrigin,
    slots: Vec<ActionSlot>,
    logical_open: bool,
}
pub(crate) struct PreparedGeneratedKeyProjection<'a> {
    timeline: timeline::PreparedStageAppend<'a, GeneratedWork>,
    environment: &'a mut Environment,
    now: &'a mut MonoTime,
    at: MonoTime,
    id: KeyProjectionId,
    projection: GeneratedKeyProjection,
}
impl PreparedGeneratedKeyProjection<'_> {
    pub(crate) fn commit(self) {
        self.timeline.commit();
        for slot in &self.projection.slots {
            *self.environment.hidden.entry(*slot).or_default() += 1;
        }
        self.environment.references += self.projection.slots.len();
        self.environment
            .key_projections
            .insert(self.id, self.projection);
        *self.now = self.at;
    }
}
pub(crate) struct PreparedGeneratedKeyProjectionClose<'a> {
    timeline: timeline::PreparedStageAppend<'a, GeneratedWork>,
    environment: &'a mut Environment,
    now: &'a mut MonoTime,
    at: MonoTime,
    id: KeyProjectionId,
}
impl PreparedGeneratedKeyProjectionClose<'_> {
    pub(crate) fn commit(self) {
        self.timeline.commit();
        if let Some(projection) = self.environment.key_projections.get_mut(&self.id) {
            for slot in &projection.slots {
                if let Some(count) = self.environment.hidden.get_mut(slot) {
                    if *count > 1 {
                        *count -= 1;
                    } else {
                        self.environment.hidden.remove(slot);
                    }
                }
            }
            self.environment.references -= projection.slots.len();
            projection.logical_open = false;
        }
        *self.now = self.at;
    }
}
impl Environment {
    pub(super) fn cancel_key_projections(
        &mut self,
        matches: impl Fn(&action_plan::PlanOrigin) -> bool,
    ) {
        let ids: Vec<_> = self
            .key_projections
            .iter()
            .filter(|(_, projection)| matches(&projection.origin))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            if let Some(projection) = self.key_projections.remove(&id)
                && projection.logical_open
            {
                for slot in &projection.slots {
                    if let Some(count) = self.hidden.get_mut(slot) {
                        if *count > 1 {
                            *count -= 1;
                        } else {
                            self.hidden.remove(slot);
                        }
                    }
                }
                self.references -= projection.slots.len();
            }
        }
    }
}
impl GeneratedStageQueue {
    pub(crate) fn prepare_key_projection(
        &mut self,
        saved: &ScopedGeneration,
        slots: &[ActionSlot],
        id: KeyProjectionId,
        at: MonoTime,
    ) -> Result<PreparedGeneratedKeyProjection<'_>, GeneratedError> {
        if self.environment.stage != Stage::Output {
            return Err(GeneratedError::Stage);
        }
        if !Arc::ptr_eq(&saved.queue, &self.identity)
            || self.environment.key_projections.contains_key(&id)
            || self.delivery.progress(saved.plan.origin.batch).is_none()
        {
            return Err(GeneratedError::Provenance);
        }
        if self.environment.key_projections.len() >= MAX_BINDINGS
            || slots.len()
                > MAX_PERSISTENT_SUPPRESSION_REFERENCES.saturating_sub(self.environment.references)
        {
            return Err(EngineError::ResourceLimit.into());
        }
        let slots: BTreeSet<_> = slots.iter().copied().collect();
        for slot in &slots {
            if !saved.plan.events().iter().any(|event| {
                event.payload.slot == *slot
                    && event.payload.edge == KeyEdge::Down
                    && matches!(event.payload.target,ActionTarget::Key(key) if key.usage!=0)
            }) {
                return Err(GeneratedError::Provenance);
            }
        }
        let origin = saved.plan.origin.clone();
        let event = StageEvent {
            stage: origin.stage,
            at_ns: at,
            device: origin.device,
            source: origin.source,
            config: origin.config.clone(),
            payload: GeneratedWork::Projection(ProjectionCommand {
                id,
                opening: true,
                batch: origin.batch,
            }),
        };
        let timeline = self
            .timeline
            .prepare_enqueue_at(vec![event], at)
            .map_err(|rejected| GeneratedError::Timeline(rejected.error))?;
        Ok(PreparedGeneratedKeyProjection {
            timeline,
            environment: &mut self.environment,
            now: &mut self.now,
            at,
            id,
            projection: GeneratedKeyProjection {
                origin,
                slots: slots.into_iter().collect(),
                logical_open: true,
            },
        })
    }
    pub(crate) fn prepare_key_projection_close(
        &mut self,
        id: KeyProjectionId,
        at: MonoTime,
    ) -> Result<PreparedGeneratedKeyProjectionClose<'_>, GeneratedError> {
        let projection = self
            .environment
            .key_projections
            .get(&id)
            .ok_or(GeneratedError::Lifecycle)?;
        if !projection.logical_open {
            return Err(GeneratedError::Lifecycle);
        }
        let origin = &projection.origin;
        let event = StageEvent {
            stage: origin.stage,
            at_ns: at,
            device: origin.device,
            source: origin.source,
            config: origin.config.clone(),
            payload: GeneratedWork::Projection(ProjectionCommand {
                id,
                opening: false,
                batch: origin.batch,
            }),
        };
        let timeline = self
            .timeline
            .prepare_enqueue_at(vec![event], at)
            .map_err(|rejected| GeneratedError::Timeline(rejected.error))?;
        Ok(PreparedGeneratedKeyProjectionClose {
            timeline,
            environment: &mut self.environment,
            now: &mut self.now,
            at,
            id,
        })
    }
    pub(super) fn delivered_key_projection(
        &mut self,
        command: &ProjectionCommand,
    ) -> Result<(), GeneratedError> {
        let projection = self
            .environment
            .key_projections
            .get(&command.id)
            .ok_or(GeneratedError::Lifecycle)?;
        if projection.origin.batch != command.batch {
            return Err(GeneratedError::Provenance);
        }
        if !command.opening {
            if projection.logical_open {
                return Err(GeneratedError::Lifecycle);
            }
            self.environment.key_projections.remove(&command.id);
        }
        Ok(())
    }
}
