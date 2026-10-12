//! Execute planned terminal work on one actor, saving real bindings per slot.
//! This is not yet the simple/complex matching actor or a platform runtime.
use crate::{
    action_plan::{ActionPlan, BatchId},
    timeline::{Stage, StageTimeline, TimelineError},
    *,
};

mod projections;
use projections::KeyProjection;
pub use projections::KeyProjectionId;
pub(crate) use projections::{
    PreparedKeyProjection, PreparedKeyProjectionClose, ProjectionCommand,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActorError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Timeline(#[from] TimelineError),
    #[error("only terminal-stage plans can be executed by this actor")]
    UnsupportedStage,
    #[error("plan must originate from the actor's current immutable snapshot")]
    InvalidSnapshot,
    #[error("batch identities must strictly increase")]
    InvalidBatch,
    #[error("bounded actor slot, batch or distinct-key capacity exceeded")]
    Capacity,
    #[error("saved slot lifecycle invariant failed; runtime must recover")]
    InvalidSlot,
    #[error("actor execution failed; reset is required before ordinary work")]
    Failed,
    #[error("invalid key projection lifecycle")]
    InvalidProjection,
    #[error("variable projection requires the generated/actual variable protocol")]
    UnsupportedVariableProjection,
}

/// Allows at most one pending down and one cleanup up per distinct key on control events.
pub const MAX_ACTOR_KEYS: usize = MAX_OUTPUTS_PER_EVENT / 2;

pub(crate) struct PreparedTerminalPointer<'a> {
    timeline: timeline::PreparedStageAppend<'a, TerminalWork>,
    now: &'a mut MonoTime,
    at: MonoTime,
}
impl PreparedTerminalPointer<'_> {
    pub(crate) fn commit(self) {
        self.timeline.commit();
        *self.now = self.at;
    }
}
struct Batch {
    plan: ActionPlan,
    base_source: u64,
    remaining: usize,
    up_queued: bool,
    keys: BTreeSet<KeyId>,
    scope: Option<BatchScope>,
}
struct BatchScope {
    pending: Option<modifier_scope::ModifierScope>,
    active: Option<modifier_scope::ScopeBinding>,
    remaining: usize,
    until_input_up: bool,
    source: u64,
    reserved: usize,
}
#[derive(Debug)]
pub struct ActorStep {
    pub outputs: Vec<OutputIntent>,
    /// The caller propagates this to the original rule binding's halt state.
    pub halt_requested: bool,
    pub completed_batch: Option<BatchId>,
}

