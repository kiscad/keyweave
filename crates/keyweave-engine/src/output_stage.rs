//! Owned output-stage generation, actual binding execution and coordinated retirement.
//! Simple/complex input actors and native output acceptance remain separate.
use crate::{
    action_plan::{ActionPlan, BatchId, PlannedAction},
    generated_stage::{GeneratedError, GeneratedStageQueue, ScopedGeneration},
    modifier_scope::ScopedActionPlan,
    rule_selection::RuleSelection,
    terminal_actor::{ActorError, ActorStep, TerminalActor},
    timeline::{Stage, StageEvent},
    *,
};

#[derive(Debug, PartialEq, Eq, Error)]
pub enum OutputStageError {
    #[error(transparent)]
    Generated(#[from] GeneratedError),
    #[error(transparent)]
    Terminal(#[from] ActorError),
    #[error("output stage clock moved backwards")]
    TimeReversed,
    #[error("output stage generation/execution invariant failed; reset is required")]
    Invariant,
    #[error("output stage failed; reset is required before ordinary work")]
    Failed,
    #[error(transparent)]
    Binding(#[from] stage_bindings::BindingError),
}
pub(crate) struct OutputDelivery {
    pub(crate) step: ActorStep,
    pub(crate) batch: Option<BatchId>,
    pub(crate) pointer: Option<timeline::DueEvent<crate::pointer_work::PointerMotion>>,
}
struct Recipe {
    saved: ScopedGeneration,
    device: DeviceInstanceId,
    parent: Option<action_plan::ActionSlot>,
}
/// A single actor owns both queues. No mutable component or original-Up credential escapes.
/// Batch IDs remain caller allocated and strictly increase, including across reset.
pub struct OutputStageActor {
    generated: GeneratedStageQueue,
    terminal: TerminalActor,
    recipes: BTreeMap<BatchId, Recipe>,
    now: MonoTime,
    failed: bool,
}
/// Both output preparations are held until commit. Drop leaves both untouched.
#[must_use = "commit or discard the prepared output batch"]
pub struct PreparedOutputAppend<'a> {
    generated: generated_stage::PreparedScopedGeneration<'a>,
    terminal: terminal_actor::PreparedTerminalAppend<'a>,
    recipes: &'a mut BTreeMap<BatchId, Recipe>,
    id: BatchId,
    device: DeviceInstanceId,
    parent: Option<action_plan::ActionSlot>,
    nonempty: bool,
}
impl PreparedOutputAppend<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.terminal.commit();
        let saved = self.generated.commit();
        if self.nonempty {
            self.recipes.insert(
                self.id,
                Recipe {
                    saved,
                    device: self.device,
                    parent: self.parent,
                },
            );
        }
    }
}
/// Owns the exclusive upstream registration and both output preparations.
/// Parent mutation is impossible until this complete transaction is committed or discarded.
#[must_use = "commit or discard the parent registration and output batch together"]
pub struct PreparedOutputChild<'a, T> {
    output: PreparedOutputAppend<'a>,
    registration: stage_bindings::Reservation<'a, T>,
}
impl<T> PreparedOutputChild<'_, T> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.registration.commit();
        self.output.commit();
    }
}
/// Empty main consumes the output ID and activates only the saved producer parent.
#[must_use = "commit or discard the consumed producer and empty output together"]
pub struct PreparedEmptyOutputChild<'a, T> {
    output: PreparedOutputAppend<'a>,
    activation: stage_bindings::PreparedConsumedDown<'a, T>,
}
impl<T> PreparedEmptyOutputChild<'_, T> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.activation.commit();
        self.output.commit();
    }
}
impl OutputStageActor {
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, OutputStageError> {
        Ok(Self {
            generated: GeneratedStageQueue::new(Stage::Output, config.clone())?,
            terminal: TerminalActor::new(config)?,
            recipes: BTreeMap::new(),
            now: MonoTime(0),
            failed: false,
        })
    }
    fn running(&self) -> Result<(), OutputStageError> {
        if self.failed {
            Err(OutputStageError::Failed)
        } else {
            Ok(())
        }
    }
    fn check_time(&self, at: MonoTime) -> Result<(), OutputStageError> {
        if at < self.now {
            Err(OutputStageError::TimeReversed)
        } else {
            Ok(())
        }
    }
    fn fault<T>(&mut self, error: OutputStageError) -> Result<T, OutputStageError> {
        self.failed = true;
        Err(error)
    }
    pub fn is_failed(&self) -> bool {
        self.failed
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        self.generated.snapshot()
    }
    pub fn pending_batches(&self) -> usize {
        self.recipes.len()
    }
    pub fn reserved_slots(&self) -> usize {
        self.generated.reserved_slots()
    }
    pub fn live_slots(&self) -> usize {
        self.terminal.live_slots()
    }
    /// Includes private scope markers, which have no ordinary generated action.
    pub fn queued_events(&self) -> usize {
        self.terminal.queued_events()
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.terminal.next_deadline()
    }
    pub fn generated_modifiers(&self) -> BTreeSet<KeyId> {
        self.generated.modifiers()
    }
    pub fn generated_variables(&self) -> BTreeMap<String, VariableValue> {
        self.generated.variable_values()
    }
    pub fn executed_variables(&self) -> BTreeMap<String, VariableValue> {
        self.terminal.variable_values()
    }
    pub fn pressed_outputs(&self) -> Vec<KeyId> {
        self.terminal.pressed_outputs()
    }
    /// Read the original batch's executed repeat target. A backend must still
    /// prove its own desired/submitted state and use exactly one repeat source.
    pub fn physical_repeat_target(&self, batch: BatchId) -> Option<KeyId> {
        if self.failed || !self.recipes.contains_key(&batch) {
            return None;
        }
        self.terminal.physical_repeat_target(batch)
    }
    /// Uses the generation-time destination view, not delayed actual output owners.
    pub fn select(
        &self,
        key: KeyId,
        context: &ContextSnapshot,
        device: DeviceInstanceId,
        at: MonoTime,
    ) -> Result<Option<RuleSelection>, OutputStageError> {
        self.running()?;
        self.check_time(at)?;
        Ok(self.generated.select(key, context, device, at)?)
    }
    /// Bare pass-through/tap work has no modifier suppression or snapshot scope.
    pub fn enqueue(&mut self, plan: ActionPlan) -> Result<(), OutputStageError> {
        self.prepare(plan)?.commit();
        Ok(())
    }
    pub fn enqueue_scoped(&mut self, plan: ScopedActionPlan) -> Result<(), OutputStageError> {
        self.prepare_scoped(plan)?.commit();
        Ok(())
    }
    pub fn prepare(
        &mut self,
        plan: ActionPlan,
    ) -> Result<PreparedOutputAppend<'_>, OutputStageError> {
        self.prepare_scoped(ScopedActionPlan::plain(plan))
    }
    pub fn prepare_scoped(
        &mut self,
        plan: ScopedActionPlan,
    ) -> Result<PreparedOutputAppend<'_>, OutputStageError> {
        self.prepare_inner(plan, None)
    }
    /// Consume the upstream child reservation so abort cannot orphan a committed output batch.
    pub fn prepare_child<'a, T>(
        &'a mut self,
        plan: ActionPlan,
        registration: stage_bindings::Reservation<'a, T>,
    ) -> Result<PreparedOutputChild<'a, T>, OutputStageError> {
        self.prepare_child_scoped(ScopedActionPlan::plain(plan), registration)
    }
    pub(crate) fn condition_truth(
        &self,
        condition: &Condition,
        context: &ContextSnapshot,
        device: DeviceInstanceId,
        at: MonoTime,
    ) -> Truth {
        self.generated
            .condition_truth(condition, context, device, at)
    }
    /// Save the delivered Complex producer's original decision before binding admission.
    pub fn decide_producer_down(
        &self,
        down: &timeline::DueEvent<PlannedAction>,
        context: &ContextSnapshot,
    ) -> Result<producer_decision::ProducerDecision, OutputStageError> {
        self.running()?;
        Ok(self.generated.decide_producer_down(down, context)?)
    }
    pub fn prepare_child_scoped<'a, T>(
        &'a mut self,
        plan: ScopedActionPlan,
        registration: stage_bindings::Reservation<'a, T>,
    ) -> Result<PreparedOutputChild<'a, T>, OutputStageError> {
        self.running()?;
        let extra = if plan.actions().events().is_empty() {
            0
        } else {
            plan.scope().reservation()
        };
        let registration = registration.with_extra_sources(extra)?;
        let child = registration.child_admission()?;
        let output = self.prepare_inner(plan, Some(&child))?;
        Ok(PreparedOutputChild {
            output,
            registration,
        })
    }
    /// Empty main must not allocate a fictitious child or lose the original producer binding.
    pub fn prepare_empty_child_scoped<'a, T>(
        &'a mut self,
        plan: ScopedActionPlan,
        activation: stage_bindings::PreparedConsumedDown<'a, T>,
    ) -> Result<PreparedEmptyOutputChild<'a, T>, OutputStageError> {
        self.running()?;
        if !plan.actions().events().is_empty() || !plan.actions().releases().is_empty() {
            return Err(GeneratedError::Lifecycle.into());
        }
        let admission = activation.admission();
        let output = self.prepare_inner(plan, Some(&admission))?;
        Ok(PreparedEmptyOutputChild { output, activation })
    }
    fn prepare_inner(
        &mut self,
        plan: ScopedActionPlan,
        child: Option<&stage_bindings::ChildAdmission<'_>>,
    ) -> Result<PreparedOutputAppend<'_>, OutputStageError> {
        self.running()?;
        let origin = &plan.actions().origin;
        let id = origin.batch;
        let device = origin.device;
        let nonempty = !plan.actions().events().is_empty();
        let generated = match child {
            Some(child) => self.generated.prepare_child_scoped(&plan, child)?,
            None => self.generated.prepare_scoped(&plan)?,
        };
        let terminal = match child {
            Some(child) => self.terminal.prepare_child_scoped(plan, child)?,
            None => self.terminal.prepare_scoped(plan)?,
        };
        Ok(PreparedOutputAppend {
            generated,
            terminal,
            recipes: &mut self.recipes,
            id,
            device,
            parent: child.map(stage_bindings::ChildAdmission::parent),
            nonempty,
        })
    }
    pub fn parent_binding(&self, id: BatchId) -> Option<action_plan::ActionSlot> {
        self.recipes.get(&id)?.parent
    }
    /// Caller drains due work first. Both recipes commit or neither does.
    pub fn input_up(&mut self, id: BatchId, at: MonoTime) -> Result<bool, OutputStageError> {
        self.running()?;
        self.check_time(at)?;
        let terminal = self.terminal.prepare_input_up(id, at)?;
        let Some(recipe) = self.recipes.get_mut(&id) else {
            if terminal.is_some() {
                return self.fault(OutputStageError::Invariant);
            }
            return Ok(false);
        };
        let generated = self
            .generated
            .prepare_input_up_scoped(&mut recipe.saved, at)?;
        match (terminal, generated) {
            (Some(terminal), Some(generated)) => {
                terminal.commit();
                generated.commit();
                self.now = at;
                Ok(true)
            }
            (None, None) => Ok(false),
            _ => {
                self.failed = true;
                Err(OutputStageError::Invariant)
            }
        }
    }
    /// Deliver exactly one actual event; markers never consume another batch's action.
    /// A completion releases generation reservations only after actual bindings/scopes finish.
    pub fn poll(&mut self, at: MonoTime) -> Result<Option<ActorStep>, OutputStageError> {
        Ok(self.poll_delivered(at)?.map(|delivery| delivery.step))
    }
    pub(crate) fn poll_delivered(
        &mut self,
        at: MonoTime,
    ) -> Result<Option<OutputDelivery>, OutputStageError> {
        self.poll_delivered_with_eof(at, |_| true)
    }
    pub(crate) fn poll_delivered_with_eof(
        &mut self,
        at: MonoTime,
        allow_sync: impl Fn(DeviceInstanceId) -> bool,
    ) -> Result<Option<OutputDelivery>, OutputStageError> {
        self.running()?;
        self.check_time(at)?;
        let actual = self.terminal.poll_delivered_with_eof(at, allow_sync);
        self.now = at;
        let actual = match actual {
            Ok(actual) => actual,
            Err(error) => return self.fault(error.into()),
        };
        let Some(actual) = actual else {
            if self.generated.next_deadline().is_some_and(|due| due <= at) {
                return self.fault(OutputStageError::Invariant);
            }
            if let Err(error) = self.generated.advance(at) {
                return self.fault(error.into());
            }
            return Ok(None);
        };
        let batch = actual
            .action
            .as_ref()
            .map(|receipt| receipt.payload.slot.batch);
        if let Some(receipt) = actual.action {
            let generated = match self.generated.poll(at) {
                Ok(Some(delivery)) => delivery.event,
                Ok(None) => return self.fault(OutputStageError::Invariant),
                Err(error) => return self.fault(error.into()),
            };
            if !same_delivery(&generated, &receipt) {
                return self.fault(OutputStageError::Invariant);
            }
        } else if let Some(pointer) = &actual.pointer {
            let generated = match self.generated.poll_mixed(at) {
                Ok(Some(generated_stage::GeneratedDelivery::Pointer(due))) => due,
                Ok(_) => return self.fault(OutputStageError::Invariant),
                Err(error) => return self.fault(error.into()),
            };
            if !same_delivery(&generated.event, &pointer.event) {
                return self.fault(OutputStageError::Invariant);
            }
        } else if let Err(error) = self.generated.advance(at) {
            return self.fault(error.into());
        }
        if let Some(id) = actual.step.completed_batch {
            if let Err(error) = self.generated.retire_batch(id) {
                return self.fault(error.into());
            }
            if self.recipes.remove(&id).is_none() {
                return self.fault(OutputStageError::Invariant);
            }
        }
        Ok(Some(OutputDelivery {
            step: actual.step,
            batch,
            pointer: actual.pointer,
        }))
    }
    pub(crate) fn enqueue_pointer(
        &mut self,
        event: StageEvent<crate::pointer_work::PointerMotion>,
    ) -> Result<(), OutputStageError> {
        self.running()?;
        self.check_time(event.at_ns)?;
        let at = event.at_ns;
        let generated = self.generated.prepare_pointer(event.clone())?;
        let terminal = self.terminal.prepare_pointer(event)?;
        generated.commit();
        terminal.commit();
        self.now = at;
        Ok(())
    }
    pub(crate) fn synchronize_if_device_idle(
        &mut self,
        device: DeviceInstanceId,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, OutputStageError> {
        self.running()?;
        self.check_time(at)?;
        if self.recipes.values().any(|recipe| recipe.device == device) {
            return Ok(Vec::new());
        }
        let outputs = match self.terminal.synchronize_modifiers(at) {
            Ok(outputs) => outputs,
            Err(error) => return self.fault(error.into()),
        };
        if let Err(error) = self.generated.advance(at) {
            return self.fault(error.into());
        }
        self.now = at;
        Ok(outputs)
    }
    pub(crate) fn check_live_device(
        &self,
        device: DeviceInstanceId,
    ) -> Result<(), OutputStageError> {
        self.generated.check_live_device(device)?;
        self.terminal.check_live_device(device)?;
        Ok(())
    }
    /// Both device histories preflight before controls; cleanup uses actual saved bindings.
    pub fn observe_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, OutputStageError> {
        self.check_time(at)?;
        self.generated.check_device(device, removed)?;
        self.terminal.check_device(device, removed, at)?;
        let outputs = match self.terminal.observe_device(device, removed, at) {
            Ok(outputs) => outputs,
            Err(error) => {
                self.now = at;
                return self.fault(error.into());
            }
        };
        if let Err(error) = self.generated.observe_device(device, removed) {
            self.now = at;
            return self.fault(error.into());
        }
        if let Err(error) = self.generated.advance(at) {
            self.now = at;
            return self.fault(error.into());
        }
        self.recipes.retain(|_, recipe| {
            !(recipe.device.id == device.id
                && (recipe.device.generation < device.generation
                    || (removed && recipe.device.generation == device.generation)))
        });
        self.now = at;
        Ok(outputs)
    }
    pub fn cancel_batch(
        &mut self,
        id: BatchId,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, OutputStageError> {
        self.check_time(at)?;
        let outputs = match self.terminal.cancel_batch(id, at) {
            Ok(outputs) => outputs,
            Err(error) => {
                self.now = at;
                return self.fault(error.into());
            }
        };
        self.generated.cancel_batch(id);
        if let Err(error) = self.generated.advance(at) {
            self.now = at;
            return self.fault(error.into());
        }
        self.recipes.remove(&id);
        self.now = at;
        Ok(outputs)
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), OutputStageError> {
        self.running()?;
        let generated_names = self.generated.check_activation(&config)?;
        let terminal_names = self.terminal.check_activation(&config)?;
        self.generated
            .commit_activation(config.clone(), generated_names);
        self.terminal.commit_activation(config, terminal_names);
        Ok(())
    }
    /// Clear both sides, preserving device tombstones and monotonic IDs. Returned
    /// emitted-key ups are engine recovery intents, not confirmed native acceptance.
    pub fn reset(&mut self, at: MonoTime) -> Result<Vec<OutputIntent>, OutputStageError> {
        self.check_time(at)?;
        let outputs = match self.terminal.reset(at) {
            Ok(outputs) => outputs,
            Err(error) => {
                self.now = at;
                return self.fault(error.into());
            }
        };
        if let Err(error) = self.generated.reset(at) {
            self.now = at;
            return self.fault(error.into());
        }
        self.recipes.clear();
        self.now = at;
        self.failed = false;
        Ok(outputs)
    }
}
fn same_delivery<T: PartialEq>(a: &StageEvent<T>, b: &StageEvent<T>) -> bool {
    a.stage == b.stage
        && a.at_ns == b.at_ns
        && a.device == b.device
        && a.source == b.source
        && Arc::ptr_eq(&a.config, &b.config)
        && a.payload == b.payload
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::action_plan::{PlanOrigin, plan_actions};
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
        actor
    }
    fn plan(actor: &OutputStageActor, id: u64, at: u64, usage: u16) -> ActionPlan {
        plan_actions(
            &[KeyId::new(7, usage).into()],
            PlanOrigin {
                batch: BatchId(id),
                stage: Stage::Output,
                start: MonoTime(at),
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                source: SourceToken(100),
                config: actor.snapshot().clone(),
            },
            false,
        )
        .unwrap()
    }
    #[test]
    fn missing_generated_receipt_latches_execution_failure_and_reset_cleans_actual_owner() {
        let mut actor = actor();
        actor.enqueue(plan(&actor, 1, 0, 4)).unwrap();
        // Private synthetic corruption: public consumers cannot access either queue.
        actor.generated.cancel_batch(BatchId(1));
        assert!(matches!(
            actor.poll(MonoTime(0)),
            Err(OutputStageError::Invariant)
        ));
        assert!(actor.is_failed());
        assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
        assert_eq!(
            actor.poll(MonoTime(1)).unwrap_err(),
            OutputStageError::Failed
        );
        assert_eq!(
            actor.input_up(BatchId(1), MonoTime(1)),
            Err(OutputStageError::Failed)
        );
        assert_eq!(
            actor.enqueue(plan(&actor, 2, 1, 5)),
            Err(OutputStageError::Failed)
        );
        assert!(matches!(
            actor.select(
                KeyId::new(7, 4),
                &ContextSnapshot::default(),
                DeviceInstanceId {
                    id: 1,
                    generation: 1
                },
                MonoTime(1)
            ),
            Err(OutputStageError::Failed)
        ));
        assert_eq!(
            actor.activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)))),
            Err(OutputStageError::Failed)
        );
        let outputs = actor.reset(MonoTime(1)).unwrap();
        assert_eq!(
            outputs,
            [OutputIntent {
                at_ns: MonoTime(1),
                key: KeyId::new(7, 4),
                edge: KeyEdge::Up
            }]
        );
        assert!(!actor.is_failed());
        assert_eq!(actor.pending_batches(), 0);
        assert_eq!(actor.reserved_slots(), 0);
        actor.enqueue(plan(&actor, 2, 2, 5)).unwrap();
        assert!(actor.poll(MonoTime(2)).unwrap().is_some());
    }
    #[test]
    fn mismatched_original_up_states_abort_terminal_preparation_and_latch_failure() {
        let mut actor = actor();
        actor.enqueue(plan(&actor, 1, 0, 4)).unwrap();
        actor.poll(MonoTime(0)).unwrap();
        let recipe = actor.recipes.get_mut(&BatchId(1)).unwrap();
        actor
            .generated
            .input_up_scoped(&mut recipe.saved, MonoTime(1))
            .unwrap();
        assert_eq!(
            actor.input_up(BatchId(1), MonoTime(2)),
            Err(OutputStageError::Invariant)
        );
        assert!(actor.is_failed());
        assert_eq!(actor.terminal.queued_events(), 0);
        actor
            .terminal
            .prepare_input_up(BatchId(1), MonoTime(1))
            .unwrap()
            .unwrap()
            .abort();
        assert_eq!(actor.terminal.queued_events(), 0);
        assert_eq!(actor.reset(MonoTime(2)).unwrap().len(), 1);
        assert_eq!(actor.generated.queued_events(), 0);
        assert_eq!(actor.pending_batches(), 0);
    }
    #[test]
    fn second_activation_preflight_rejection_does_not_commit_first_snapshot() {
        let mut actor = actor();
        let old = actor.snapshot().clone();
        let next = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
        // Inject a differing second-side generation to exercise joint preflight.
        actor.terminal.activate(next.clone()).unwrap();
        assert_eq!(
            actor.activate(next.clone()),
            Err(OutputStageError::Terminal(ActorError::Engine(
                EngineError::InvalidGeneration
            )))
        );
        assert!(Arc::ptr_eq(actor.snapshot(), &old));
        assert!(Arc::ptr_eq(actor.terminal.snapshot(), &next));
    }
    #[test]
    fn second_device_preflight_rejection_does_not_retire_first_history() {
        let mut actor = actor();
        actor
            .terminal
            .observe_device(
                DeviceInstanceId {
                    id: 1,
                    generation: 2,
                },
                false,
                MonoTime(0),
            )
            .unwrap();
        let old = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        assert_eq!(
            actor.observe_device(old, true, MonoTime(0)),
            Err(OutputStageError::Terminal(ActorError::Timeline(
                crate::timeline::TimelineError::RetiredDevice
            )))
        );
        assert!(actor.generated.check_device(old, false).is_ok());
        assert_eq!(actor.pending_batches(), 0);
    }
}

