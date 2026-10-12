use super::*;
use crate::terminal_actor::KeyProjectionId;
pub(super) struct ProjectionRecipe {
    pub(super) batch: BatchId,
    pub(super) device: DeviceInstanceId,
}
#[must_use = "commit or abort both projection preparations"]
pub struct PreparedOutputKeyProjection<'a> {
    terminal: terminal_actor::PreparedKeyProjection<'a>,
    generated: generated_stage::PreparedGeneratedKeyProjection<'a>,
    recipes: &'a mut BTreeMap<KeyProjectionId, ProjectionRecipe>,
    id: KeyProjectionId,
    batch: BatchId,
    device: DeviceInstanceId,
    now: &'a mut MonoTime,
    at: MonoTime,
}
impl PreparedOutputKeyProjection<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) -> KeyProjectionId {
        self.terminal.commit();
        self.generated.commit();
        self.recipes.insert(
            self.id,
            ProjectionRecipe {
                batch: self.batch,
                device: self.device,
            },
        );
        *self.now = self.at;
        self.id
    }
}
#[must_use = "commit or abort both projection closes"]
pub struct PreparedOutputKeyProjectionClose<'a> {
    terminal: terminal_actor::PreparedKeyProjectionClose<'a>,
    generated: generated_stage::PreparedGeneratedKeyProjectionClose<'a>,
    now: &'a mut MonoTime,
    at: MonoTime,
}
impl PreparedOutputKeyProjectionClose<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.terminal.commit();
        self.generated.commit();
        *self.now = self.at;
    }
}
impl OutputStageActor {
    pub fn prepare_key_projection(
        &mut self,
        batch: BatchId,
        at: MonoTime,
    ) -> Result<Option<PreparedOutputKeyProjection<'_>>, OutputStageError> {
        self.running()?;
        self.check_time(at)?;
        let recipe = self
            .recipes
            .get(&batch)
            .ok_or(terminal_actor::ActorError::InvalidProjection)?;
        let Some(terminal) = self.terminal.prepare_key_projection(batch, at)? else {
            return Ok(None);
        };
        let id = terminal.id();
        let generated =
            self.generated
                .prepare_key_projection(&recipe.saved, terminal.slots(), id, at)?;
        Ok(Some(PreparedOutputKeyProjection {
            terminal,
            generated,
            recipes: &mut self.projections,
            id,
            batch,
            device: recipe.device,
            now: &mut self.now,
            at,
        }))
    }
    pub fn begin_key_projection(
        &mut self,
        batch: BatchId,
        at: MonoTime,
    ) -> Result<Option<KeyProjectionId>, OutputStageError> {
        Ok(self
            .prepare_key_projection(batch, at)?
            .map(PreparedOutputKeyProjection::commit))
    }
    pub fn prepare_key_projection_close(
        &mut self,
        id: KeyProjectionId,
        at: MonoTime,
    ) -> Result<PreparedOutputKeyProjectionClose<'_>, OutputStageError> {
        self.running()?;
        self.check_time(at)?;
        if !self.projections.contains_key(&id) {
            return Err(terminal_actor::ActorError::InvalidProjection.into());
        }
        let terminal = self.terminal.prepare_key_projection_close(id, at)?;
        let generated = self.generated.prepare_key_projection_close(id, at)?;
        Ok(PreparedOutputKeyProjectionClose {
            terminal,
            generated,
            now: &mut self.now,
            at,
        })
    }
    pub fn close_key_projection(
        &mut self,
        id: KeyProjectionId,
        at: MonoTime,
    ) -> Result<(), OutputStageError> {
        self.prepare_key_projection_close(id, at)?.commit();
        Ok(())
    }
}