pub(crate) struct TerminalDelivery {
    pub(crate) step: ActorStep,
    pub(crate) action: Option<timeline::StageEvent<action_plan::PlannedAction>>,
    pub(crate) pointer: Option<timeline::DueEvent<crate::pointer_work::PointerMotion>>,
    pub(crate) projection: Option<timeline::StageEvent<ProjectionCommand>>,
}
pub(crate) enum TerminalWork {
    Action(action_plan::PlannedAction),
    ScopeUp(BatchId),
    Pointer(crate::pointer_work::PointerMotion),
    ProjectionDown(KeyProjectionId, BatchId),
    ProjectionUp(KeyProjectionId, BatchId),
}
impl TerminalWork {
    fn batch(&self) -> Option<BatchId> {
        match self {
            Self::Action(action) => Some(action.slot.batch),
            Self::ScopeUp(id) => Some(*id),
            Self::Pointer(_) => None,
            Self::ProjectionDown(_, batch) | Self::ProjectionUp(_, batch) => Some(*batch),
        }
    }
}
fn action_event(
    event: timeline::StageEvent<action_plan::PlannedAction>,
) -> timeline::StageEvent<TerminalWork> {
    timeline::StageEvent {
        stage: event.stage,
        at_ns: event.at_ns,
        device: event.device,
        source: event.source,
        config: event.config,
        payload: TerminalWork::Action(event.payload),
    }
}
pub struct TerminalActor {
    core: Engine,
    timeline: StageTimeline<TerminalWork>,
    batches: BTreeMap<BatchId, Batch>,
    bindings: stage_bindings::BindingArena<Bound>,
    last_batch: Option<BatchId>,
    scope_slots: usize,
    max_source_end: u64,
    projections: BTreeMap<KeyProjectionId, KeyProjection>,
    next_projection: u64,
    variable_projection_refs: BTreeMap<action_plan::ActionSlot, usize>,
    failed: bool,
}
/// Exclusive preflight of timeline, bindings, source IDs and scope reservations.
/// Drop/abort leaves the actor unchanged; actual owners are opened only by poll.
#[must_use = "commit or discard the prepared terminal admission"]
pub struct PreparedTerminalAppend<'a> {
    timeline: timeline::PreparedStageAppend<'a, TerminalWork>,
    reservation: Option<stage_bindings::Reservation<'a, Bound>>,
    batches: &'a mut BTreeMap<BatchId, Batch>,
    next_source: &'a mut u64,
    last_batch: &'a mut Option<BatchId>,
    scope_slots: &'a mut usize,
    batch: Option<Batch>,
    id: BatchId,
    next_source_value: u64,
    scope_reserved: usize,
}
impl PreparedTerminalAppend<'_> {
    /// The consumed plan is discarded, as for a rejected enqueue; retain a
    /// caller-owned recipe when a retry is required.
    pub fn abort(self) {
        drop(self);
    }
    /// No business checks remain. Allocation failure/panic is outside this guarantee.
    pub fn commit(self) {
        self.timeline.commit();
        if let Some(reservation) = self.reservation {
            reservation.commit();
        }
        *self.next_source = self.next_source_value;
        *self.last_batch = Some(self.id);
        *self.scope_slots += self.scope_reserved;
        if let Some(batch) = self.batch {
            self.batches.insert(self.id, batch);
        }
    }
}
/// Prepared saved Up, including a scope marker when the original recipe needs one.
#[must_use = "commit or discard the prepared terminal original Up"]
pub struct PreparedTerminalUp<'a> {
    timeline: timeline::PreparedStageAppend<'a, TerminalWork>,
    saved: &'a mut Batch,
    now: &'a mut MonoTime,
    at: MonoTime,
}
impl PreparedTerminalUp<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.timeline.commit();
        self.saved.up_queued = true;
        *self.now = self.at;
    }
}
impl TerminalActor {
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, ActorError> {
        Self::new_with_source_limit(config, u64::MAX)
    }
    pub(crate) fn new_with_source_limit(
        config: Arc<ConfigSnapshot>,
        max_source_end: u64,
    ) -> Result<Self, ActorError> {
        Ok(Self {
            core: Engine::new_inner(config, true)?,
            timeline: StageTimeline::new(),
            batches: BTreeMap::new(),
            bindings: stage_bindings::BindingArena::default(),
            last_batch: None,
            scope_slots: 0,
            max_source_end,
            projections: BTreeMap::new(),
            next_projection: 0,
            variable_projection_refs: BTreeMap::new(),
            failed: false,
        })
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        &self.core.config
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), ActorError> {
        self.core.activate_inner(config, true)?;
        Ok(())
    }
    pub(crate) fn check_live_device(&self, device: DeviceInstanceId) -> Result<(), ActorError> {
        Ok(self.timeline.check_live_device(device)?)
    }
    pub(crate) fn check_device(
        &self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<(), ActorError> {
        self.check_time(at)?;
        self.timeline.check_device(device, removed)?;
        Ok(())
    }
    pub(crate) fn check_activation(
        &self,
        config: &Arc<ConfigSnapshot>,
    ) -> Result<BTreeSet<String>, ActorError> {
        Ok(self.core.check_activation(config, true)?)
    }
    pub(crate) fn commit_activation(
        &mut self,
        config: Arc<ConfigSnapshot>,
        names: BTreeSet<String>,
    ) {
        self.core.commit_activation(config, names);
    }
    pub fn live_slots(&self) -> usize {
        self.bindings.live_slots()
    }
    pub fn hidden_modifier_references(&self) -> usize {
        self.core.suppression_references
    }
    pub fn pending_batches(&self) -> usize {
        self.batches.len()
    }
    pub fn reserved_slots(&self) -> usize {
        self.bindings.reserved_slots() + self.scope_slots
    }
    pub fn queued_events(&self) -> usize {
        self.timeline.len()
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.timeline.next_deadline()
    }
    /// Original already-filtered plan, including initial events and saved Up.
    /// This does not report timeline-shifted delivery, Live visibility or native acceptance.
    pub fn saved_plan(&self, batch: BatchId) -> Option<&ActionPlan> {
        self.batches.get(&batch).map(|saved| &saved.plan)
    }
    pub fn pressed_outputs(&self) -> Vec<KeyId> {
        self.core.emitted.iter().copied().collect()
    }
    /// No owners/intents are created. Only the retained main keyboard target may
    /// repeat; an admitted original Up stops eligibility before its actual delivery.
    pub fn physical_repeat_target(&self, batch: BatchId) -> Option<KeyId> {
        if self.failed {
            return None;
        }
        let saved = self.batches.get(&batch)?;
        if saved.up_queued {
            return None;
        }
        let slot = saved.plan.releases().first()?.slot;
        self.core.retained_repeat_target(self.bindings.live(slot)?)
    }
    pub fn variable_values(&self) -> BTreeMap<String, VariableValue> {
        self.core.variable_values()
    }
    fn check_time(&self, at: MonoTime) -> Result<(), ActorError> {
        if at < self.core.now {
            return Err(EngineError::TimeBackwards.into());
        }
        Ok(())
    }
    fn advance(&mut self, at: MonoTime) -> Result<(), ActorError> {
        self.check_time(at)?;
        self.timeline.advance(at)?;
        self.core.now = at;
        Ok(())
    }
    /// A physical control event precedes ordinary work at this same time.
    pub fn observe_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, ActorError> {
        self.check_time(at)?;
        self.timeline.observe_device(device, removed)?;
        self.advance(at)?;
        let ids: Vec<_> = self
            .batches
            .iter()
            .filter(|(_, batch)| {
                batch.plan.origin.device.id == device.id
                    && (batch.plan.origin.device.generation < device.generation
                        || (removed && batch.plan.origin.device.generation == device.generation))
            })
            .map(|(id, _)| *id)
            .collect();
        let canceled = !ids.is_empty();
        let mut outputs = Vec::new();
        for id in ids {
            self.cancel(id, &mut outputs)?;
        }
        let projected: Vec<_> = self
            .projections
            .values()
            .filter(|projection| {
                projection.origin.device.id == device.id
                    && (projection.origin.device.generation < device.generation
                        || (removed && projection.origin.device.generation == device.generation))
            })
            .map(|projection| projection.origin.batch)
            .collect();
        for batch in projected {
            self.cancel_key_projections(batch)?;
        }
        if removed || canceled {
            self.core.sync_modifiers(&mut outputs);
        }
        Ok(outputs)
    }
    /// Atomic admission; source ranges are reserved before any planned down runs.
    pub fn enqueue(&mut self, plan: ActionPlan) -> Result<(), ActorError> {
        self.prepare(plan)?.commit();
        Ok(())
    }
    pub fn prepare(&mut self, plan: ActionPlan) -> Result<PreparedTerminalAppend<'_>, ActorError> {
        self.prepare_inner(plan, None, None)
    }
    /// Scope must describe this frozen plan. Scope owners are opened on its first
    /// delivered event, not on admission; empty plans never open a scope.
    pub fn enqueue_scoped(
        &mut self,
        plan: modifier_scope::ScopedActionPlan,
    ) -> Result<(), ActorError> {
        self.prepare_scoped(plan)?.commit();
        Ok(())
    }
    pub fn prepare_scoped(
        &mut self,
        plan: modifier_scope::ScopedActionPlan,
    ) -> Result<PreparedTerminalAppend<'_>, ActorError> {
        let (plan, scope) = plan.into_parts();
        self.prepare_inner(plan, Some(scope), None)
    }
    pub(crate) fn prepare_child_scoped(
        &mut self,
        plan: modifier_scope::ScopedActionPlan,
        child: &stage_bindings::ChildAdmission<'_>,
    ) -> Result<PreparedTerminalAppend<'_>, ActorError> {
        let (plan, scope) = plan.into_parts();
        self.prepare_inner(plan, Some(scope), Some(child))
    }
    fn prepare_inner(
        &mut self,
        plan: ActionPlan,
        scope: Option<modifier_scope::ModifierScope>,
        child: Option<&stage_bindings::ChildAdmission<'_>>,
    ) -> Result<PreparedTerminalAppend<'_>, ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        if plan.origin.stage != Stage::Output {
            return Err(ActorError::UnsupportedStage);
        }
        let slots = plan
            .events
            .iter()
            .filter(|e| e.payload.edge == KeyEdge::Down)
            .count();
        if let Some(child) = child {
            if !child.matches(&plan.origin, slots)
                || (plan.origin.config.generation >= self.core.config.generation
                    && !Arc::ptr_eq(&plan.origin.config, &self.core.config))
            {
                return Err(ActorError::InvalidSnapshot);
            }
            plan.origin.config.validate_timed_stages(true)?;
            plan.origin.config.variable_names()?;
        } else if !Arc::ptr_eq(&plan.origin.config, &self.core.config) {
            return Err(ActorError::InvalidSnapshot);
        }
        if self.last_batch.is_some_and(|id| plan.origin.batch <= id) {
            return Err(ActorError::InvalidBatch);
        }
        let scope_up =
            !plan.events.is_empty() && scope.as_ref().is_some_and(|scope| scope.until_input_up());
        let remaining = plan.events.len() + plan.release_on_input_up.len() + usize::from(scope_up);
        let scope_reserved = if remaining == 0 {
            0
        } else {
            scope.as_ref().map_or(0, |scope| scope.reservation())
        };
        if self.batches.len() >= MAX_BINDINGS
            || self
                .reserved_slots()
                .saturating_add(slots)
                .saturating_add(scope_reserved)
                > MAX_OUTPUT_OWNERS
        {
            return Err(ActorError::Capacity);
        }
        let mut keys: BTreeSet<_> = self
            .batches
            .values()
            .flat_map(|batch| batch.keys.iter().copied())
            .chain(self.core.emitted.iter().copied())
            .collect();
        let mut batch_keys = BTreeSet::new();
        for event in &plan.events {
            match &event.payload.target {
                ActionTarget::Key(key) if key.usage != 0 => {
                    keys.insert(*key);
                    batch_keys.insert(*key);
                }
                ActionTarget::SetVariable(variable) => {
                    let valid = match &variable.operation {
                        VariableOperation::Set {
                            value,
                            key_up_value,
                        } => {
                            (value.is_some() || key_up_value.is_some())
                                && value
                                    .iter()
                                    .chain(key_up_value.iter())
                                    .all(VariableValue::bounded)
                        }
                        VariableOperation::Unset => true,
                    };
                    if !self.core.variables.contains_key(&variable.name) || !valid {
                        return Err(EngineError::InvalidVariable.into());
                    }
                }
                _ => {}
            }
        }
        if remaining > 0
            && let Some(scope) = &scope
        {
            for key in scope.keys() {
                keys.insert(key);
                batch_keys.insert(key);
            }
        }
        if keys.len() > MAX_ACTOR_KEYS {
            return Err(ActorError::Capacity);
        }
        let base_source = self.core.next_source;
        let scope_source = base_source
            .checked_add(slots as u64)
            .ok_or(EngineError::ResourceLimit)?;
        let next_source = scope_source
            .checked_add(scope_reserved as u64)
            .filter(|end| *end <= self.max_source_end)
            .ok_or(EngineError::ResourceLimit)?;
        let scope_events = plan.events.len();
        let up_queued = plan.release_on_input_up.is_empty() && !scope_up;
        let events = plan.events.iter().cloned().map(action_event).collect();
        let reservation = if remaining > 0 {
            Some(
                self.bindings
                    .prepare(plan.origin.clone(), slots, None)
                    .map_err(|_| ActorError::InvalidSlot)?,
            )
        } else {
            None
        };
        let timeline = self
            .timeline
            .prepare_enqueue(events)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?;
        let id = plan.origin.batch;
        let batch = if remaining > 0 {
            let scope = scope.map(|spec| BatchScope {
                until_input_up: spec.until_input_up(),
                pending: Some(spec),
                active: None,
                remaining: scope_events,
                source: scope_source,
                reserved: scope_reserved,
            });
            Some(Batch {
                plan,
                base_source,
                remaining,
                up_queued,
                keys: batch_keys,
                scope,
            })
        } else {
            None
        };
        Ok(PreparedTerminalAppend {
            timeline,
            reservation,
            batches: &mut self.batches,
            next_source: &mut self.core.next_source,
            last_batch: &mut self.last_batch,
            scope_slots: &mut self.scope_slots,
            batch,
            id,
            next_source_value: next_source,
            scope_reserved,
        })
    }
    /// Unknown/completed or duplicate original up is a no-op, not a new release.
    pub fn input_up(&mut self, batch: BatchId, at: MonoTime) -> Result<bool, ActorError> {
        let Some(prepared) = self.prepare_input_up(batch, at)? else {
            return Ok(false);
        };
        prepared.commit();
        Ok(true)
    }
    /// Due work must drain first. Preparation leaves the saved Up and clock intact.
    pub fn prepare_input_up(
        &mut self,
        batch: BatchId,
        at: MonoTime,
    ) -> Result<Option<PreparedTerminalUp<'_>>, ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        self.check_time(at)?;
        if self
            .timeline
            .next_deadline()
            .is_some_and(|deadline| deadline <= at)
        {
            return Err(EngineError::DeadlinePending.into());
        }
        let Some(saved) = self.batches.get_mut(&batch) else {
            return Ok(None);
        };
        if saved.up_queued {
            return Ok(None);
        }
        let mut events: Vec<_> = saved
            .plan
            .input_up_events(at)
            .into_iter()
            .map(action_event)
            .collect();
        if saved
            .scope
            .as_ref()
            .is_some_and(|scope| scope.until_input_up)
        {
            let origin = &saved.plan.origin;
            events.push(timeline::StageEvent {
                stage: origin.stage,
                at_ns: at.max(saved.plan.ends_at()),
                device: origin.device,
                source: origin.source,
                config: Arc::clone(&origin.config),
                payload: TerminalWork::ScopeUp(batch),
            });
        }
        let timeline = self
            .timeline
            .prepare_enqueue_at(events, at)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?;
        Ok(Some(PreparedTerminalUp {
            timeline,
            saved,
            now: &mut self.core.now,
            at,
        }))
    }
    /// Execute one due stage event. Late delivery uses actual actor time.
    pub(crate) fn prepare_pointer(
        &mut self,
        event: timeline::StageEvent<crate::pointer_work::PointerMotion>,
    ) -> Result<PreparedTerminalPointer<'_>, ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        if event.stage != Stage::Output {
            return Err(ActorError::UnsupportedStage);
        }
        if !Arc::ptr_eq(&event.config, &self.core.config)
            && event.config.generation >= self.core.config.generation
        {
            return Err(ActorError::InvalidSnapshot);
        }
        self.timeline.check_live_device(event.device)?;
        let event = timeline::StageEvent {
            stage: event.stage,
            at_ns: event.at_ns,
            device: event.device,
            source: event.source,
            config: event.config,
            payload: TerminalWork::Pointer(event.payload),
        };
        let at = event.at_ns;
        let timeline = self
            .timeline
            .prepare_enqueue_at(vec![event], at)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?;
        Ok(PreparedTerminalPointer {
            timeline,
            now: &mut self.core.now,
            at,
        })
    }
    pub(crate) fn synchronize_modifiers(
        &mut self,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        self.check_time(at)?;
        self.timeline.advance(at)?;
        self.core.now = at;
        let mut outputs = Vec::new();
        self.core.sync_modifiers(&mut outputs);
        Ok(outputs)
    }
    pub fn poll(&mut self, at: MonoTime) -> Result<Option<ActorStep>, ActorError> {
        Ok(self.poll_delivered(at)?.map(|delivery| delivery.step))
    }
    /// Receipt describes successful engine delivery, never OS acceptance.
    pub(crate) fn poll_delivered(
        &mut self,
        at: MonoTime,
    ) -> Result<Option<TerminalDelivery>, ActorError> {
        self.poll_delivered_with_eof(at, |_| true)
    }
    pub(crate) fn poll_delivered_with_eof(
        &mut self,
        at: MonoTime,
        allow_sync: impl Fn(DeviceInstanceId) -> bool,
    ) -> Result<Option<TerminalDelivery>, ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        self.check_time(at)?;
        let Some(due) = self.timeline.pop_due(at)? else {
            self.core.now = at;
            return Ok(None);
        };
        self.core.now = at;
        self.core.current_device = due.event.device;
        if let TerminalWork::ProjectionDown(id, batch) | TerminalWork::ProjectionUp(id, batch) =
            &due.event.payload
        {
            let event = timeline::StageEvent {
                stage: due.event.stage,
                at_ns: due.event.at_ns,
                device: due.event.device,
                source: due.event.source,
                config: due.event.config.clone(),
                payload: ProjectionCommand {
                    id: *id,
                    batch: *batch,
                    opening: matches!(due.event.payload, TerminalWork::ProjectionDown(_, _)),
                },
            };
            return self.deliver_key_projection(event).map(Some);
        }
        if let TerminalWork::Pointer(pointer) = &due.event.payload {
            let mut outputs = Vec::new();
            self.core.sync_pointer_modifiers(&mut outputs);
            return Ok(Some(TerminalDelivery {
                step: ActorStep {
                    outputs,
                    halt_requested: false,
                    completed_batch: None,
                },
                action: None,
                projection: None,
                pointer: Some(timeline::DueEvent {
                    event: timeline::StageEvent {
                        stage: due.event.stage,
                        at_ns: due.event.at_ns,
                        device: due.event.device,
                        source: due.event.source,
                        config: due.event.config.clone(),
                        payload: pointer.clone(),
                    },
                    delivered_at: due.delivered_at,
                }),
            }));
        }
        let work = due.event.payload;
        let id = work.batch().ok_or(ActorError::InvalidSlot)?;
        let action = match work {
            TerminalWork::Action(action) => Some(action),
            TerminalWork::ScopeUp(_) => None,
            TerminalWork::Pointer(_)
            | TerminalWork::ProjectionDown(_, _)
            | TerminalWork::ProjectionUp(_, _) => return Err(ActorError::InvalidSlot),
        };
        let batch = self.batches.get_mut(&id).ok_or(ActorError::InvalidSlot)?;
        if let Some(scope) = &mut batch.scope
            && let Some(spec) = scope.pending.take()
        {
            scope.active = Some(match spec.open(&mut self.core, scope.source) {
                Ok(active) => active,
                Err(error) => {
                    self.failed = true;
                    return Err(error.into());
                }
            });
        }
        let config = Arc::clone(&batch.plan.origin.config);
        let mut outputs = Vec::new();
        if let Some(action) = &action {
            let mut cursor = batch.base_source + u64::from(action.slot.index);
            match action.edge {
                KeyEdge::Down => {
                    self.bindings
                        .check_down(action.slot)
                        .map_err(|_| ActorError::InvalidSlot)?;
                    let bound = match &action.target {
                        ActionTarget::Key(key) => self.core.press_stage(
                            KeyRequest {
                                key: *key,
                                lazy_down: action.lazy,
                                lazy_up: action.lazy,
                            },
                            2,
                            &config,
                            &mut cursor,
                            &mut outputs,
                        ),
                        ActionTarget::SetVariable(variable) => {
                            self.core.sync_modifiers(&mut outputs);
                            let key_up_value = match &variable.operation {
                                VariableOperation::Set {
                                    value,
                                    key_up_value,
                                } => {
                                    if let Some(value) = value {
                                        self.core
                                            .assign_variable(&variable.name, Some(value.clone()));
                                    }
                                    key_up_value.clone()
                                }
                                VariableOperation::Unset => {
                                    self.core.assign_variable(&variable.name, None);
                                    None
                                }
                            };
                            Bound::Variable {
                                name: variable.name.clone(),
                                key_up_value,
                            }
                        }
                    };
                    self.bindings
                        .insert(action.slot, bound)
                        .map_err(|_| ActorError::InvalidSlot)?;
                }
                KeyEdge::Up => {
                    let mut bound = self
                        .bindings
                        .take(action.slot)
                        .map_err(|_| ActorError::InvalidSlot)?;
                    // Attached modifier scopes may have non-lazy Down and lazy Up.
                    // Release uses the saved planned Up, not the Down's flag.
                    if let Bound::Leaf(owner) = &mut bound {
                        owner.lazy_up = action.lazy;
                    }
                    self.core
                        .release_bound(bound, &config, &mut cursor, false, &mut outputs);
                }
                KeyEdge::Repeat => return Err(ActorError::InvalidSlot),
            }
        }
        let batch = self.batches.get_mut(&id).ok_or(ActorError::InvalidSlot)?;
        batch.remaining -= 1;
        if let Some(scope) = &mut batch.scope {
            scope.remaining = scope.remaining.saturating_sub(1);
            if ((scope.remaining == 0 && !scope.until_input_up) || batch.remaining == 0)
                && let Some(active) = scope.active.take()
            {
                active.close(&mut self.core);
            }
        }
        let completed_batch = if batch.remaining == 0 {
            self.bindings
                .retire(id)
                .map_err(|_| ActorError::InvalidSlot)?;
            let batch = self.batches.remove(&id).ok_or(ActorError::InvalidSlot)?;
            self.scope_slots -= batch.scope.as_ref().map_or(0, |scope| scope.reserved);
            if allow_sync(due.event.device)
                && !self
                    .batches
                    .values()
                    .any(|other| other.plan.origin.device == due.event.device)
            {
                self.core.sync_modifiers(&mut outputs);
            }
            Some(id)
        } else {
            None
        };
        let receipt = action.as_ref().map(|action| timeline::StageEvent {
            stage: due.event.stage,
            at_ns: due.event.at_ns,
            device: due.event.device,
            source: due.event.source,
            config: due.event.config,
            payload: action.clone(),
        });
        Ok(Some(TerminalDelivery {
            step: ActorStep {
                outputs,
                halt_requested: action.is_some_and(|action| action.halt),
                completed_batch,
            },
            action: receipt,
            pointer: None,
            projection: None,
        }))
    }
    pub fn cancel_batch(
        &mut self,
        id: BatchId,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, ActorError> {
        self.advance(at)?;
        self.cancel_key_projections(id)?;
        if !self.batches.contains_key(&id) {
            return Ok(Vec::new());
        }
        let mut outputs = Vec::new();
        self.cancel(id, &mut outputs)?;
        self.core.sync_modifiers(&mut outputs);
        Ok(outputs)
    }
    fn cancel(&mut self, id: BatchId, outputs: &mut Vec<OutputIntent>) -> Result<(), ActorError> {
        self.cancel_key_projections(id)?;
        if !self.batches.contains_key(&id) {
            return Ok(());
        }
        let bounds = self
            .bindings
            .cancel_subtree(id)
            .map_err(|_| ActorError::InvalidSlot)?;
        self.timeline
            .discard_where(|event| event.payload.batch() == Some(id));
        let batch = self.batches.remove(&id).ok_or(ActorError::InvalidSlot)?;
        for canceled in bounds {
            for saved in canceled.payloads {
                let mut cursor = batch.base_source;
                self.core.release_bound(
                    saved.payload,
                    &canceled.origin.config,
                    &mut cursor,
                    false,
                    outputs,
                );
            }
        }
        if let Some(scope) = batch.scope {
            self.scope_slots -= scope.reserved;
            if let Some(active) = scope.active {
                active.close(&mut self.core);
            }
        }
        Ok(())
    }
    pub fn reset(&mut self, at: MonoTime) -> Result<Vec<OutputIntent>, ActorError> {
        self.check_time(at)?;
        self.timeline.reset(at)?;
        self.projections.clear();
        self.variable_projection_refs.clear();
        self.batches.clear();
        self.bindings = stage_bindings::BindingArena::default();
        self.scope_slots = 0;
        self.failed = false;
        Ok(self.core.reset(at)?)
    }
}

