use super::*;
use crate::terminal_actor::{KeyProjectionId, ProjectionCommand};
pub(super) struct GeneratedKeyProjection {
    origin: action_plan::PlanOrigin,
    slots: Vec<ActionSlot>,
    logical_open: bool,
    variables: Vec<(ActionSlot, ActionTarget)>,
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
        self.environment
            .install_key_projection(self.id, self.projection);
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
        self.environment.restore_projection_value(self.id);
        *self.now = self.at;
    }
}
impl Environment {
    fn restore_projection_value(&mut self, id: KeyProjectionId) {
        if let Some(projection) = self.key_projections.get_mut(&id)
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
            for (slot, target) in &projection.variables {
                if !self.hidden.contains_key(slot)
                    && self.down.contains_key(slot)
                    && let Some((name, value)) = variable_update(target, KeyEdge::Down)
                {
                    self.variables.insert(name.to_owned(), value);
                }
            }
            self.references -= projection.slots.len();
            projection.logical_open = false;
        }
    }

    pub(super) fn install_key_projection(
        &mut self,
        id: KeyProjectionId,
        projection: GeneratedKeyProjection,
    ) {
        for (slot, target) in &projection.variables {
            if !self.hidden.contains_key(slot)
                && self.down.contains_key(slot)
                && let Some((name, value)) = variable_update(target, KeyEdge::Up)
            {
                self.variables.insert(name.to_owned(), value);
            }
        }
        for slot in &projection.slots {
            *self.hidden.entry(*slot).or_default() += 1;
        }
        self.references += projection.slots.len();
        self.key_projections.insert(id, projection);
    }

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
    pub(crate) fn check_projection_control(
        &self,
        id: KeyProjectionId,
        at: MonoTime,
    ) -> Result<&action_plan::PlanOrigin, GeneratedError> {
        if at < self.now {
            return Err(TimelineError::TimeReversed.into());
        }
        let projection = self
            .environment
            .key_projections
            .get(&id)
            .ok_or(GeneratedError::Lifecycle)?;
        if projection.logical_open
            && (projection.slots.len() > self.environment.references
                || projection.slots.iter().any(|slot| {
                    !self
                        .environment
                        .hidden
                        .get(slot)
                        .is_some_and(|count| *count > 0)
                }))
        {
            return Err(GeneratedError::Lifecycle);
        }
        Ok(&projection.origin)
    }
    pub(crate) fn restore_projection_control(
        &mut self,
        id: KeyProjectionId,
        at: MonoTime,
    ) -> Result<(), GeneratedError> {
        self.check_projection_control(id, at)?;
        self.environment.restore_projection_value(id);
        self.environment.key_projections.remove(&id);
        self.timeline.discard_where(|event| matches!(&event.payload, GeneratedWork::Projection(command) if command.id == id));
        self.advance(at)?;
        Ok(())
    }

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
        let mut variables = Vec::new();
        for slot in &slots {
            let action = saved
                .plan
                .events()
                .iter()
                .find(|event| event.payload.slot == *slot && event.payload.edge == KeyEdge::Down)
                .ok_or(GeneratedError::Provenance)?;
            match &action.payload.target {
                ActionTarget::Key(key) if key.usage != 0 => {}
                ActionTarget::SetVariable(_) => {
                    variables.push((*slot, action.payload.target.clone()))
                }
                _ => return Err(GeneratedError::Provenance),
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
                variables,
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

impl PreparedScopedGeneration<'_> {
    pub(crate) fn projected(mut self, id: KeyProjectionId) -> Result<Self, GeneratedError> {
        let env = &self.append.append.environment;
        if env.stage != Stage::Output {
            return Err(GeneratedError::Stage);
        }
        let actions: Vec<_> = self
            .saved
            .plan
            .events()
            .iter()
            .filter(|event| {
                event.payload.edge == KeyEdge::Down
                    && !matches!(event.payload.target,ActionTarget::Key(key) if key.usage==0)
            })
            .map(|event| &event.payload)
            .collect();
        let cost = actions.len().saturating_add(
            self.append
                .scope
                .as_ref()
                .map_or(0, |(_, scope)| scope.hidden.len()),
        );
        if env.key_projections.len() >= MAX_BINDINGS
            || env.key_projections.contains_key(&id)
            || cost > MAX_PERSISTENT_SUPPRESSION_REFERENCES.saturating_sub(env.references)
        {
            return Err(EngineError::ResourceLimit.into());
        }
        let slots = actions.iter().map(|action| action.slot).collect();
        let variables = actions
            .iter()
            .filter(|action| matches!(action.target, ActionTarget::SetVariable(_)))
            .map(|action| (action.slot, action.target.clone()))
            .collect();
        self.append.birth_projection = Some((
            id,
            GeneratedKeyProjection {
                origin: self.saved.plan.origin.clone(),
                slots,
                variables,
                logical_open: true,
            },
        ));
        Ok(self)
    }
}