#[must_use = "commit or abort projected batch admission on both sides"]
pub struct PreparedProjectedOutputBatch<'a> {
    terminal: terminal_actor::PreparedTerminalProjectedAppend<'a>,
    generated: generated_stage::PreparedScopedGeneration<'a>,
    recipes: &'a mut BTreeMap<BatchId, Recipe>,
    projections: &'a mut BTreeMap<KeyProjectionId, ProjectionRecipe>,
    id: KeyProjectionId,
    batch: BatchId,
    device: DeviceInstanceId,
    parent: Option<action_plan::ActionSlot>,
}
impl PreparedProjectedOutputBatch<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) -> KeyProjectionId {
        self.terminal.commit();
        let saved = self.generated.commit();
        self.recipes.insert(
            self.batch,
            Recipe {
                saved,
                device: self.device,
                parent: self.parent,
            },
        );
        self.projections.insert(
            self.id,
            ProjectionRecipe {
                batch: self.batch,
                device: self.device,
            },
        );
        self.id
    }
}
impl OutputStageActor {
    /// New batch keys/variables become projected before their first real action.
    /// No ordinary Down or variable Down value is briefly exposed during projection.
    pub fn prepare_projected_scoped(
        &mut self,
        plan: ScopedActionPlan,
    ) -> Result<PreparedProjectedOutputBatch<'_>, OutputStageError> {
        self.prepare_projected_inner(plan, None)
    }
    fn prepare_projected_inner(
        &mut self,
        plan: ScopedActionPlan,
        child: Option<&stage_bindings::ChildAdmission<'_>>,
    ) -> Result<PreparedProjectedOutputBatch<'_>, OutputStageError> {
        self.running()?;
        let batch = plan.actions().origin.batch;
        let device = plan.actions().origin.device;
        let generated = match child {
            Some(child) => self.generated.prepare_child_scoped(&plan, child)?,
            None => self.generated.prepare_scoped(&plan)?,
        };
        let terminal = match child {
            Some(child) => self.terminal.prepare_child_scoped(plan, child)?,
            None => self.terminal.prepare_scoped(plan)?,
        }
        .projected()?;
        let id = terminal.id();
        let generated = generated.projected(id)?;
        Ok(PreparedProjectedOutputBatch {
            terminal,
            generated,
            recipes: &mut self.recipes,
            projections: &mut self.projections,
            id,
            batch,
            device,
            parent: child.map(stage_bindings::ChildAdmission::parent),
        })
    }
    pub fn prepare_projected_child_scoped<'a, T>(
        &'a mut self,
        plan: ScopedActionPlan,
        registration: stage_bindings::Reservation<'a, T>,
    ) -> Result<PreparedProjectedOutputChild<'a, T>, OutputStageError> {
        self.running()?;
        let registration = registration.with_extra_sources(plan.scope().reservation())?;
        let child = registration.child_admission()?;
        let output = self.prepare_projected_inner(plan, Some(&child))?;
        Ok(PreparedProjectedOutputChild {
            output,
            registration,
        })
    }
}

#[must_use = "commit or abort parent registration and born-projected output together"]
pub struct PreparedProjectedOutputChild<'a, T> {
    output: PreparedProjectedOutputBatch<'a>,
    registration: stage_bindings::Reservation<'a, T>,
}
impl<T> PreparedProjectedOutputChild<'_, T> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) -> KeyProjectionId {
        self.registration.commit();
        self.output.commit()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn actor() -> OutputStageActor {
        let mut actor =
            OutputStageActor::new(Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))).unwrap();
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
        let plan = action_plan::plan_actions(
            &["left_shift".parse::<KeyId>().unwrap().into()],
            origin(&actor, 1),
            false,
        )
        .unwrap();
        actor.enqueue(plan).unwrap();
        while actor.poll(MonoTime(0)).unwrap().is_some() {}
        actor
    }
    fn origin(actor: &OutputStageActor, id: u64) -> action_plan::PlanOrigin {
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
        }
    }
    fn bulk(actor: &OutputStageActor, id: u64) -> ActionPlan {
        action_plan::plan_actions(
            &vec![
                Action {
                    hold_down_ns: 1,
                    .."x".parse::<KeyId>().unwrap().into()
                };
                512
            ],
            origin(actor, id),
            true,
        )
        .unwrap()
    }
    #[test]
    fn generated_fifo_rejection_aborts_prepared_terminal_mask_budget_identity_and_clock() {
        let mut actor = actor();
        for id in 100..108 {
            actor
                .generated
                .enqueue(bulk(&actor, id).events().to_vec())
                .unwrap();
        }
        assert!(matches!(
            actor.begin_key_projection(BatchId(1), MonoTime(100)),
            Err(OutputStageError::Generated(GeneratedError::Timeline(
                timeline::TimelineError::Capacity
            )))
        ));
        assert_eq!(actor.terminal.suppression_references(), 0);
        assert_eq!(actor.terminal.queued_events(), 0);
        assert!(actor.projections.is_empty());
        assert!(
            actor
                .generated_modifiers()
                .contains(&"left_shift".parse().unwrap())
        );
        actor.generated.reset(MonoTime(0)).unwrap(); // isolated fixture only; no polling misaligned actor
        assert!(
            actor
                .terminal
                .begin_key_projection(BatchId(1), MonoTime(0))
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn terminal_fifo_rejection_leaves_generation_modifier_visible_and_no_logical_reference() {
        let mut actor = actor();
        for id in 100..108 {
            actor.terminal.enqueue(bulk(&actor, id)).unwrap();
        }
        assert!(matches!(
            actor.begin_key_projection(BatchId(1), MonoTime(100)),
            Err(OutputStageError::Terminal(ActorError::Timeline(
                timeline::TimelineError::Capacity
            )))
        ));
        assert_eq!(actor.generated.queued_events(), 0);
        assert_eq!(actor.generated.hidden_modifier_references(), 0);
        actor.generated.advance(MonoTime(0)).unwrap();
        assert!(actor.projections.is_empty());
        assert!(
            actor
                .generated_modifiers()
                .contains(&"left_shift".parse().unwrap())
        );
    }
    #[test]
    fn missing_projection_recipe_latches_failed_after_receipt_without_retargering_marker() {
        let mut actor = actor();
        actor
            .begin_key_projection(BatchId(1), MonoTime(10))
            .unwrap()
            .unwrap();
        actor.projections.clear();
        assert!(matches!(
            actor.poll(MonoTime(10)),
            Err(OutputStageError::Invariant)
        ));
        assert!(actor.is_failed());
        assert!(matches!(
            actor.poll(MonoTime(10)),
            Err(OutputStageError::Failed)
        ));
        actor.reset(MonoTime(10)).unwrap();
        assert!(!actor.is_failed());
        assert_eq!(actor.next_deadline(), None);
    }
}

#[cfg(test)]
mod close_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn second_side_full_rejection_keeps_original_projection_close_retryable() {
        let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        let mut actor = OutputStageActor::new(config.clone()).unwrap();
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let origin = |id| action_plan::PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(0),
            device,
            source: SourceToken(id),
            config: config.clone(),
        };
        actor
            .enqueue(
                action_plan::plan_actions(
                    &["left_shift".parse::<KeyId>().unwrap().into()],
                    origin(1),
                    false,
                )
                .unwrap(),
            )
            .unwrap();
        while actor.poll(MonoTime(0)).unwrap().is_some() {}
        let id = actor
            .begin_key_projection(BatchId(1), MonoTime(0))
            .unwrap()
            .unwrap();
        while actor.poll(MonoTime(0)).unwrap().is_some() {}
        let actions = vec![
            Action {
                hold_down_ns: 1,
                .."x".parse::<KeyId>().unwrap().into()
            };
            512
        ];
        for batch in 100..108 {
            actor
                .generated
                .enqueue(
                    action_plan::plan_actions(&actions, origin(batch), true)
                        .unwrap()
                        .events()
                        .to_vec(),
                )
                .unwrap();
        }
        assert!(matches!(
            actor.close_key_projection(id, MonoTime(100)),
            Err(OutputStageError::Generated(GeneratedError::Timeline(
                timeline::TimelineError::Capacity
            )))
        ));
        assert_eq!(actor.terminal.queued_events(), 0);
        assert_eq!(actor.terminal.suppression_references(), 1);
        assert!(
            !actor
                .generated_modifiers()
                .contains(&"left_shift".parse().unwrap())
        );
        // Free only the synthetic fixture batches, preserving paired target/projection state.
        for batch in 100..108 {
            actor.generated.cancel_batch(BatchId(batch));
        }
        actor.close_key_projection(id, MonoTime(0)).unwrap();
        assert!(
            actor
                .generated_modifiers()
                .contains(&"left_shift".parse().unwrap())
        );
        while actor.poll(MonoTime(0)).unwrap().is_some() {}
        assert_eq!(actor.terminal.suppression_references(), 0);
        assert!(actor.projections.is_empty());
    }
}

