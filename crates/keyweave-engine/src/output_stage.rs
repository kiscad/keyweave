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
}
struct Recipe {
    saved: ScopedGeneration,
    device: DeviceInstanceId,
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
        self.enqueue_scoped(ScopedActionPlan::plain(plan))
    }
    pub fn enqueue_scoped(&mut self, plan: ScopedActionPlan) -> Result<(), OutputStageError> {
        self.running()?;
        let origin = &plan.actions().origin;
        let id = origin.batch;
        let device = origin.device;
        let nonempty = !plan.actions().events().is_empty();
        let generated = self.generated.prepare_scoped(&plan)?;
        let terminal = self.terminal.prepare_scoped(plan)?;
        // Both preparations succeeded; neither committed state on a rejection.
        terminal.commit();
        let saved = generated.commit();
        if nonempty {
            self.recipes.insert(id, Recipe { saved, device });
        }
        Ok(())
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
        self.running()?;
        self.check_time(at)?;
        let actual = self.terminal.poll_delivered(at);
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
        if let Some(receipt) = actual.action {
            let generated = match self.generated.poll(at) {
                Ok(Some(delivery)) => delivery.event,
                Ok(None) => return self.fault(OutputStageError::Invariant),
                Err(error) => return self.fault(error.into()),
            };
            if !same_delivery(&generated, &receipt) {
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
        Ok(Some(actual.step))
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
fn same_delivery(a: &StageEvent<PlannedAction>, b: &StageEvent<PlannedAction>) -> bool {
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
