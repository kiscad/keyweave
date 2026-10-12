//! Owned Complex producer execution. Held/alone/delayed and Simple controller pending.
use crate::{
    action_plan::{ActionPlan, ActionSlot, BatchId, PlanError, PlannedAction},
    output_stage::{OutputStageActor, OutputStageError},
    producer_decision::{ProducerDecision, ProducerUpError},
    stage_bindings::{BindingArena, BindingError},
    timeline::{DueEvent, Stage},
    *,
};
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ComplexActorError {
    #[error(transparent)]
    Output(#[from] OutputStageError),
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Up(#[from] ProducerUpError),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("Complex held/alone/delayed branch scheduling is not connected")]
    UnsupportedBranches,
    #[error("invalid registered Complex input receipt")]
    Input,
    #[error("batch identity must increase and must not overflow")]
    Batch,
    #[error("Complex actor clock moved backwards")]
    TimeReversed,
    #[error("poll due output before admitting input")]
    DeadlinePending,
    #[error("Complex actor failed; reset required")]
    Failed,
    #[error("Complex producer/output invariant failed")]
    Invariant,
}
#[derive(Debug, Default)]
pub struct ComplexStep {
    pub outputs: Vec<OutputIntent>,
    pub released_producers: Vec<ActionSlot>,
    /// Forward to the rule which generated this incoming slot.
    pub originating_halt: Option<ActionSlot>,
    pub discarded_input_batches: Vec<BatchId>,
}
struct InputBatch {
    plan: ActionPlan,
    remaining: usize,
}
pub struct ComplexStageActor {
    output: OutputStageActor,
    producers: BindingArena<ProducerDecision>,
    inputs: BTreeMap<BatchId, InputBatch>,
    bindings: BTreeMap<ActionSlot, Option<BatchId>>,
    children: BTreeMap<BatchId, ActionSlot>,
    halted: BTreeSet<ActionSlot>,
    last_batch: u64,
    now: MonoTime,
    failed: bool,
}
fn check_branches(config: &ConfigSnapshot) -> Result<(), ComplexActorError> {
    if config.complex.iter().any(|mapping| {
        let extras = &mapping.extras;
        !extras.if_held_down.is_empty()
            || !extras.if_alone.is_empty()
            || extras
                .delayed
                .as_ref()
                .is_some_and(|d| !d.if_invoked.is_empty() || !d.if_canceled.is_empty())
    }) {
        return Err(ComplexActorError::UnsupportedBranches);
    }
    Ok(())
}
impl ComplexStageActor {
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, ComplexActorError> {
        check_branches(&config)?;
        Ok(Self {
            output: OutputStageActor::new(config)?,
            producers: BindingArena::default(),
            inputs: BTreeMap::new(),
            bindings: BTreeMap::new(),
            children: BTreeMap::new(),
            halted: BTreeSet::new(),
            last_batch: 0,
            now: MonoTime(0),
            failed: false,
        })
    }
    fn running(&self) -> Result<(), ComplexActorError> {
        if self.failed {
            Err(ComplexActorError::Failed)
        } else {
            Ok(())
        }
    }
    fn check_time(&self, at: MonoTime) -> Result<(), ComplexActorError> {
        if at < self.now {
            Err(ComplexActorError::TimeReversed)
        } else {
            Ok(())
        }
    }
    pub fn next_batch_id(&self) -> Result<BatchId, ComplexActorError> {
        self.running()?;
        self.last_batch
            .checked_add(1)
            .map(BatchId)
            .ok_or(ComplexActorError::Batch)
    }
    pub fn is_failed(&self) -> bool {
        self.failed
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        self.output.snapshot()
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.output.next_deadline()
    }
    pub fn reserved_sources(&self) -> usize {
        self.producers.reserved_sources()
    }
    pub fn pending_inputs(&self) -> usize {
        self.inputs.len()
    }
    pub fn producer(&self, slot: ActionSlot) -> Option<&ProducerDecision> {
        self.producers.live(slot)
    }
    pub fn producer_halted(&self, slot: ActionSlot) -> bool {
        self.halted.contains(&slot)
    }
    pub fn generated_modifiers(&self) -> BTreeSet<KeyId> {
        self.output.generated_modifiers()
    }
    pub fn generated_variables(&self) -> BTreeMap<String, VariableValue> {
        self.output.generated_variables()
    }
    /// Register an upstream generated Complex batch before any of its receipts arrive.
    pub fn register_input(&mut self, plan: ActionPlan) -> Result<BatchId, ComplexActorError> {
        self.running()?;
        if plan.origin.stage != Stage::Complex || !Arc::ptr_eq(&plan.origin.config, self.snapshot())
        {
            return Err(ComplexActorError::Input);
        }
        let id = plan.origin.batch;
        if id.0 <= self.last_batch {
            return Err(ComplexActorError::Batch);
        }
        let slots = plan
            .events()
            .iter()
            .filter(|event| event.payload.edge == KeyEdge::Down)
            .count();
        if slots != 0 {
            self.producers
                .prepare(plan.origin.clone(), slots, None)?
                .commit();
            self.inputs.insert(
                id,
                InputBatch {
                    plan,
                    remaining: slots,
                },
            );
        }
        self.last_batch = id.0;
        Ok(id)
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), ComplexActorError> {
        self.running()?;
        check_branches(&config)?;
        self.output.activate(config)?;
        Ok(())
    }
    /// Caller retains the receipt. DeadlinePending has not selected or admitted it yet.
    /// Other admission failures latch Failed; stop capture and recover before reset.
    pub fn admit(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
    ) -> Result<ComplexStep, ComplexActorError> {
        self.running()?;
        self.check_time(due.delivered_at)?;
        if self
            .next_deadline()
            .is_some_and(|deadline| deadline <= due.delivered_at)
        {
            return Err(ComplexActorError::DeadlinePending);
        }
        let result = self.admit_inner(due, context);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn admit_inner(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
    ) -> Result<ComplexStep, ComplexActorError> {
        let slot = due.event.payload.slot;
        let mut step = ComplexStep::default();
        match due.event.payload.edge {
            KeyEdge::Down => {
                let input = self
                    .inputs
                    .get(&slot.batch)
                    .ok_or(ComplexActorError::Input)?;
                let expected = input
                    .plan
                    .events()
                    .iter()
                    .find(|event| event.payload.slot == slot && event.payload.edge == KeyEdge::Down)
                    .ok_or(ComplexActorError::Input)?;
                if due.event.payload != expected.payload || due.event.at_ns < expected.at_ns {
                    return Err(ComplexActorError::Input);
                }
                let up = input
                    .plan
                    .events()
                    .iter()
                    .find(|event| event.payload.slot == slot && event.payload.edge == KeyEdge::Up)
                    .map(|event| &event.payload)
                    .or_else(|| {
                        input
                            .plan
                            .releases()
                            .iter()
                            .find(|action| action.slot == slot)
                    })
                    .ok_or(ComplexActorError::Input)?;
                let decision = self
                    .output
                    .decide_producer_down(due, context)?
                    .with_original_up(up)?;
                let id = self.next_batch_id()?;
                let plan = decision.plan_main(id)?;
                let origin = plan.actions().origin.clone();
                let slots = plan
                    .actions()
                    .events()
                    .iter()
                    .filter(|e| e.payload.edge == KeyEdge::Down)
                    .count();
                let generated_halt = plan
                    .actions()
                    .events()
                    .iter()
                    .any(|event| event.payload.halt);
                let halt = decision.originating_halt();
                let receipt = DueEvent {
                    event: due.event.clone(),
                    delivered_at: due.delivered_at,
                };
                if slots == 0 {
                    let activation = self
                        .producers
                        .prepare_consumed_down(origin, receipt, decision)?;
                    self.output
                        .prepare_empty_child_scoped(plan, activation)?
                        .commit();
                    self.bindings.insert(slot, None);
                } else {
                    let registration = self
                        .producers
                        .prepare_child_with_down(origin, slots, receipt, decision)?;
                    self.output
                        .prepare_child_scoped(plan, registration)?
                        .commit();
                    self.bindings.insert(slot, Some(id));
                    self.children.insert(id, slot);
                }
                self.last_batch = id.0;
                if generated_halt {
                    self.halted.insert(slot);
                }
                if halt {
                    step.originating_halt = Some(slot);
                }
            }
            KeyEdge::Up => {
                let input = self
                    .inputs
                    .get(&slot.batch)
                    .ok_or(ComplexActorError::Input)?;
                let earliest = if let Some(event) = input
                    .plan
                    .events()
                    .iter()
                    .find(|e| e.payload.slot == slot && e.payload.edge == KeyEdge::Up)
                {
                    event.at_ns
                } else if input.plan.releases().iter().any(|e| e.slot == slot) {
                    input.plan.ends_at()
                } else {
                    return Err(ComplexActorError::Input);
                };
                if due.event.at_ns < earliest {
                    return Err(ComplexActorError::Input);
                }
                let child = *self.bindings.get(&slot).ok_or(ComplexActorError::Input)?;
                let parent_up = self.producers.prepare_producer_up(due)?;
                if let Some(child) = child {
                    self.output.input_up(child, due.delivered_at)?;
                }
                parent_up.commit();
                self.after_key_up(slot, context, due.delivered_at)?;
                self.finish(slot, &mut step)?;
            }
            KeyEdge::Repeat => return Err(ComplexActorError::Input),
        }
        self.now = due.delivered_at;
        Ok(step)
    }
    fn after_key_up(
        &mut self,
        slot: ActionSlot,
        context: &ContextSnapshot,
        at: MonoTime,
    ) -> Result<(), ComplexActorError> {
        let decision = self
            .producers
            .live(slot)
            .ok_or(ComplexActorError::Invariant)?;
        let Some(selection) = decision.selection() else {
            return Ok(());
        };
        if selection.mapping().extras.after_key_up.is_empty() {
            return Ok(());
        }
        let branch = selection
            .select_branch(
                rule_selection::RuleBranch::AfterKeyUp,
                self.halted.contains(&slot),
                |condition| {
                    self.output
                        .condition_truth(condition, context, decision.down().device, at)
                },
            )
            .ok_or(ComplexActorError::Invariant)?;
        if branch.actions().len() == 0 {
            return Ok(());
        }
        let id = self.next_batch_id()?;
        let plan = modifier_scope::ScopedActionPlan::branch(
            &branch,
            id,
            at,
            decision.down().device,
            decision.down().source,
        )?;
        let slots = plan
            .actions()
            .events()
            .iter()
            .filter(|e| e.payload.edge == KeyEdge::Down)
            .count();
        if slots == 0 {
            return Ok(());
        }
        let generated_halt = plan.actions().events().iter().any(|e| e.payload.halt);
        let registration =
            self.producers
                .prepare(plan.actions().origin.clone(), slots, Some(slot))?;
        self.output
            .prepare_child_scoped(plan, registration)?
            .commit();
        self.children.insert(id, slot);
        self.last_batch = id.0;
        if generated_halt {
            self.halted.insert(slot);
        }
        Ok(())
    }
    fn finish(
        &mut self,
        slot: ActionSlot,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
        if self
            .producers
            .live(slot)
            .is_none_or(|d| d.up_delivered_at().is_none())
        {
            return Ok(());
        }
        match self.producers.finish_producer(slot) {
            Err(ProducerUpError::Binding(BindingError::ChildPending)) => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        self.bindings.remove(&slot);
        self.halted.remove(&slot);
        step.released_producers.push(slot);
        let input = self
            .inputs
            .get_mut(&slot.batch)
            .ok_or(ComplexActorError::Invariant)?;
        input.remaining = input
            .remaining
            .checked_sub(1)
            .ok_or(ComplexActorError::Invariant)?;
        if input.remaining == 0 {
            self.producers.retire(slot.batch)?;
            self.inputs.remove(&slot.batch);
        }
        Ok(())
    }
    /// Deliver one bounded output step. Never internally drains and discards intents.
    pub fn poll(&mut self, at: MonoTime) -> Result<Option<ComplexStep>, ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        let result = self.poll_inner(at);
        if result.is_err() {
            self.failed = true;
        }
        self.now = at;
        result
    }
    fn poll_inner(&mut self, at: MonoTime) -> Result<Option<ComplexStep>, ComplexActorError> {
        let Some(delivery) = self.output.poll_delivered(at)? else {
            return Ok(None);
        };
        let actual = delivery.step;
        let mut step = ComplexStep {
            outputs: actual.outputs,
            ..Default::default()
        };
        if actual.halt_requested {
            let id = delivery.batch.ok_or(ComplexActorError::Invariant)?;
            let parent = *self.children.get(&id).ok_or(ComplexActorError::Invariant)?;
            self.halted.insert(parent);
        }
        if let Some(id) = actual.completed_batch {
            let parent = self
                .children
                .remove(&id)
                .ok_or(ComplexActorError::Invariant)?;
            let canceled = self.producers.cancel_subtree(id)?;
            if canceled.len() != 1 || !canceled[0].payloads.is_empty() {
                return Err(ComplexActorError::Invariant);
            }
            let main = self
                .bindings
                .get_mut(&parent)
                .ok_or(ComplexActorError::Invariant)?;
            if *main == Some(id) {
                *main = None;
            }
            self.finish(parent, &mut step)?;
        }
        Ok(Some(step))
    }
    pub fn observe_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<ComplexStep, ComplexActorError> {
        self.check_time(at)?;
        let result = self.observe_device_inner(device, removed, at);
        self.now = at;
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn observe_device_inner(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<ComplexStep, ComplexActorError> {
        let outputs = self.output.observe_device(device, removed, at)?;
        let ids: Vec<_> = self
            .inputs
            .iter()
            .filter(|(_, input)| {
                input.plan.origin.device.id == device.id
                    && (input.plan.origin.device.generation < device.generation
                        || (removed && input.plan.origin.device.generation == device.generation))
            })
            .map(|(&id, _)| id)
            .collect();
        self.discard_inputs(&ids)?;
        self.now = at;
        Ok(ComplexStep {
            outputs,
            discarded_input_batches: ids,
            ..Default::default()
        })
    }
    fn discard_inputs(&mut self, ids: &[BatchId]) -> Result<(), ComplexActorError> {
        for &id in ids {
            let canceled = self.producers.cancel_subtree(id)?;
            for batch in canceled {
                self.children.remove(&batch.origin.batch);
            }
            self.bindings.retain(|slot, _| slot.batch != id);
            self.halted.retain(|slot| slot.batch != id);
            self.inputs.remove(&id);
        }
        Ok(())
    }
    pub fn reset(&mut self, at: MonoTime) -> Result<ComplexStep, ComplexActorError> {
        self.check_time(at)?;
        let result = self.reset_inner(at);
        self.now = at;
        self.failed = result.is_err();
        result
    }
    fn reset_inner(&mut self, at: MonoTime) -> Result<ComplexStep, ComplexActorError> {
        let outputs = self.output.reset(at)?;
        let ids: Vec<_> = self.inputs.keys().copied().collect();
        self.discard_inputs(&ids)?;
        self.now = at;
        self.failed = false;
        Ok(ComplexStep {
            outputs,
            discarded_input_batches: ids,
            ..Default::default()
        })
    }
}