#[cfg(test)]
mod admission_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn aborted_source_reservation_preserves_exact_cursor_and_last_identity() {
        let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
        let mut actor = TerminalActor::new(config.clone()).unwrap();
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        actor.core.next_source = u64::MAX - 2;
        let origin = action_plan::PlanOrigin {
            batch: BatchId(100),
            stage: Stage::Output,
            start: MonoTime(100),
            device,
            source: SourceToken(100),
            config: config.clone(),
        };
        let actions = [KeyId::new(7, 4).into(), KeyId::new(7, 5).into()];
        let plan = action_plan::plan_actions(&actions, origin, false).unwrap();
        actor.prepare(plan).unwrap().abort();
        assert_eq!(actor.core.next_source, u64::MAX - 2);
        assert_eq!(actor.last_batch, None);
        assert_eq!(actor.bindings.reserved_slots(), 0);
        let origin = action_plan::PlanOrigin {
            batch: BatchId(1),
            stage: Stage::Output,
            start: MonoTime(100),
            device,
            source: SourceToken(100),
            config,
        };
        let plan = action_plan::plan_actions(&actions, origin.clone(), false).unwrap();
        actor.prepare(plan).unwrap().commit();
        assert_eq!(actor.core.next_source, u64::MAX);
        assert_eq!(actor.last_batch, Some(BatchId(1)));
        let plan = action_plan::plan_actions(
            &actions[..1],
            action_plan::PlanOrigin {
                batch: BatchId(2),
                ..origin
            },
            false,
        )
        .unwrap();
        assert!(matches!(
            actor.prepare(plan),
            Err(ActorError::Engine(EngineError::ResourceLimit))
        ));
        assert_eq!(actor.core.next_source, u64::MAX);
        assert_eq!(actor.last_batch, Some(BatchId(1)));
        assert_eq!(actor.reserved_slots(), 2);
    }
}