#[cfg(test)]
mod variable_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn generated_full_rejects_without_pausing_variable_or_committing_actual_projection() {
        let action = Action {
            target: ActionTarget::SetVariable(VariableAction {
                name: "v".into(),
                operation: VariableOperation::Set {
                    value: Some(VariableValue::Integer(1)),
                    key_up_value: Some(VariableValue::Integer(2)),
                },
            }),
            .."vk_none".parse::<KeyId>().unwrap().into()
        };
        let config = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "variable capacity".into(),
            simple: vec![],
            complex: vec![Mapping {
                id: "a".into(),
                from: "a".parse().unwrap(),
                to: vec![action.clone()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            }],
        });
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        let mut actor = OutputStageActor::new(config.clone()).unwrap();
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let origin = |id| action_plan::PlanOrigin {
            batch: BatchId(id),
            stage: Stage::Output,
            start: MonoTime(0),
            device,
            source: SourceToken(id),
            config: config.clone(),
        };
        actor
            .enqueue(action_plan::plan_actions(&[action], origin(1), false).unwrap())
            .unwrap();
        while actor.poll(MonoTime(0)).unwrap().is_some() {}
        let actions = vec![
            Action {
                hold_down_ns: 1,
                .."x".parse::<KeyId>().unwrap().into()
            };
            512
        ];
        for id in 100..108 {
            actor
                .generated
                .enqueue(
                    action_plan::plan_actions(&actions, origin(id), true)
                        .unwrap()
                        .events()
                        .to_vec(),
                )
                .unwrap();
        }
        assert!(matches!(
            actor.begin_key_projection(BatchId(1), MonoTime(100)),
            Err(OutputStageError::Generated(GeneratedError::Timeline(
                timeline::TimelineError::Capacity
            )))
        ));
        assert_eq!(
            actor.generated_variables().get("v"),
            Some(&VariableValue::Integer(1))
        );
        assert_eq!(
            actor.executed_variables().get("v"),
            Some(&VariableValue::Integer(1))
        );
        assert_eq!(actor.terminal.suppression_references(), 0);
        assert_eq!(actor.terminal.queued_events(), 0);
        assert!(actor.projections.is_empty());
    }
}
