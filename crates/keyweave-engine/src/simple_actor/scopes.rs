use super::*;
use crate::{
    complex_actor::ComplexProjectionId, generated_stage::ScopeCommand, timeline::StageEvent,
};
pub(super) struct ScopeRun {
    open: StageEvent<ScopeCommand>,
    references: Vec<(ComplexProjectionId, DeviceInstanceId)>,
}
impl SimpleStageActor {
    pub(super) fn ready_scope_close(&self) -> bool {
        self.deferred_scope_closes
            .front()
            .is_some_and(|due| self.complex.input_up_complete(due.event.payload.batch))
    }
    pub(super) fn remember_scope_source_removal(&mut self, canceled: &[ActionSlot]) {
        let pending = self.generated.pending_scope_sources();
        self.removed_scope_sources
            .retain(|slot| pending.contains(slot));
        self.removed_scope_sources.extend(
            canceled
                .iter()
                .copied()
                .filter(|slot| pending.contains(slot)),
        );
    }

    pub(super) fn deliver_scope(
        &mut self,
        due: DueEvent<ScopeCommand>,
    ) -> Result<SimpleStep, SimpleActorError> {
        if due.event.stage != Stage::Complex {
            return Err(SimpleActorError::Invariant);
        }
        let id = due.event.payload.batch;
        let mut step = SimpleStep::default();
        if due.event.payload.opening {
            if self.scope_runs.contains_key(&id)
                || self.scope_runs.len() >= MAX_BINDINGS
                || !self.children.contains_key(&id)
            {
                return Err(SimpleActorError::Invariant);
            }
            let slots: Vec<_> = due
                .event
                .payload
                .slots
                .iter()
                .copied()
                .filter(|slot| !self.removed_scope_sources.contains(slot))
                .collect();
            let references = self
                .complex
                .begin_descendant_projection_group(&slots, due.delivered_at)?;
            let references = references
                .into_iter()
                .map(|id| {
                    let device = self
                        .complex
                        .descendant_projection_device(id)
                        .ok_or(SimpleActorError::Invariant)?;
                    Ok((id, device))
                })
                .collect::<Result<Vec<_>, SimpleActorError>>()?;
            self.scope_runs.insert(
                id,
                ScopeRun {
                    open: due.event,
                    references,
                },
            );
        } else {
            let run = self
                .scope_runs
                .get(&id)
                .ok_or(SimpleActorError::Invariant)?;
            if run.open.payload.slots != due.event.payload.slots
                || run.open.device != due.event.device
                || run.open.source != due.event.source
                || !Arc::ptr_eq(&run.open.config, &due.event.config)
                || due.event.at_ns < run.open.at_ns
            {
                return Err(SimpleActorError::Invariant);
            }
            let ids: Vec<_> = run.references.iter().map(|(id, _)| *id).collect();
            self.complex
                .close_descendant_projection_group(&ids, due.delivered_at)?;
            self.scope_runs.remove(&id);
            self.finish_child(id, &mut step)?;
            self.remember_scope_source_removal(&[]);
        }
        Ok(step)
    }
    pub(super) fn cleanup_scope_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, SimpleActorError> {
        let mut outputs = Vec::new();
        // Complex has already discarded the removed source device's templates.
        // Stale references are pruned only in this trusted lifecycle path.
        for run in self.scope_runs.values_mut() {
            for &(id, source_device) in &run.references {
                match self.complex.descendant_projection_device(id) {
                    Some(actual) if actual == source_device => {}
                    None if source_device.id == device.id
                        && (source_device.generation < device.generation
                            || (removed && source_device.generation == device.generation)) => {}
                    _ => return Err(SimpleActorError::Invariant),
                }
            }
            run.references
                .retain(|(id, _)| self.complex.descendant_projection_device(*id).is_some());
        }
        let ids: Vec<_> = self
            .scope_runs
            .iter()
            .filter(|(_, run)| {
                run.open.device.id == device.id
                    && (run.open.device.generation < device.generation
                        || (removed && run.open.device.generation == device.generation))
            })
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let run = self
                .scope_runs
                .get(&id)
                .ok_or(SimpleActorError::Invariant)?;
            for &(reference, _) in &run.references {
                let restored = self
                    .complex
                    .restore_descendant_projection_control(reference, at)?;
                outputs.extend(restored.outputs);
            }
            self.scope_runs.remove(&id);
        }
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn key(name: &str) -> KeyId {
        name.parse().unwrap()
    }
    fn device(id: u64) -> DeviceInstanceId {
        DeviceInstanceId { id, generation: 1 }
    }
    fn fixture() -> SimpleStageActor {
        let mut extras = MappingExtras::default();
        extras.from_modifiers.mandatory.insert(Modifier::LeftShift);
        let cfg = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "scope fault".into(),
            simple: vec![Mapping {
                id: "a".into(),
                from: key("a"),
                to: vec![key("right_control").into()],
                allow_extra_modifiers: true,
                extras,
            }],
            complex: vec![Mapping {
                id: "shift".into(),
                from: key("left_shift"),
                to: vec![key("x").into()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            }],
        });
        let mut actor = SimpleStageActor::new(cfg).unwrap();
        actor.observe_device(device(1), false, MonoTime(0)).unwrap();
        actor.observe_device(device(2), false, MonoTime(0)).unwrap();
        admit(&mut actor, "left_shift", 1, 100);
        while actor.poll(MonoTime(100)).unwrap().is_some() {}
        actor
    }
    fn admit(actor: &mut SimpleStageActor, name: &str, dev: u64, at: u64) {
        let id = actor.next_batch_id().unwrap();
        let plan = action_plan::plan_actions(
            &[key(name).into()],
            action_plan::PlanOrigin {
                batch: id,
                stage: Stage::Simple,
                start: MonoTime(at),
                device: device(dev),
                source: SourceToken(id.0),
                config: actor.snapshot().clone(),
            },
            false,
        )
        .unwrap();
        let event = plan.events()[0].clone();
        actor.register_input(plan).unwrap();
        actor
            .admit(
                &DueEvent {
                    event,
                    delivered_at: MonoTime(at),
                },
                &ContextSnapshot::default(),
            )
            .unwrap();
    }
    #[test]
    fn missing_reference_on_unrelated_device_control_is_failure_and_reset_clears_record() {
        let mut actor = fixture();
        admit(&mut actor, "a", 2, 110);
        while actor.poll(MonoTime(110)).unwrap().is_some() {}
        let reference = actor.scope_runs.values().next().unwrap().references[0].0;
        // Deliberate private-state corruption, with no platform IO.
        actor
            .complex
            .restore_descendant_projection_control(reference, MonoTime(120))
            .unwrap();
        assert_eq!(
            actor
                .observe_device(device(2), false, MonoTime(120))
                .unwrap_err(),
            SimpleActorError::Invariant
        );
        assert!(actor.is_failed());
        assert_eq!(actor.scope_runs.len(), 1);
        actor.reset(MonoTime(120)).unwrap();
        assert!(actor.scope_runs.is_empty());
        assert!(!actor.is_failed());
    }
    #[test]
    fn exhausted_template_reference_budget_fails_scope_dispatch_without_fallback_and_reset_recovers()
     {
        let mut actor = fixture();
        let child = *actor.children.keys().next().unwrap();
        let source_slot = ActionSlot {
            batch: child,
            index: 0,
        };
        assert!(actor.complex.producer(source_slot).is_some());
        actor
            .complex
            .begin_descendant_projection_group(&vec![source_slot; MAX_BINDINGS], MonoTime(100))
            .unwrap();
        // Deliver the synthetic group's real visibility marker before testing admission.
        while actor.poll(MonoTime(100)).unwrap().is_some() {}
        admit(&mut actor, "a", 2, 110);
        assert!(matches!(
            actor.poll(MonoTime(110)),
            Err(SimpleActorError::Complex(ComplexActorError::Binding(
                BindingError::Capacity
            )))
        ));
        assert!(actor.is_failed());
        assert!(actor.scope_runs.is_empty());
        assert!(matches!(
            actor.poll(MonoTime(110)),
            Err(SimpleActorError::Failed)
        ));
        let reset = actor.reset(MonoTime(110)).unwrap();
        assert!(reset.outputs.is_empty());
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.pending_children(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
}