#[cfg(test)]
mod repeat_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn repeat_target_requires_its_exact_source_visible_and_actually_emitted() {
        let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
        let mut actor = TerminalActor::new(config.clone()).unwrap();
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let key = KeyId::new(7, 4);
        let plan = action_plan::plan_actions(
            &[key.into()],
            action_plan::PlanOrigin {
                batch: BatchId(1),
                stage: Stage::Output,
                start: MonoTime(0),
                device,
                source: SourceToken(100),
                config,
            },
            false,
        )
        .unwrap();
        actor.enqueue(plan).unwrap();
        actor.poll(MonoTime(0)).unwrap();
        let Bound::Leaf(owned) = actor
            .bindings
            .live(action_plan::ActionSlot {
                batch: BatchId(1),
                index: 0,
            })
            .unwrap()
        else {
            panic!("terminal leaf")
        };
        let source = owned.source;
        assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key));
        // Private synthetic evidence changes; every query remains read-only.
        actor.core.owners.get_mut(&key).unwrap().remove(&source);
        actor
            .core
            .owners
            .get_mut(&key)
            .unwrap()
            .insert(SourceToken(source.0 + 1));
        assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
        actor.core.owners.get_mut(&key).unwrap().insert(source);
        actor.core.hidden.insert((key, source), 1);
        assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
        actor.core.hidden.remove(&(key, source));
        actor.core.emitted.remove(&key);
        assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
        actor.core.emitted.insert(key);
        assert_eq!(actor.physical_repeat_target(BatchId(1)), Some(key));
        actor.failed = true;
        assert_eq!(actor.physical_repeat_target(BatchId(1)), None);
    }
}

