//! Owned Simple main generation and downstream Complex/Output execution.
//! Pure stage component; native acceptance remains separate.
mod scopes;
use crate::{
    action_plan::{ActionPlan, ActionSlot, BatchId, PlanError, PlannedAction},
    complex_actor::{ComplexActorError, ComplexStageActor, ComplexStep},
    generated_stage::{GeneratedError, GeneratedStageQueue, ScopedGeneration},
    producer_decision::{ProducerDecision, ProducerUpError},
    stage_bindings::{BindingArena, BindingError},
    timeline::{DueEvent, Stage},
    *,
};
use scopes::ScopeRun;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SimpleActorError {
    #[error(transparent)]
    Complex(#[from] ComplexActorError),
    #[error(transparent)]
    Generated(#[from] GeneratedError),
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Up(#[from] ProducerUpError),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("invalid registered Simple input receipt")]
    Input,
    #[error("batch identity must increase and must not overflow")]
    Batch,
    #[error("Simple actor clock moved backwards")]
    TimeReversed,
    #[error("poll due downstream work before admitting input")]
    DeadlinePending,
    #[error("Simple advanced branches are not implemented")]
    UnsupportedSimpleSemantics,
    #[error("Simple actor failed; reset required")]
    Failed,
    #[error("Simple producer/downstream invariant failed")]
    Invariant,
}
#[derive(Debug, Default)]
pub struct SimpleStep {
    pub outputs: Vec<OutputIntent>,
    pub forwarded_pointers: Vec<DueEvent<crate::pointer_work::PointerMotion>>,
    pub released_producers: Vec<ActionSlot>,
    /// Halt belongs to the rule which generated the registered Simple input.
    pub originating_halt: Option<ActionSlot>,
    pub discarded_input_batches: Vec<BatchId>,
}
struct InputBatch {
    plan: ActionPlan,
    remaining: usize,
    unreleased: usize,
}
struct Child {
    saved: ScopedGeneration,
    parent: ActionSlot,
    remaining: usize,
    up_seen: bool,
    repeat_slot: Option<ActionSlot>,
}
pub struct SimpleStageActor {
    generated: GeneratedStageQueue,
    complex: ComplexStageActor,
    producers: BindingArena<ProducerDecision>,
    inputs: BTreeMap<BatchId, InputBatch>,
    bindings: BTreeMap<ActionSlot, Option<BatchId>>,
    children: BTreeMap<BatchId, Child>,
    halted: BTreeSet<ActionSlot>,
    scope_runs: BTreeMap<BatchId, ScopeRun>,
    removed_scope_sources: BTreeSet<ActionSlot>,
    last_batch: u64,
    now: MonoTime,
    failed: bool,
}
impl SimpleStageActor {
    fn check_simple(config: &ConfigSnapshot) -> Result<(), SimpleActorError> {
        if config.simple.iter().any(|mapping| {
            let extras = &mapping.extras;
            !extras.if_held_down.is_empty()
                || !extras.if_alone.is_empty()
                || !extras.after_key_up.is_empty()
                || extras
                    .delayed
                    .as_ref()
                    .is_some_and(|d| !d.if_invoked.is_empty() || !d.if_canceled.is_empty())
        }) {
            return Err(SimpleActorError::UnsupportedSimpleSemantics);
        }
        Ok(())
    }
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, SimpleActorError> {
        Self::new_with_source_limit(config, u64::MAX)
    }
    pub(crate) fn new_with_source_limit(
        config: Arc<ConfigSnapshot>,
        max_source_end: u64,
    ) -> Result<Self, SimpleActorError> {
        Self::check_simple(&config)?;
        Ok(Self {
            generated: GeneratedStageQueue::new_complex_propagating(config.clone())?,
            complex: ComplexStageActor::new_with_source_limit(config, max_source_end)?,
            producers: BindingArena::default(),
            inputs: BTreeMap::new(),
            bindings: BTreeMap::new(),
            children: BTreeMap::new(),
            halted: BTreeSet::new(),
            scope_runs: BTreeMap::new(),
            removed_scope_sources: BTreeSet::new(),
            last_batch: 0,
            now: MonoTime(0),
            failed: false,
        })
    }
    fn running(&self) -> Result<(), SimpleActorError> {
        if self.failed {
            Err(SimpleActorError::Failed)
        } else {
            Ok(())
        }
    }
    fn check_time(&self, at: MonoTime) -> Result<(), SimpleActorError> {
        if at < self.now {
            Err(SimpleActorError::TimeReversed)
        } else {
            Ok(())
        }
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        self.generated.snapshot()
    }
    pub fn is_failed(&self) -> bool {
        self.failed
    }
    pub fn pending_inputs(&self) -> usize {
        self.inputs.len()
    }
    pub fn pending_children(&self) -> usize {
        self.children.len()
    }
    pub(crate) fn output_model(&self) -> (usize, Vec<KeyId>, usize) {
        let (owners, pressed, pending) = self.complex.output_model();
        (owners, pressed, pending + self.generated.queued_events())
    }
    /// Accounting in the external Simple tree only; not a unified allstage source budget.
    pub fn reserved_sources(&self) -> usize {
        self.producers.reserved_sources()
    }
    pub fn producer(&self, slot: ActionSlot) -> Option<&ProducerDecision> {
        self.producers.live(slot)
    }
    /// Read the current retained primary leaf without selection, generation or ownership changes.
    pub fn physical_repeat_target(&self, slot: ActionSlot) -> Option<KeyId> {
        if self.failed || self.producers.live(slot)?.up_delivered_at().is_some() {
            return None;
        }
        let id = self.bindings.get(&slot).copied().flatten()?;
        let child = self.children.get(&id)?;
        if child.up_seen {
            return None;
        }
        self.complex.physical_repeat_target(child.repeat_slot?)
    }
    pub(crate) fn enqueue_pointer(
        &mut self,
        event: timeline::StageEvent<crate::pointer_work::PointerMotion>,
    ) -> Result<(), SimpleActorError> {
        self.running()?;
        self.check_time(event.at_ns)?;
        if event.stage != Stage::Complex || !Arc::ptr_eq(&event.config, self.snapshot()) {
            return Err(SimpleActorError::Input);
        }
        let at = event.at_ns;
        self.generated.prepare_pointer(event)?.commit();
        self.now = at;
        Ok(())
    }
    pub fn producer_halted(&self, slot: ActionSlot) -> bool {
        self.halted.contains(&slot)
    }
    pub fn generated_modifiers(&self) -> BTreeSet<KeyId> {
        self.generated.modifiers()
    }
    pub fn generated_variables(&self) -> BTreeMap<String, VariableValue> {
        self.generated.variable_values()
    }
    pub fn output_generated_variables(&self) -> BTreeMap<String, VariableValue> {
        self.complex.generated_variables()
    }
    pub fn next_batch_id(&self) -> Result<BatchId, SimpleActorError> {
        self.running()?;
        let next = self
            .last_batch
            .checked_add(1)
            .ok_or(SimpleActorError::Batch)?;
        Ok(BatchId(next.max(self.complex.next_batch_id()?.0)))
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.generated
            .next_deadline()
            .into_iter()
            .chain(self.complex.next_deadline())
            .min()
    }
    /// Register a physical adapter's current-snapshot Simple plan before receipts arrive.
    /// Empty registration consumes its identity but creates no root binding.
    pub fn register_input(&mut self, plan: ActionPlan) -> Result<BatchId, SimpleActorError> {
        self.running()?;
        if plan.origin.stage != Stage::Simple || !Arc::ptr_eq(&plan.origin.config, self.snapshot())
        {
            return Err(SimpleActorError::Input);
        }
        self.generated.check_live_device(plan.origin.device)?;
        if plan.origin.batch.0 < self.next_batch_id()?.0 {
            return Err(SimpleActorError::Batch);
        }
        let id = plan.origin.batch;
        let slots = plan
            .events()
            .iter()
            .filter(|e| e.payload.edge == KeyEdge::Down)
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
                    unreleased: slots,
                },
            );
        }
        self.last_batch = id.0;
        Ok(id)
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), SimpleActorError> {
        self.running()?;
        Self::check_simple(&config)?;
        let names = self.generated.check_activation(&config)?;
        self.complex.activate(config.clone())?;
        self.generated.commit_activation(config, names);
        Ok(())
    }
    pub fn admit(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
    ) -> Result<SimpleStep, SimpleActorError> {
        self.running()?;
        self.check_time(due.delivered_at)?;
        if self
            .next_deadline()
            .is_some_and(|deadline| deadline <= due.delivered_at)
        {
            return Err(SimpleActorError::DeadlinePending);
        }
        let result = self.admit_inner(due, context);
        self.now = due.delivered_at;
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn admit_inner(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
    ) -> Result<SimpleStep, SimpleActorError> {
        let slot = due.event.payload.slot;
        let mut step = SimpleStep::default();
        let input = self
            .inputs
            .get(&slot.batch)
            .ok_or(SimpleActorError::Input)?;
        match due.event.payload.edge {
            KeyEdge::Down => {
                let expected = input
                    .plan
                    .events()
                    .iter()
                    .find(|e| e.payload.slot == slot && e.payload.edge == KeyEdge::Down)
                    .ok_or(SimpleActorError::Input)?;
                if due.event.payload != expected.payload || due.event.at_ns < expected.at_ns {
                    return Err(SimpleActorError::Input);
                }
                let up = input
                    .plan
                    .events()
                    .iter()
                    .find(|e| e.payload.slot == slot && e.payload.edge == KeyEdge::Up)
                    .map(|e| &e.payload)
                    .or_else(|| input.plan.releases().iter().find(|e| e.slot == slot))
                    .ok_or(SimpleActorError::Input)?;
                self.producers.check_down(slot)?;
                let decision = self
                    .generated
                    .decide_producer_down(due, context)?
                    .with_original_up(up)?;
                let halt = decision.originating_halt();
                let id = self.next_batch_id()?;
                let plan = decision.plan_main(id)?;
                let slots = plan
                    .actions()
                    .events()
                    .iter()
                    .filter(|e| e.payload.edge == KeyEdge::Down)
                    .count();
                let receipt = DueEvent {
                    event: due.event.clone(),
                    delivered_at: due.delivered_at,
                };
                let generated_halt = plan.actions().events().iter().any(|e| e.payload.halt);
                if slots == 0 {
                    let activation = self.producers.prepare_consumed_down(
                        plan.actions().origin.clone(),
                        receipt,
                        decision,
                    )?;
                    self.complex
                        .prepare_generated_empty_input(&mut self.generated, &plan, activation)?
                        .commit();
                    self.bindings.insert(slot, None);
                } else {
                    let reservation = self.producers.prepare_child_with_down(
                        plan.actions().origin.clone(),
                        slots,
                        receipt,
                        decision,
                    )?;
                    let saved = self
                        .complex
                        .prepare_generated_child_input(&mut self.generated, &plan, reservation)?
                        .commit();
                    self.bindings.insert(slot, Some(id));
                    self.children.insert(
                        id,
                        Child {
                            saved,
                            parent: slot,
                            remaining: slots,
                            up_seen: false,
                            repeat_slot: plan
                                .actions()
                                .releases()
                                .first()
                                .map(|action| action.slot),
                        },
                    );
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
                let earliest = input
                    .plan
                    .events()
                    .iter()
                    .find(|e| e.payload.slot == slot && e.payload.edge == KeyEdge::Up)
                    .map(|e| e.at_ns)
                    .or_else(|| {
                        input
                            .plan
                            .releases()
                            .iter()
                            .any(|e| e.slot == slot)
                            .then_some(input.plan.ends_at())
                    })
                    .ok_or(SimpleActorError::Input)?;
                if due.event.at_ns < earliest {
                    return Err(SimpleActorError::Input);
                }
                let id = *self.bindings.get(&slot).ok_or(SimpleActorError::Input)?;
                let marker = self.producers.prepare_producer_up(due)?;
                if let Some(id) = id {
                    let child = self
                        .children
                        .get_mut(&id)
                        .ok_or(SimpleActorError::Invariant)?;
                    if let Some(up) = self
                        .generated
                        .prepare_input_up_scoped(&mut child.saved, due.delivered_at)?
                    {
                        up.commit();
                    }
                    child.up_seen = true;
                }
                marker.commit();
                let input = self
                    .inputs
                    .get_mut(&slot.batch)
                    .ok_or(SimpleActorError::Invariant)?;
                input.unreleased = input
                    .unreleased
                    .checked_sub(1)
                    .ok_or(SimpleActorError::Invariant)?;
                if let Some(id) = id {
                    self.finish_child(id, &mut step)?;
                }
                self.finish(slot, &mut step)?;
                if Self::device_idle(&self.inputs, due.event.device) {
                    step.outputs.extend(
                        self.complex
                            .synchronize_if_device_idle(due.event.device, due.delivered_at)?,
                    );
                }
            }
            KeyEdge::Repeat => return Err(SimpleActorError::Input),
        }
        Ok(step)
    }
    fn device_idle(inputs: &BTreeMap<BatchId, InputBatch>, device: DeviceInstanceId) -> bool {
        !inputs
            .values()
            .any(|input| input.plan.origin.device == device && input.unreleased != 0)
    }
    fn finish(&mut self, slot: ActionSlot, step: &mut SimpleStep) -> Result<(), SimpleActorError> {
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
            .ok_or(SimpleActorError::Invariant)?;
        input.remaining = input
            .remaining
            .checked_sub(1)
            .ok_or(SimpleActorError::Invariant)?;
        if input.remaining == 0 {
            self.producers.retire(slot.batch)?;
            self.inputs.remove(&slot.batch);
        }
        Ok(())
    }
    fn finish_child(&mut self, id: BatchId, step: &mut SimpleStep) -> Result<(), SimpleActorError> {
        let child = self.children.get(&id).ok_or(SimpleActorError::Invariant)?;
        if child.remaining != 0 || !child.up_seen || !self.generated.scope_delivery_complete(id) {
            return Ok(());
        }
        let parent = child.parent;
        self.generated.retire_batch(id)?;
        let canceled = self.producers.cancel_subtree(id)?;
        if canceled.len() != 1 || !canceled[0].payloads.is_empty() {
            return Err(SimpleActorError::Invariant);
        }
        self.children.remove(&id);
        *self
            .bindings
            .get_mut(&parent)
            .ok_or(SimpleActorError::Invariant)? = None;
        self.finish(parent, step)
    }
    fn absorb(&mut self, actual: ComplexStep) -> Result<SimpleStep, SimpleActorError> {
        if !actual.discarded_input_batches.is_empty() {
            return Err(SimpleActorError::Invariant);
        }
        let mut step = SimpleStep {
            outputs: actual.outputs,
            forwarded_pointers: actual.forwarded_pointers,
            ..Default::default()
        };
        if let Some(slot) = actual.originating_halt {
            let child = self
                .children
                .get(&slot.batch)
                .ok_or(SimpleActorError::Invariant)?;
            self.halted.insert(child.parent);
        }
        for slot in actual.released_producers {
            let child = self
                .children
                .get_mut(&slot.batch)
                .ok_or(SimpleActorError::Invariant)?;
            child.remaining = child
                .remaining
                .checked_sub(1)
                .ok_or(SimpleActorError::Invariant)?;
            self.finish_child(slot.batch, &mut step)?;
        }
        Ok(step)
    }
    /// One bounded downstream or generation delivery. Caller must retain every returned intent.
    pub fn poll(&mut self, at: MonoTime) -> Result<Option<SimpleStep>, SimpleActorError> {
        self.poll_with_context(at, &ContextSnapshot::default())
    }
    pub fn poll_with_context(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
    ) -> Result<Option<SimpleStep>, SimpleActorError> {
        self.running()?;
        self.check_time(at)?;
        let result = self.poll_inner(at, context);
        self.now = at;
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn poll_inner(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
    ) -> Result<Option<SimpleStep>, SimpleActorError> {
        let inputs = &self.inputs;
        if self
            .complex
            .next_deadline()
            .is_some_and(|deadline| deadline <= at)
        {
            let actual = self
                .complex
                .poll_with_context_and_eof(at, context, |device| Self::device_idle(inputs, device))?
                .ok_or(SimpleActorError::Invariant)?;
            return self.absorb(actual).map(Some);
        }
        if self
            .generated
            .next_deadline()
            .is_some_and(|deadline| deadline <= at)
        {
            match self
                .generated
                .poll_mixed(at)?
                .ok_or(SimpleActorError::Invariant)?
            {
                crate::generated_stage::GeneratedDelivery::Action(due) => {
                    let actual = self.complex.admit_with_eof(&due, context, |device| {
                        Self::device_idle(inputs, device)
                    })?;
                    return self.absorb(actual).map(Some);
                }
                crate::generated_stage::GeneratedDelivery::Projection(_) => {
                    return Err(SimpleActorError::Invariant);
                }
                crate::generated_stage::GeneratedDelivery::Scope(due) => {
                    let step = self.deliver_scope(due)?;
                    return Ok(Some(step));
                }
                crate::generated_stage::GeneratedDelivery::Pointer(due) => {
                    self.complex.admit_pointer(&due)?;
                    return Ok(Some(SimpleStep::default()));
                }
            }
        }
        self.generated.advance(at)?;
        if let Some(actual) = self
            .complex
            .poll_with_context_and_eof(at, context, |device| Self::device_idle(inputs, device))?
        {
            return self.absorb(actual).map(Some);
        }
        Ok(None)
    }
    pub fn observe_wheel(
        &mut self,
        horizontal: i32,
        vertical: i32,
        at: MonoTime,
    ) -> Result<(), SimpleActorError> {
        self.running()?;
        self.check_time(at)?;
        let result = self
            .complex
            .observe_wheel(horizontal, vertical, at)
            .map_err(SimpleActorError::from)
            .and_then(|()| self.generated.advance(at).map_err(SimpleActorError::from));
        self.now = at;
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn observe_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<SimpleStep, SimpleActorError> {
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
    ) -> Result<SimpleStep, SimpleActorError> {
        self.generated.check_device(device, removed)?;
        let canceled = self.generated.slots_for_device_removal(device, removed);
        let actual = self.complex.observe_device(device, removed, at)?;
        self.remember_scope_source_removal(&canceled);
        let restored = self.cleanup_scope_device(device, removed, at)?;
        self.generated.observe_device(device, removed)?;
        self.remember_scope_source_removal(&[]);
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
        Ok(SimpleStep {
            outputs: actual.outputs.into_iter().chain(restored).collect(),
            discarded_input_batches: ids,
            ..Default::default()
        })
    }
    fn discard_inputs(&mut self, ids: &[BatchId]) -> Result<(), SimpleActorError> {
        for &id in ids {
            for batch in self.producers.cancel_subtree(id)? {
                self.children.remove(&batch.origin.batch);
            }
            self.bindings.retain(|slot, _| slot.batch != id);
            self.halted.retain(|slot| slot.batch != id);
            self.inputs.remove(&id);
        }
        Ok(())
    }
    pub fn reset(&mut self, at: MonoTime) -> Result<SimpleStep, SimpleActorError> {
        self.check_time(at)?;
        let result = self.reset_inner(at);
        self.now = at;
        self.failed = result.is_err();
        result
    }
    fn reset_inner(&mut self, at: MonoTime) -> Result<SimpleStep, SimpleActorError> {
        let actual = self.complex.reset(at)?;
        self.generated.reset(at)?;
        self.scope_runs.clear();
        self.removed_scope_sources.clear();
        let ids: Vec<_> = self.inputs.keys().copied().collect();
        self.discard_inputs(&ids)?;
        Ok(SimpleStep {
            outputs: actual.outputs,
            discarded_input_batches: ids,
            ..Default::default()
        })
    }
}
