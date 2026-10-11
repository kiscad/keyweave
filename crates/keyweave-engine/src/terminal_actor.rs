//! Execute planned terminal work on one actor, saving real bindings per slot.
//! This is not yet the simple/complex matching actor or a platform runtime.
use crate::{
    action_plan::{ActionPlan, BatchId},
    timeline::{Stage, StageTimeline, TimelineError},
    *,
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
}

/// Allows at most one pending down and one cleanup up per distinct key on control events.
pub const MAX_ACTOR_KEYS: usize = MAX_OUTPUTS_PER_EVENT / 2;

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

enum TerminalWork {
    Action(action_plan::PlannedAction),
    ScopeUp(BatchId),
}
impl TerminalWork {
    fn batch(&self) -> BatchId {
        match self {
            Self::Action(action) => action.slot.batch,
            Self::ScopeUp(id) => *id,
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
    failed: bool,
}
impl TerminalActor {
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, ActorError> {
        Ok(Self {
            core: Engine::new_inner(config, true)?,
            timeline: StageTimeline::new(),
            batches: BTreeMap::new(),
            bindings: stage_bindings::BindingArena::default(),
            last_batch: None,
            scope_slots: 0,
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
    pub fn pressed_outputs(&self) -> Vec<KeyId> {
        self.core.emitted.iter().copied().collect()
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
        if removed || canceled {
            self.core.sync_modifiers(&mut outputs);
        }
        Ok(outputs)
    }
    /// Atomic admission; source ranges are reserved before any planned down runs.
    pub fn enqueue(&mut self, plan: ActionPlan) -> Result<(), ActorError> {
        self.enqueue_inner(plan, None)
    }
    /// Scope must describe this frozen plan. Scope owners are opened on its first
    /// delivered event, not on admission; empty plans never open a scope.
    pub fn enqueue_scoped(
        &mut self,
        plan: modifier_scope::ScopedActionPlan,
    ) -> Result<(), ActorError> {
        let (plan, scope) = plan.into_parts();
        self.enqueue_inner(plan, Some(scope))
    }
    fn enqueue_inner(
        &mut self,
        mut plan: ActionPlan,
        scope: Option<modifier_scope::ModifierScope>,
    ) -> Result<(), ActorError> {
        if self.failed {
            return Err(ActorError::Failed);
        }
        if plan.origin.stage != Stage::Output {
            return Err(ActorError::UnsupportedStage);
        }
        if !Arc::ptr_eq(&plan.origin.config, &self.core.config) {
            return Err(ActorError::InvalidSnapshot);
        }
        if self.last_batch.is_some_and(|id| plan.origin.batch <= id) {
            return Err(ActorError::InvalidBatch);
        }
        let slots = plan
            .events
            .iter()
            .filter(|e| e.payload.edge == KeyEdge::Down)
            .count();
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
            .ok_or(EngineError::ResourceLimit)?;
        let scope_events = plan.events.len();
        let up_queued = plan.release_on_input_up.is_empty() && !scope_up;
        let events = std::mem::take(&mut plan.events)
            .into_iter()
            .map(action_event)
            .collect();
        let reservation = if remaining > 0 {
            Some(
                self.bindings
                    .prepare(plan.origin.clone(), slots, None)
                    .map_err(|_| ActorError::InvalidSlot)?,
            )
        } else {
            None
        };
        self.timeline
            .enqueue(events)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?;
        if let Some(reservation) = reservation {
            reservation.commit();
        }
        self.core.next_source = next_source;
        self.last_batch = Some(plan.origin.batch);
        if remaining > 0 {
            self.scope_slots += scope_reserved;
            let scope = scope.map(|spec| BatchScope {
                until_input_up: spec.until_input_up(),
                pending: Some(spec),
                active: None,
                remaining: scope_events,
                source: scope_source,
                reserved: scope_reserved,
            });
            self.batches.insert(
                plan.origin.batch,
                Batch {
                    plan,
                    base_source,
                    remaining,
                    up_queued,
                    keys: batch_keys,
                    scope,
                },
            );
        }
        Ok(())
    }
    /// Unknown/completed or duplicate original up is a no-op, not a new release.
    pub fn input_up(&mut self, batch: BatchId, at: MonoTime) -> Result<bool, ActorError> {
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
        let Some(saved) = self.batches.get(&batch) else {
            return Ok(false);
        };
        if saved.up_queued {
            return Ok(false);
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
        self.timeline
            .enqueue(events)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?;
        if let Some(saved) = self.batches.get_mut(&batch) {
            saved.up_queued = true;
        }
        self.advance(at)?;
        Ok(true)
    }
    /// Execute one due stage event. Late delivery uses actual actor time.
    pub fn poll(&mut self, at: MonoTime) -> Result<Option<ActorStep>, ActorError> {
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
        let work = due.event.payload;
        let id = work.batch();
        let action = match work {
            TerminalWork::Action(action) => Some(action),
            TerminalWork::ScopeUp(_) => None,
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
                    let bound = self
                        .bindings
                        .take(action.slot)
                        .map_err(|_| ActorError::InvalidSlot)?;
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
            if !self
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
        Ok(Some(ActorStep {
            outputs,
            halt_requested: action.is_some_and(|action| action.halt),
            completed_batch,
        }))
    }
    pub fn cancel_batch(
        &mut self,
        id: BatchId,
        at: MonoTime,
    ) -> Result<Vec<OutputIntent>, ActorError> {
        self.advance(at)?;
        if !self.batches.contains_key(&id) {
            return Ok(Vec::new());
        }
        let mut outputs = Vec::new();
        self.cancel(id, &mut outputs)?;
        self.core.sync_modifiers(&mut outputs);
        Ok(outputs)
    }
    fn cancel(&mut self, id: BatchId, outputs: &mut Vec<OutputIntent>) -> Result<(), ActorError> {
        if !self.batches.contains_key(&id) {
            return Ok(());
        }
        let bounds = self
            .bindings
            .cancel_subtree(id)
            .map_err(|_| ActorError::InvalidSlot)?;
        self.timeline
            .discard_where(|event| event.payload.batch() == id);
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
        self.batches.clear();
        self.bindings = stage_bindings::BindingArena::default();
        self.scope_slots = 0;
        self.failed = false;
        Ok(self.core.reset(at)?)
    }
}
