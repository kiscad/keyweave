use super::*;
use crate::terminal_actor::KeyProjectionId;
/// Actor-local template reference, not native authority or an owner identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ComplexProjectionId(u64);
pub(super) struct DescendantProjection {
    pub(super) device: DeviceInstanceId,
    pub(super) outputs: Vec<KeyProjectionId>,
    references: usize,
}
impl ComplexStageActor {
    /// Lifecycle cancellation bypasses FIFO admission while preserving original
    /// child work and other template references. Retain every returned intent.
    pub fn restore_descendant_projection_control(
        &mut self,
        id: ComplexProjectionId,
        at: MonoTime,
    ) -> Result<ComplexStep, ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        let slot = *self
            .projection_handles
            .get(&id)
            .ok_or(ComplexActorError::Input)?;
        let group = self
            .projection_groups
            .get_mut(&slot)
            .ok_or(ComplexActorError::Invariant)?;
        let mut step = ComplexStep::default();
        if group.references == 1 {
            for &projection in &group.outputs {
                match self.output.restore_key_projection_control(projection, at) {
                    Ok(outputs) => step.outputs.extend(outputs),
                    Err(error) => {
                        self.failed = true;
                        self.now = at;
                        return Err(error.into());
                    }
                }
            }
            self.projection_groups.remove(&slot);
        } else {
            group.references -= 1;
        }
        self.projection_handles.remove(&id);
        self.now = at;
        Ok(step)
    }

    /// Hide this retained producer's current and future Output children.
    /// Output admission errors latch Failed; only reset may recover a partial group transaction.
    pub fn begin_descendant_projection(
        &mut self,
        slot: ActionSlot,
        at: MonoTime,
    ) -> Result<ComplexProjectionId, ComplexActorError> {
        self.projection_control_preflight(at)?;
        let device = if let Some(group) = self.projection_groups.get(&slot) {
            group.device
        } else {
            if self.producers.live(slot).is_none() {
                self.producers.check_down(slot)?;
            }
            self.inputs
                .get(&slot.batch)
                .ok_or(ComplexActorError::Input)?
                .plan
                .origin
                .device
        };
        if self.projection_handles.len() >= MAX_BINDINGS {
            return Err(BindingError::Capacity.into());
        }
        let id = ComplexProjectionId(
            self.next_projection
                .checked_add(1)
                .ok_or(ComplexActorError::Batch)?,
        );
        if let Some(group) = self.projection_groups.get_mut(&slot) {
            group.references += 1;
        } else {
            let children: Vec<_> = self
                .children
                .iter()
                .filter(|(_, parent)| **parent == slot)
                .map(|(&id, _)| id)
                .collect();
            let mut outputs = Vec::new();
            for child in children {
                match self.output.begin_key_projection(child, at) {
                    Ok(Some(projection)) => outputs.push(projection),
                    Ok(None) => {}
                    Err(error) => {
                        self.failed = true;
                        self.now = at;
                        return Err(error.into());
                    }
                }
            }
            self.projection_groups.insert(
                slot,
                DescendantProjection {
                    device,
                    outputs,
                    references: 1,
                },
            );
        }
        self.projection_handles.insert(id, slot);
        self.next_projection = id.0;
        self.now = at;
        Ok(id)
    }
    /// Close one reference; only the last restores surviving original child sources.
    pub fn close_descendant_projection(
        &mut self,
        id: ComplexProjectionId,
        at: MonoTime,
    ) -> Result<(), ComplexActorError> {
        self.projection_control_preflight(at)?;
        let slot = *self
            .projection_handles
            .get(&id)
            .ok_or(ComplexActorError::Input)?;
        let group = self
            .projection_groups
            .get_mut(&slot)
            .ok_or(ComplexActorError::Invariant)?;
        if group.references == 1 {
            for &projection in &group.outputs {
                if let Err(error) = self.output.close_key_projection(projection, at) {
                    self.failed = true;
                    self.now = at;
                    return Err(error.into());
                }
            }
            self.projection_groups.remove(&slot);
        } else {
            group.references -= 1;
        }
        self.projection_handles.remove(&id);
        self.now = at;
        Ok(())
    }
    fn projection_control_preflight(&self, at: MonoTime) -> Result<(), ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        if self.next_deadline().is_some_and(|due| due <= at) {
            return Err(ComplexActorError::DeadlinePending);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn output_admission_failure_latches_complex_and_reset_discards_partial_projection_state() {
        let cfg = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        let mut actor = ComplexStageActor::new(cfg.clone()).unwrap();
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let origin = |id, stage, at| action_plan::PlanOrigin {
            batch: BatchId(id),
            stage,
            start: MonoTime(at),
            device,
            source: SourceToken(id),
            config: cfg.clone(),
        };
        let b: KeyId = "b".parse().unwrap();
        let plan =
            action_plan::plan_actions(&[b.into()], origin(1, Stage::Complex, 100), false).unwrap();
        let event = plan.events()[0].clone();
        let slot = event.payload.slot;
        actor.register_input(plan).unwrap();
        actor
            .admit(
                &DueEvent {
                    event,
                    delivered_at: MonoTime(100),
                },
                &ContextSnapshot::default(),
            )
            .unwrap();
        while actor.poll(MonoTime(100)).unwrap().is_some() {}
        let actions = vec![
            Action {
                hold_down_ns: 1,
                ..b.into()
            };
            512
        ];
        for id in 100..108 {
            actor
                .output
                .enqueue(
                    action_plan::plan_actions(&actions, origin(id, Stage::Output, 200), true)
                        .unwrap(),
                )
                .unwrap();
        }
        assert!(matches!(
            actor.begin_descendant_projection(slot, MonoTime(110)),
            Err(ComplexActorError::Output(_))
        ));
        assert!(actor.is_failed());
        assert!(actor.projection_handles.is_empty());
        assert!(matches!(
            actor.poll(MonoTime(110)),
            Err(ComplexActorError::Failed)
        ));
        let reset = actor.reset(MonoTime(110)).unwrap();
        assert_eq!(reset.outputs.len(), 1);
        assert_eq!(
            (reset.outputs[0].key, reset.outputs[0].edge),
            (b, KeyEdge::Up)
        );
        assert!(!actor.is_failed());
        assert_eq!(actor.pending_inputs(), 0);
        assert!(actor.projection_groups.is_empty());
    }
    #[test]
    fn partial_last_close_latches_failed_and_reset_clears_both_closed_and_open_child_masks() {
        let b: KeyId = "b".parse().unwrap();
        let c: KeyId = "c".parse().unwrap();
        let a: KeyId = "a".parse().unwrap();
        let cfg = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "partial close".into(),
            simple: vec![],
            complex: vec![Mapping {
                id: "a".into(),
                from: a,
                to: vec![b.into()],
                allow_extra_modifiers: true,
                extras: MappingExtras {
                    if_alone: vec![Action {
                        hold_down_ns: 100,
                        ..c.into()
                    }],
                    ..Default::default()
                },
            }],
        });
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        let mut actor = ComplexStageActor::new(cfg.clone()).unwrap();
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let origin = |id, stage, at| action_plan::PlanOrigin {
            batch: BatchId(id),
            stage,
            start: MonoTime(at),
            device,
            source: SourceToken(id),
            config: cfg.clone(),
        };
        let plan =
            action_plan::plan_actions(&[a.into()], origin(1, Stage::Complex, 100), false).unwrap();
        let event = plan.events()[0].clone();
        let slot = event.payload.slot;
        actor.register_input(plan).unwrap();
        actor
            .admit(
                &DueEvent {
                    event,
                    delivered_at: MonoTime(100),
                },
                &ContextSnapshot::default(),
            )
            .unwrap();
        while actor.poll(MonoTime(100)).unwrap().is_some() {}
        actor
            .branch(
                slot,
                rule_selection::RuleBranch::Alone,
                &ContextSnapshot::default(),
                MonoTime(110),
            )
            .unwrap();
        while actor.poll(MonoTime(110)).unwrap().is_some() {}
        let projection = actor
            .begin_descendant_projection(slot, MonoTime(111))
            .unwrap();
        assert_eq!(actor.projection_groups[&slot].outputs.len(), 2);
        while actor.poll(MonoTime(210)).unwrap().is_some() {}
        let action = Action {
            hold_down_ns: 1,
            ..c.into()
        };
        for id in 100..107 {
            actor
                .output
                .enqueue(
                    action_plan::plan_actions(
                        &vec![action.clone(); 512],
                        origin(id, Stage::Output, 300),
                        true,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        actor
            .output
            .enqueue(
                action_plan::plan_actions(
                    &vec![action; 511],
                    origin(107, Stage::Output, 300),
                    true,
                )
                .unwrap(),
            )
            .unwrap();
        actor
            .output
            .enqueue(
                action_plan::plan_actions(&[c.into()], origin(108, Stage::Output, 300), false)
                    .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            actor.close_descendant_projection(projection, MonoTime(220)),
            Err(ComplexActorError::Output(_))
        ));
        assert!(actor.is_failed());
        assert_eq!(actor.now, MonoTime(220));
        assert_eq!(actor.projection_handles.get(&projection), Some(&slot));
        assert!(matches!(
            actor.poll(MonoTime(220)),
            Err(ComplexActorError::Failed)
        ));
        actor.reset(MonoTime(220)).unwrap();
        assert!(actor.projection_groups.is_empty());
        assert!(actor.projection_handles.is_empty());
        assert_eq!(actor.output.next_deadline(), None);
        assert!(!actor.is_failed());
    }
}
