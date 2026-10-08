//! Execute planned terminal work on one actor, saving real bindings per slot.
//! This is not yet the simple/complex matching actor or a platform runtime.
use crate::{
    action_plan::{ActionPlan, ActionSlot, BatchId},
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
}

/// Allows at most one pending down and one cleanup up per distinct key on control events.
pub const MAX_ACTOR_KEYS: usize = MAX_OUTPUTS_PER_EVENT / 2;

struct Batch {
    plan: ActionPlan,
    base_source: u64,
    slots: usize,
    remaining: usize,
    up_queued: bool,
    keys: BTreeSet<KeyId>,
}
#[derive(Debug)]
pub struct ActorStep {
    pub outputs: Vec<OutputIntent>,
    /// The caller propagates this to the original rule binding's halt state.
    pub halt_requested: bool,
    pub completed_batch: Option<BatchId>,
}

pub struct TerminalActor {
    core: Engine,
    timeline: StageTimeline<action_plan::PlannedAction>,
    batches: BTreeMap<BatchId, Batch>,
    slots: BTreeMap<ActionSlot, Bound>,
    reserved_slots: usize,
    last_batch: Option<BatchId>,
}
impl TerminalActor {
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, ActorError> {
        Ok(Self {
            core: Engine::new_inner(config, true)?,
            timeline: StageTimeline::new(),
            batches: BTreeMap::new(),
            slots: BTreeMap::new(),
            reserved_slots: 0,
            last_batch: None,
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
        self.slots.len()
    }
    pub fn pending_batches(&self) -> usize {
        self.batches.len()
    }
    pub fn reserved_slots(&self) -> usize {
        self.reserved_slots
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
            self.cancel(id, &mut outputs);
        }
        if removed || canceled {
            self.core.sync_modifiers(&mut outputs);
        }
        Ok(outputs)
    }
    /// Atomic admission; source ranges are reserved before any planned down runs.
    pub fn enqueue(&mut self, mut plan: ActionPlan) -> Result<(), ActorError> {
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
        if self.batches.len() >= MAX_BINDINGS
            || self.reserved_slots.saturating_add(slots) > MAX_OUTPUT_OWNERS
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
        if keys.len() > MAX_ACTOR_KEYS {
            return Err(ActorError::Capacity);
        }
        let base_source = self.core.next_source;
        let next_source = base_source
            .checked_add(slots as u64)
            .ok_or(EngineError::ResourceLimit)?;
        let remaining = plan.events.len() + plan.release_on_input_up.len();
        let up_queued = plan.release_on_input_up.is_empty();
        let events = std::mem::take(&mut plan.events);
        self.timeline
            .enqueue(events)
            .map_err(|rejected| ActorError::Timeline(rejected.error))?;
        self.core.next_source = next_source;
        self.last_batch = Some(plan.origin.batch);
        if remaining > 0 {
            self.reserved_slots += slots;
            self.batches.insert(
                plan.origin.batch,
                Batch {
                    plan,
                    base_source,
                    slots,
                    remaining,
                    up_queued,
                    keys: batch_keys,
                },
            );
        }
        Ok(())
    }
    /// Unknown/completed or duplicate original up is a no-op, not a new release.
    pub fn input_up(&mut self, batch: BatchId, at: MonoTime) -> Result<bool, ActorError> {
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
        self.timeline
            .enqueue(saved.plan.input_up_events(at))
            .map_err(|rejected| ActorError::Timeline(rejected.error))?;
        if let Some(saved) = self.batches.get_mut(&batch) {
            saved.up_queued = true;
        }
        self.advance(at)?;
        Ok(true)
    }
    /// Execute one due stage event. Late delivery uses actual actor time.
    pub fn poll(&mut self, at: MonoTime) -> Result<Option<ActorStep>, ActorError> {
        self.check_time(at)?;
        let Some(due) = self.timeline.pop_due(at)? else {
            self.core.now = at;
            return Ok(None);
        };
        self.core.now = at;
        self.core.current_device = due.event.device;
        let action = due.event.payload;
        let id = action.slot.batch;
        let batch = self.batches.get(&id).ok_or(ActorError::InvalidSlot)?;
        let config = Arc::clone(&batch.plan.origin.config);
        let mut cursor = batch.base_source + u64::from(action.slot.index);
        let mut outputs = Vec::new();
        match action.edge {
            KeyEdge::Down => {
                if self.slots.contains_key(&action.slot) {
                    return Err(ActorError::InvalidSlot);
                }
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
                self.slots.insert(action.slot, bound);
            }
            KeyEdge::Up => {
                let bound = self
                    .slots
                    .remove(&action.slot)
                    .ok_or(ActorError::InvalidSlot)?;
                self.core
                    .release_bound(bound, &config, &mut cursor, false, &mut outputs);
            }
            KeyEdge::Repeat => return Err(ActorError::InvalidSlot),
        }
        let batch = self.batches.get_mut(&id).ok_or(ActorError::InvalidSlot)?;
        batch.remaining -= 1;
        let completed_batch = if batch.remaining == 0 {
            let batch = self.batches.remove(&id).ok_or(ActorError::InvalidSlot)?;
            self.reserved_slots -= batch.slots;
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
            halt_requested: action.halt,
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
        self.cancel(id, &mut outputs);
        self.core.sync_modifiers(&mut outputs);
        Ok(outputs)
    }
    fn cancel(&mut self, id: BatchId, outputs: &mut Vec<OutputIntent>) {
        self.timeline
            .discard_where(|event| event.payload.slot.batch == id);
        let Some(batch) = self.batches.remove(&id) else {
            return;
        };
        let slots: Vec<_> = self
            .slots
            .keys()
            .rev()
            .filter(|slot| slot.batch == id)
            .copied()
            .collect();
        for slot in slots {
            if let Some(bound) = self.slots.remove(&slot) {
                let mut cursor = batch.base_source;
                self.core.release_bound(
                    bound,
                    &batch.plan.origin.config,
                    &mut cursor,
                    false,
                    outputs,
                );
            }
        }
        self.reserved_slots -= batch.slots;
    }
    pub fn reset(&mut self, at: MonoTime) -> Result<Vec<OutputIntent>, ActorError> {
        self.check_time(at)?;
        self.timeline.reset(at)?;
        self.batches.clear();
        self.slots.clear();
        self.reserved_slots = 0;
        Ok(self.core.reset(at)?)
    }
}