#[cfg(test)]
mod source_domain_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn partitioned_slot_and_snapshot_sources_stay_below_origin_domain_and_abort_preserves_cursor() {
        use crate::{modifier_scope::ScopedActionPlan, rule_selection::*};
        let key = |name: &str| name.parse::<KeyId>().unwrap();
        let boundary = crate::source_identity::ORIGIN_DOMAIN_START;
        let config = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "source-domains".into(),
            simple: vec![],
            complex: vec![Mapping {
                id: "a".into(),
                from: key("a"),
                to: vec![],
                allow_extra_modifiers: true,
                extras: MappingExtras {
                    if_alone: vec![Action {
                        hold_down_ns: 10,
                        modifiers: vec![key("left_alt")],
                        ..key("x").into()
                    }],
                    ..Default::default()
                },
            }],
        });
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        let selection = select_rule(
            config.clone(),
            MappingStage::Complex,
            key("a"),
            BTreeSet::from([key("left_shift"), key("left_control")]),
            |_| Truth::True,
        )
        .unwrap();
        let branch = selection
            .select_branch(RuleBranch::Alone, false, |_| Truth::True)
            .unwrap();
        let plan = || {
            ScopedActionPlan::branch(
                &branch,
                BatchId(1),
                MonoTime(100),
                device,
                SourceToken(boundary | 1),
            )
            .unwrap()
        };
        let mut actor = TerminalActor::new_with_source_limit(config.clone(), boundary).unwrap();
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        actor.core.next_source = boundary - 4; // two action slots + two snapshot owners
        actor.prepare_scoped(plan()).unwrap().abort();
        assert_eq!(actor.core.next_source, boundary - 4);
        assert!(actor.core.owners.is_empty());
        assert_eq!(actor.last_batch, None);
        actor.prepare_scoped(plan()).unwrap().commit();
        assert_eq!(actor.core.next_source, boundary);
        while actor.poll(MonoTime(100)).unwrap().is_some() {}
        let sources: BTreeSet<_> = actor
            .core
            .owners
            .values()
            .flat_map(|sources| sources.iter().copied())
            .collect();
        assert_eq!(sources.len(), 4);
        assert!(sources.iter().all(|source| source.0 < boundary));
        assert!(!sources.contains(&SourceToken(boundary | 1)));
        let extra = action_plan::plan_actions(
            &[key("z").into()],
            action_plan::PlanOrigin {
                batch: BatchId(2),
                stage: Stage::Output,
                start: MonoTime(100),
                device,
                source: SourceToken(boundary | 2),
                config,
            },
            false,
        )
        .unwrap();
        assert!(matches!(
            actor.prepare(extra),
            Err(ActorError::Engine(EngineError::ResourceLimit))
        ));
        assert_eq!(actor.core.next_source, boundary);
        assert_eq!(actor.last_batch, Some(BatchId(1)));
        actor.reset(MonoTime(100)).unwrap();
        assert_eq!(actor.core.next_source, boundary);
        assert_eq!(actor.max_source_end, boundary);
    }
}