#[cfg(test)]
mod pointer_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn terminal_capacity_rejection_aborts_prepared_generated_pointer_and_clock() {
        let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
        let mut actor = OutputStageActor::new(config.clone()).unwrap();
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let actions = vec![
            Action {
                hold_down_ns: 1,
                .."b".parse::<KeyId>().unwrap().into()
            };
            512
        ];
        for id in 100..108 {
            let plan = action_plan::plan_actions(
                &actions,
                action_plan::PlanOrigin {
                    batch: BatchId(id),
                    stage: Stage::Output,
                    start: MonoTime(0),
                    device,
                    source: SourceToken(id),
                    config: config.clone(),
                },
                true,
            )
            .unwrap();
            actor.terminal.enqueue(plan).unwrap();
        }
        let event = StageEvent {
            stage: Stage::Output,
            at_ns: MonoTime(10),
            device,
            source: SourceToken(1),
            config,
            payload: crate::pointer_work::PointerMotion::Move { x: 1, y: 2 },
        };
        assert!(matches!(
            actor.enqueue_pointer(event),
            Err(OutputStageError::Terminal(
                terminal_actor::ActorError::Timeline(timeline::TimelineError::Capacity)
            ))
        ));
        assert_eq!(actor.generated.queued_events(), 0);
        actor.generated.advance(MonoTime(0)).unwrap();
        assert_eq!(actor.now, MonoTime(0));
        assert!(!actor.is_failed());
    }
}
