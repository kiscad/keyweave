//! Owned Complex producer generation/execution.
mod groups;
mod projections;
use crate::{
    action_plan::{ActionPlan, ActionSlot, BatchId, PlanError, PlannedAction},
    generated_stage::{
        GeneratedError, GeneratedStageQueue, PreparedEmptyGeneratedChild, PreparedGeneratedChild,
        ScopedGeneration,
    },
    output_stage::{OutputStageActor, OutputStageError},
    producer_decision::{ProducerDecision, ProducerUpError},
    stage_bindings::{BindingArena, BindingError},
    timeline::{DueEvent, Stage},
    *,
};
pub use groups::MatchedGroupStep;
pub use projections::ComplexProjectionId;
use projections::DescendantProjection;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ComplexActorError {
    #[error(transparent)]
    Output(#[from] OutputStageError),
    #[error(transparent)]
    Generated(#[from] GeneratedError),
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Up(#[from] ProducerUpError),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Group(#[from] simultaneous::GroupOwnerError),
    #[error("invalid registered Complex input receipt")]
    Input,
    #[error("batch identity must increase and must not overflow")]
    Batch,
    #[error("Complex actor clock moved backwards")]
    TimeReversed,
    #[error("branch deadline would overflow monotonic time")]
    TimeOverflow,
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
    pub forwarded_pointers: Vec<DueEvent<crate::pointer_work::PointerMotion>>,
    pub released_producers: Vec<ActionSlot>,
    /// Forward to the rule which generated this incoming slot.
    pub originating_halt: Option<ActionSlot>,
    pub discarded_input_batches: Vec<BatchId>,
}
struct InputBatch {
    plan: ActionPlan,
    remaining: usize,
    unreleased: usize,
}
pub struct ComplexStageActor {
    output: OutputStageActor,
    producers: BindingArena<ProducerDecision>,
    inputs: BTreeMap<BatchId, InputBatch>,
    bindings: BTreeMap<ActionSlot, Option<BatchId>>,
    children: BTreeMap<BatchId, ActionSlot>,
    halted: BTreeSet<ActionSlot>,
    projection_groups: BTreeMap<ActionSlot, DescendantProjection>,
    projection_handles: BTreeMap<ComplexProjectionId, ActionSlot>,
    groups: BTreeMap<ActionSlot, simultaneous::GroupBindingOwner>,
    group_members: BTreeMap<ActionSlot, ActionSlot>,
    quarantined_group_holds: Vec<stage_bindings::SharedRetention>,
    pending_group_initial: BTreeSet<ActionSlot>,
    next_projection: u64,
    alone_candidate: Option<ActionSlot>,
    held: Option<(MonoTime, ActionSlot)>,
    delayed: Option<(MonoTime, ActionSlot)>,
    last_batch: u64,
    now: MonoTime,
    failed: bool,
}
/// Exclusive root registration preflight; compose with upstream generation before committing either.
#[must_use = "commit or abort the prepared Complex input registration"]
pub struct PreparedComplexInput<'a> {
    reservation: Option<stage_bindings::Reservation<'a, ProducerDecision>>,
    inputs: &'a mut BTreeMap<BatchId, InputBatch>,
    last_batch: &'a mut u64,
    plan: ActionPlan,
    slots: usize,
}
impl PreparedComplexInput<'_> {
    /// Return the original owned plan without reserving sources, input metadata or ID.
    pub fn abort(self) -> ActionPlan {
        self.plan
    }
    pub fn commit(self) -> BatchId {
        let id = self.plan.origin.batch;
        if let Some(reservation) = self.reservation {
            reservation.commit();
        }
        if self.slots != 0 {
            self.inputs.insert(
                id,
                InputBatch {
                    plan: self.plan,
                    remaining: self.slots,
                    unreleased: self.slots,
                },
            );
        }
        *self.last_batch = id.0;
        id
    }
}
enum PreparedInputGeneration<'a, T> {
    Child(PreparedGeneratedChild<'a, T>),
    Empty(PreparedEmptyGeneratedChild<'a, T>),
}
/// Owns parent activation/registration, destination generation and actual input registration.
/// Only commit may expose the generated original-Up credential.
#[must_use = "commit or abort all three parts of the Complex input transaction"]
pub struct PreparedComplexGeneratedInput<'a, T> {
    input: PreparedComplexInput<'a>,
    generated: PreparedInputGeneration<'a, T>,
}
impl<T> PreparedComplexGeneratedInput<'_, T> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) -> ScopedGeneration {
        self.input.commit();
        match self.generated {
            PreparedInputGeneration::Child(generated) => generated.commit(),
            PreparedInputGeneration::Empty(generated) => generated.commit(),
        }
    }
}
impl ComplexStageActor {
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, ComplexActorError> {
        Self::new_with_source_limit(config, u64::MAX)
    }
    pub(crate) fn new_with_source_limit(
        config: Arc<ConfigSnapshot>,
        max_source_end: u64,
    ) -> Result<Self, ComplexActorError> {
        config
            .validate_timed_stages(true)
            .map_err(GeneratedError::from)?;
        Ok(Self::from_output(OutputStageActor::new_with_source_limit(
            config,
            max_source_end,
        )?))
    }
    fn from_output(output: OutputStageActor) -> Self {
        Self {
            output,
            producers: BindingArena::default(),
            inputs: BTreeMap::new(),
            bindings: BTreeMap::new(),
            children: BTreeMap::new(),
            halted: BTreeSet::new(),
            projection_groups: BTreeMap::new(),
            projection_handles: BTreeMap::new(),
            groups: BTreeMap::new(),
            group_members: BTreeMap::new(),
            quarantined_group_holds: Vec::new(),
            pending_group_initial: BTreeSet::new(),
            next_projection: 0,
            alone_candidate: None,
            held: None,
            delayed: None,
            last_batch: 0,
            now: MonoTime(0),
            failed: false,
        }
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
        self.output
            .next_deadline()
            .into_iter()
            .chain(self.held.map(|(at, _)| at))
            .chain(self.delayed.map(|(at, _)| at))
            .chain((!self.pending_group_initial.is_empty()).then_some(self.now))
            .min()
    }
    pub(crate) fn output_model(&self) -> (usize, Vec<KeyId>, usize) {
        (
            self.output.output_owner_count(),
            self.output.pressed_outputs(),
            self.output.queued_events()
                + usize::from(self.held.is_some())
                + usize::from(self.delayed.is_some())
                + self.pending_group_initial.len(),
        )
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
    /// Follow this retained Complex producer's current actual main child, including held replacement.
    /// This is a query only; native desired/submitted state and repeat policy remain separate.
    pub fn physical_repeat_target(&self, slot: ActionSlot) -> Option<KeyId> {
        if self.failed || self.producers.live(slot)?.up_delivered_at().is_some() {
            return None;
        }
        let child = self.bindings.get(&slot).copied().flatten()?;
        self.output.physical_repeat_target(child)
    }
    pub(crate) fn admit_pointer(
        &mut self,
        due: &DueEvent<crate::pointer_work::PointerMotion>,
    ) -> Result<(), ComplexActorError> {
        self.running()?;
        self.check_time(due.delivered_at)?;
        if self
            .next_deadline()
            .is_some_and(|at| at <= due.delivered_at)
        {
            return Err(ComplexActorError::DeadlinePending);
        }
        let event = timeline::StageEvent {
            stage: Stage::Output,
            at_ns: due.delivered_at,
            device: due.event.device,
            source: due.event.source,
            config: due.event.config.clone(),
            payload: due.event.payload.clone(),
        };
        let result = self
            .output
            .enqueue_pointer(event)
            .map_err(ComplexActorError::from);
        if result.is_err() {
            self.failed = true;
            return result;
        }
        if let crate::pointer_work::PointerMotion::Wheel {
            horizontal,
            vertical,
        } = due.event.payload
            && (horizontal != 0 || vertical != 0)
        {
            self.alone_candidate = None;
        }
        self.now = due.delivered_at;
        Ok(())
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
        Ok(self.prepare_input(plan)?.commit())
    }
    pub fn prepare_input(
        &mut self,
        plan: ActionPlan,
    ) -> Result<PreparedComplexInput<'_>, ComplexActorError> {
        self.prepare_input_inner(plan, None)
    }
    /// Joint admission from a retained or jointly activated Simple parent.
    /// Snapshot extra reservations are charged by generation; do not charge them twice.
    pub fn prepare_generated_child_input<'a, T>(
        &'a mut self,
        source: &'a mut GeneratedStageQueue,
        plan: &modifier_scope::ScopedActionPlan,
        registration: stage_bindings::Reservation<'a, T>,
    ) -> Result<PreparedComplexGeneratedInput<'a, T>, ComplexActorError> {
        if !Arc::ptr_eq(source.snapshot(), self.snapshot()) {
            return Err(ComplexActorError::Input);
        }
        let admission = registration.child_admission()?;
        let input = self.prepare_input_inner(plan.actions().clone(), Some(&admission))?;
        let generated = source.prepare_complex_child_scoped(plan, registration)?;
        Ok(PreparedComplexGeneratedInput {
            input,
            generated: PreparedInputGeneration::Child(generated),
        })
    }
    /// An empty matched main consumes the original Down without creating a child root.
    pub fn prepare_generated_empty_input<'a, T>(
        &'a mut self,
        source: &'a mut GeneratedStageQueue,
        plan: &modifier_scope::ScopedActionPlan,
        activation: stage_bindings::PreparedConsumedDown<'a, T>,
    ) -> Result<PreparedComplexGeneratedInput<'a, T>, ComplexActorError> {
        if !Arc::ptr_eq(source.snapshot(), self.snapshot()) {
            return Err(ComplexActorError::Input);
        }
        let admission = activation.admission();
        let input = self.prepare_input_inner(plan.actions().clone(), Some(&admission))?;
        let generated = source.prepare_empty_complex_child_scoped(plan, activation)?;
        Ok(PreparedComplexGeneratedInput {
            input,
            generated: PreparedInputGeneration::Empty(generated),
        })
    }
    fn prepare_input_inner(
        &mut self,
        plan: ActionPlan,
        child: Option<&stage_bindings::ChildAdmission<'_>>,
    ) -> Result<PreparedComplexInput<'_>, ComplexActorError> {
        self.running()?;
        let slots = plan
            .events()
            .iter()
            .filter(|event| event.payload.edge == KeyEdge::Down)
            .count();
        let authorized = match child {
            Some(child) => {
                child.matches(&plan.origin, slots)
                    && (plan.origin.config.generation < self.snapshot().generation
                        || Arc::ptr_eq(&plan.origin.config, self.snapshot()))
            }
            None => Arc::ptr_eq(&plan.origin.config, self.snapshot()),
        };
        if plan.origin.stage != Stage::Complex || !authorized {
            return Err(ComplexActorError::Input);
        }
        self.output.check_live_device(plan.origin.device)?;
        let id = plan.origin.batch;
        if id.0 <= self.last_batch {
            return Err(ComplexActorError::Batch);
        }
        let reservation = if slots != 0 {
            Some(self.producers.prepare(plan.origin.clone(), slots, None)?)
        } else {
            None
        };
        Ok(PreparedComplexInput {
            reservation,
            inputs: &mut self.inputs,
            last_batch: &mut self.last_batch,
            plan,
            slots,
        })
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), ComplexActorError> {
        self.running()?;
        config
            .validate_timed_stages(true)
            .map_err(GeneratedError::from)?;
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
        self.admit_with_eof(due, context, |_| true)
    }
    pub(crate) fn admit_with_eof(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
        allow_sync: impl Fn(DeviceInstanceId) -> bool,
    ) -> Result<ComplexStep, ComplexActorError> {
        self.running()?;
        self.check_time(due.delivered_at)?;
        if self
            .next_deadline()
            .is_some_and(|deadline| deadline <= due.delivered_at)
        {
            return Err(ComplexActorError::DeadlinePending);
        }
        let result = self.admit_inner(due, context, allow_sync);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn admit_inner(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
        allow_sync: impl Fn(DeviceInstanceId) -> bool,
    ) -> Result<ComplexStep, ComplexActorError> {
        let slot = due.event.payload.slot;
        let mut step = ComplexStep::default();
        match due.event.payload.edge {
            KeyEdge::Down => {
                let input = self
                    .inputs
                    .get(&slot.batch)
                    .ok_or(ComplexActorError::Input)?;
                let has_identity = !input.plan.snapshot_modifier_slots.contains(&slot);
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
                    .cloned()
                    .ok_or(ComplexActorError::Input)?;
                self.producers.check_down(slot)?;
                self.held = None;
                if let Some((_, previous)) = self.delayed.take() {
                    let _ = self.branch(
                        previous,
                        rule_selection::RuleBranch::DelayedCanceled,
                        context,
                        due.delivered_at,
                    )?;
                    self.finish(previous, &mut step)?;
                }
                let decision = self
                    .output
                    .decide_producer_down_with_identity(due, context, has_identity)?
                    .with_original_up(&up)?;
                let id = self.next_batch_id()?;
                let plan = decision.plan_main(id)?;
                let origin = plan.actions().origin.clone();
                let slots = plan
                    .actions()
                    .events()
                    .iter()
                    .filter(|e| e.payload.edge == KeyEdge::Down)
                    .count();
                let momentary = matches!(due.event.payload.target, ActionTarget::Key(_));
                let candidate = momentary
                    && decision
                        .selection()
                        .is_some_and(|rule| !rule.mapping().extras.if_alone.is_empty());
                let held = if let Some(rule) = decision
                    .selection()
                    .filter(|r| !r.mapping().extras.if_held_down.is_empty())
                {
                    Some((
                        MonoTime(
                            due.delivered_at
                                .0
                                .checked_add(rule.mapping().extras.held_threshold_ns)
                                .ok_or(ComplexActorError::TimeOverflow)?,
                        ),
                        slot,
                    ))
                } else {
                    None
                };
                let delayed = if let Some(action) = decision
                    .selection()
                    .and_then(|rule| rule.mapping().extras.delayed.as_ref())
                    .filter(|d| !d.if_invoked.is_empty() || !d.if_canceled.is_empty())
                {
                    Some((
                        MonoTime(
                            due.delivered_at
                                .0
                                .checked_add(action.delay_ns)
                                .ok_or(ComplexActorError::TimeOverflow)?,
                        ),
                        slot,
                    ))
                } else {
                    None
                };
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
                    if let Some(group) = self.projection_groups.get_mut(&slot) {
                        let projection = self
                            .output
                            .prepare_projected_child_scoped(plan, registration)?
                            .commit();
                        group.outputs.push(projection);
                    } else {
                        self.output
                            .prepare_child_scoped(plan, registration)?
                            .commit();
                    }
                    self.bindings.insert(slot, Some(id));
                    self.children.insert(id, slot);
                }
                self.last_batch = id.0;
                self.held = held;
                self.delayed = delayed;
                if momentary {
                    self.alone_candidate = if candidate { Some(slot) } else { None };
                }
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
                if let Some(&primary) = self.group_members.get(&slot) {
                    self.admit_group_up(primary, due, context, &mut step)?;
                    if allow_sync(due.event.device)
                        && !self.inputs.values().any(|input| {
                            input.plan.origin.device == due.event.device && input.unreleased != 0
                        })
                    {
                        step.outputs.extend(
                            self.output
                                .synchronize_if_device_idle(due.event.device, due.delivered_at)?,
                        );
                    }
                    return Ok(step);
                }
                let child = *self.bindings.get(&slot).ok_or(ComplexActorError::Input)?;
                let parent_up = self.producers.prepare_producer_up(due)?;
                if let Some(child) = child {
                    self.output.input_up(child, due.delivered_at)?;
                }
                parent_up.commit();
                let input = self
                    .inputs
                    .get_mut(&slot.batch)
                    .ok_or(ComplexActorError::Invariant)?;
                input.unreleased = input
                    .unreleased
                    .checked_sub(1)
                    .ok_or(ComplexActorError::Invariant)?;
                if self.held.is_some_and(|(_, candidate)| candidate == slot) {
                    self.held = None;
                }
                let alone = self.alone_candidate == Some(slot);
                if alone {
                    self.alone_candidate = None;
                }
                if alone {
                    let decision = self
                        .producers
                        .live(slot)
                        .ok_or(ComplexActorError::Invariant)?;
                    let within = decision.selection().is_some_and(|rule| {
                        due.delivered_at.0 - decision.delivered_at().0
                            < rule.mapping().extras.alone_timeout_ns
                    });
                    if within {
                        let _ = self.branch(
                            slot,
                            rule_selection::RuleBranch::Alone,
                            context,
                            due.delivered_at,
                        )?;
                    }
                }
                let _ = self.branch(
                    slot,
                    rule_selection::RuleBranch::AfterKeyUp,
                    context,
                    due.delivered_at,
                )?;
                self.finish(slot, &mut step)?;
                if allow_sync(due.event.device)
                    && !self.inputs.values().any(|input| {
                        input.plan.origin.device == due.event.device && input.unreleased != 0
                    })
                {
                    step.outputs.extend(
                        self.output
                            .synchronize_if_device_idle(due.event.device, due.delivered_at)?,
                    );
                }
            }
            KeyEdge::Repeat => return Err(ComplexActorError::Input),
        }
        self.now = due.delivered_at;
        Ok(step)
    }
    fn branch(
        &mut self,
        slot: ActionSlot,
        kind: rule_selection::RuleBranch,
        context: &ContextSnapshot,
        at: MonoTime,
    ) -> Result<Option<BatchId>, ComplexActorError> {
        self.branch_for_trigger(slot, kind, context, at, slot)
    }
    fn branch_for_trigger(
        &mut self,
        slot: ActionSlot,
        kind: rule_selection::RuleBranch,
        context: &ContextSnapshot,
        at: MonoTime,
        trigger_slot: ActionSlot,
    ) -> Result<Option<BatchId>, ComplexActorError> {
        let decision = self
            .producers
            .live(slot)
            .ok_or(ComplexActorError::Invariant)?;
        let trigger = self
            .producers
            .live(trigger_slot)
            .ok_or(ComplexActorError::Invariant)?;
        let Some(selection) = decision.selection() else {
            return Ok(None);
        };
        let empty = match kind {
            rule_selection::RuleBranch::Held => selection.mapping().extras.if_held_down.is_empty(),
            rule_selection::RuleBranch::Alone => selection.mapping().extras.if_alone.is_empty(),
            rule_selection::RuleBranch::DelayedInvoked => selection
                .mapping()
                .extras
                .delayed
                .as_ref()
                .is_none_or(|d| d.if_invoked.is_empty()),
            rule_selection::RuleBranch::DelayedCanceled => selection
                .mapping()
                .extras
                .delayed
                .as_ref()
                .is_none_or(|d| d.if_canceled.is_empty()),
            rule_selection::RuleBranch::AfterKeyUp => {
                selection.mapping().extras.after_key_up.is_empty()
            }
            rule_selection::RuleBranch::GroupAfterKeyUp => selection
                .mapping()
                .extras
                .simultaneous
                .as_ref()
                .is_none_or(|group| group.after_key_up.is_empty()),
        };
        if empty {
            return Ok(None);
        }
        let branch = selection
            .select_branch(kind, self.halted.contains(&slot), |condition| {
                self.output
                    .condition_truth_for_producer(condition, context, trigger, at)
            })
            .ok_or(ComplexActorError::Invariant)?;
        if branch.actions().len() == 0 {
            return Ok(None);
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
            return Ok(None);
        }
        let generated_halt = plan.actions().events().iter().any(|e| e.payload.halt);
        let registration =
            self.producers
                .prepare(plan.actions().origin.clone(), slots, Some(slot))?;
        if let Some(group) = self.projection_groups.get_mut(&slot) {
            let projection = self
                .output
                .prepare_projected_child_scoped(plan, registration)?
                .commit();
            group.outputs.push(projection);
        } else {
            self.output
                .prepare_child_scoped(plan, registration)?
                .commit();
        }
        self.children.insert(id, slot);
        self.last_batch = id.0;
        if generated_halt {
            self.halted.insert(slot);
        }
        Ok(Some(id))
    }
    fn finish(
        &mut self,
        slot: ActionSlot,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
        if let Some(&primary) = self.group_members.get(&slot) {
            return self.finish_group(primary, step);
        }
        if self.delayed.is_some_and(|(_, candidate)| candidate == slot) {
            return Ok(());
        }
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
        self.finish_input_metadata(slot, step)
    }
    fn finish_input_metadata(
        &mut self,
        slot: ActionSlot,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
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
        self.poll_with_context(at, &ContextSnapshot::default())
    }
    /// Timers use this fresh context at their actual generating poll.
    pub fn poll_with_context(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
    ) -> Result<Option<ComplexStep>, ComplexActorError> {
        self.poll_with_context_and_eof(at, context, |_| true)
    }
    pub(crate) fn poll_with_context_and_eof(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
        allow_sync: impl Fn(DeviceInstanceId) -> bool,
    ) -> Result<Option<ComplexStep>, ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        let result = if self.output.next_deadline().is_some_and(|due| due <= at) {
            self.poll_inner(at, &allow_sync)
        } else if let Some(&primary) = self.pending_group_initial.first() {
            self.initialize_group_up(primary, at, context).map(Some)
        } else {
            let held = self.held.filter(|(due, _)| *due <= at);
            let delayed = self.delayed.filter(|(due, _)| *due <= at);
            if held.is_some_and(|(due, _)| delayed.is_none_or(|(other, _)| due <= other)) {
                self.fire_held(at, context).map(Some)
            } else if delayed.is_some() {
                self.fire_delayed(at, context).map(Some)
            } else {
                self.poll_inner(at, &allow_sync)
            }
        };
        if result.is_err() {
            self.failed = true;
        }
        self.now = at;
        result
    }
    fn fire_delayed(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
    ) -> Result<ComplexStep, ComplexActorError> {
        let (_, slot) = self.delayed.take().ok_or(ComplexActorError::Invariant)?;
        let _ = self.branch(
            slot,
            rule_selection::RuleBranch::DelayedInvoked,
            context,
            at,
        )?;
        let mut step = ComplexStep::default();
        self.finish(slot, &mut step)?;
        Ok(step)
    }
    fn fire_held(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
    ) -> Result<ComplexStep, ComplexActorError> {
        let (_, slot) = self.held.take().ok_or(ComplexActorError::Invariant)?;
        let old = *self
            .bindings
            .get(&slot)
            .ok_or(ComplexActorError::Invariant)?;
        if let Some(old) = old {
            self.output.input_up(old, at)?;
        }
        let new = self.branch(slot, rule_selection::RuleBranch::Held, context, at)?;
        self.bindings.insert(slot, new);
        if let Some(group) = self.groups.get_mut(&slot) {
            group.replace_main(new, &self.output)?;
            for &member in group.members() {
                self.bindings.insert(member, new);
            }
        }
        Ok(ComplexStep::default())
    }
    fn poll_inner(
        &mut self,
        at: MonoTime,
        allow_sync: &impl Fn(DeviceInstanceId) -> bool,
    ) -> Result<Option<ComplexStep>, ComplexActorError> {
        let inputs = &self.inputs;
        let Some(delivery) = self.output.poll_delivered_with_eof(at, |device| {
            allow_sync(device)
                && !inputs
                    .values()
                    .any(|input| input.plan.origin.device == device && input.unreleased != 0)
        })?
        else {
            return Ok(None);
        };
        let actual = delivery.step;
        if let Some(id) = actual.completed_batch
            && let Some(parent) = self.children.get(&id)
            && let Some(group) = self.groups.get_mut(parent)
        {
            group.observe_output_step(&actual, &self.output)?;
        }
        let mut step = ComplexStep {
            outputs: actual.outputs,
            forwarded_pointers: delivery.pointer.into_iter().collect(),
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
            if let Some(group) = self.groups.get(&parent) {
                for &member in group.members() {
                    if self.bindings.get(&member) == Some(&Some(id)) {
                        self.bindings.insert(member, None);
                    }
                }
            }
            self.finish(parent, &mut step)?;
        }
        Ok(Some(step))
    }
    pub(crate) fn synchronize_if_device_idle(
        &mut self,
        device: DeviceInstanceId,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        if self
            .inputs
            .values()
            .any(|input| input.plan.origin.device == device && input.unreleased != 0)
        {
            return Ok(Vec::new());
        }
        let outputs = self.output.synchronize_if_device_idle(device, at)?;
        self.now = at;
        Ok(outputs)
    }
    /// Eligibility only. Caller forwards pointer input and coordinates modifier sync separately.
    pub fn observe_wheel(
        &mut self,
        horizontal: i32,
        vertical: i32,
        at: MonoTime,
    ) -> Result<(), ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        if horizontal != 0 || vertical != 0 {
            self.alone_candidate = None;
        }
        self.now = at;
        Ok(())
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
        let removed_slots: BTreeSet<_> = self
            .projection_groups
            .iter()
            .filter(|(_, group)| {
                group.device.id == device.id
                    && (group.device.generation < device.generation
                        || (removed && group.device.generation == device.generation))
            })
            .map(|(&slot, _)| slot)
            .collect();
        self.projection_groups
            .retain(|slot, _| !removed_slots.contains(slot));
        self.projection_handles
            .retain(|_, slot| !removed_slots.contains(slot));
        self.now = at;
        Ok(ComplexStep {
            outputs,
            discarded_input_batches: ids,
            ..Default::default()
        })
    }
    fn discard_inputs(&mut self, ids: &[BatchId]) -> Result<(), ComplexActorError> {
        for &id in ids {
            if self.delayed.is_some_and(|(_, slot)| slot.batch == id) {
                self.delayed = None;
            }
            if self.held.is_some_and(|(_, slot)| slot.batch == id) {
                self.held = None;
            }
            if self.alone_candidate.is_some_and(|slot| slot.batch == id) {
                self.alone_candidate = None;
            }
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
        self.release_group_holds_for_reset()?;
        let ids: Vec<_> = self.inputs.keys().copied().collect();
        self.discard_inputs(&ids)?;
        self.projection_groups.clear();
        self.projection_handles.clear();
        self.now = at;
        self.failed = false;
        Ok(ComplexStep {
            outputs,
            discarded_input_batches: ids,
            ..Default::default()
        })
    }
}
