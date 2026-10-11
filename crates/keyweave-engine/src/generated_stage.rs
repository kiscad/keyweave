//! Destination-queue logical state changes at generation, not timed delivery.
//! This queue is a building block; binding execution and modifier scopes are separate.
use crate::{
    action_plan::{ActionSlot, PlannedAction},
    rule_selection::{self, MappingStage, RuleSelection},
    timeline::{DueEvent, MAX_STAGE_APPEND, Stage, StageEvent, StageTimeline, TimelineError},
    *,
};

pub use crate::stage_delivery::DeliveryProgress;
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GeneratedError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Timeline(#[from] TimelineError),
    #[error("event belongs to a different destination stage")]
    Stage,
    #[error("invalid generated slot lifecycle")]
    Lifecycle,
    #[error("generated event has different saved provenance")]
    Provenance,
    #[error(transparent)]
    Binding(#[from] stage_bindings::BindingError),
}
pub struct RejectedGeneratedBatch {
    pub error: GeneratedError,
    pub events: Vec<StageEvent<PlannedAction>>,
}
impl std::fmt::Debug for RejectedGeneratedBatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedGeneratedBatch")
            .field("error", &self.error)
            .field("event_count", &self.events.len())
            .finish()
    }
}
struct Down {
    device: DeviceInstanceId,
    source: SourceToken,
    config: Arc<ConfigSnapshot>,
    target: ActionTarget,
}
struct LogicalScope {
    hidden: Vec<ActionSlot>,
    device: DeviceInstanceId,
}
/// A single queue's admitted original-up recipe. Not a native permission or owner.
pub struct ScopedGeneration {
    queue: Arc<()>,
    plan: action_plan::ActionPlan,
    up_generated: bool,
}
struct Environment {
    stage: Stage,
    config: Arc<ConfigSnapshot>,
    down: BTreeMap<ActionSlot, Down>,
    variables: BTreeMap<String, Option<VariableValue>>,
    hidden: BTreeMap<ActionSlot, usize>,
    scopes: BTreeMap<action_plan::BatchId, LogicalScope>,
    references: usize,
}
struct Append<'a> {
    environment: &'a mut Environment,
    down: BTreeMap<ActionSlot, Option<Down>>,
    variables: BTreeMap<String, Option<VariableValue>>,
}
impl<'a> Append<'a> {
    fn commit(self) -> &'a mut Environment {
        for (slot, down) in self.down {
            match down {
                Some(down) => {
                    self.environment.down.insert(slot, down);
                }
                None => {
                    self.environment.down.remove(&slot);
                }
            }
        }
        for (name, value) in self.variables {
            self.environment.variables.insert(name, value);
        }
        self.environment
    }
}
/// A validated generation transaction. Drop/abort has no logical or queue effects.
#[must_use = "commit or abort the prepared generation"]
pub struct PreparedGeneratedAppend<'a> {
    append: Append<'a>,
    delivery: stage_delivery::DeliveryAppend<'a>,
    timeline: timeline::PreparedStageAppend<'a, PlannedAction>,
    scope: Option<(action_plan::BatchId, LogicalScope)>,
}
impl PreparedGeneratedAppend<'_> {
    pub fn abort(self) -> Vec<StageEvent<PlannedAction>> {
        self.timeline.abort()
    }
    pub fn commit(self) {
        self.timeline.commit();
        self.delivery.commit();
        let environment = self.append.commit();
        if let Some((id, scope)) = self.scope {
            for slot in &scope.hidden {
                *environment.hidden.entry(*slot).or_default() += 1;
            }
            environment.references += scope.hidden.len();
            environment.scopes.insert(id, scope);
        }
    }
}
/// Only commit creates a usable original-Up recipe for this queue.
#[must_use = "commit or discard the prepared scoped generation"]
pub struct PreparedScopedGeneration<'a> {
    append: PreparedGeneratedAppend<'a>,
    saved: ScopedGeneration,
}
impl PreparedScopedGeneration<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) -> ScopedGeneration {
        self.append.commit();
        self.saved
    }
}
/// Atomic Simple-parent registration and Complex destination generation.
/// No actual producer binding or downstream execution is implied by this guard.
#[must_use = "commit or discard the child registration and generation together"]
pub struct PreparedGeneratedChild<'a, T> {
    generated: PreparedScopedGeneration<'a>,
    registration: stage_bindings::Reservation<'a, T>,
}
impl<T> PreparedGeneratedChild<'_, T> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) -> ScopedGeneration {
        self.registration.commit();
        self.generated.commit()
    }
}
/// Saved original-Up admission. Drop leaves scope, variables, recipe and clock unchanged.
#[must_use = "commit or discard the prepared original Up"]
pub struct PreparedGeneratedUp<'a> {
    append: Append<'a>,
    delivery: stage_delivery::DeliveryAppend<'a>,
    timeline: timeline::PreparedStageAppend<'a, PlannedAction>,
    saved: &'a mut ScopedGeneration,
    now: &'a mut MonoTime,
    at: MonoTime,
}
impl PreparedGeneratedUp<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.timeline.commit();
        self.delivery.commit();
        self.append
            .commit()
            .close_scope(self.saved.plan.origin.batch);
        *self.now = self.at;
        self.saved.up_generated = true;
    }
}
fn variable_update(target: &ActionTarget, edge: KeyEdge) -> Option<(&str, Option<VariableValue>)> {
    let ActionTarget::SetVariable(variable) = target else {
        return None;
    };
    match (&variable.operation, edge) {
        (
            VariableOperation::Set {
                value: Some(value), ..
            },
            KeyEdge::Down,
        ) => Some((&variable.name, Some(value.clone()))),
        (
            VariableOperation::Set {
                key_up_value: Some(value),
                ..
            },
            KeyEdge::Up,
        ) => Some((&variable.name, Some(value.clone()))),
        (VariableOperation::Unset, KeyEdge::Down) => Some((&variable.name, None)),
        _ => None,
    }
}
impl Environment {
    fn prepare(
        &mut self,
        events: &[StageEvent<PlannedAction>],
        temporary: usize,
        child_config: Option<&Arc<ConfigSnapshot>>,
    ) -> Result<Append<'_>, GeneratedError> {
        if events.len() > MAX_STAGE_APPEND {
            return Err(TimelineError::Capacity.into());
        }
        let mut down = BTreeMap::<ActionSlot, Option<Down>>::new();
        let mut variables = BTreeMap::new();
        let mut count = self.down.len();
        if count.saturating_add(temporary) > MAX_OUTPUT_OWNERS {
            return Err(EngineError::ResourceLimit.into());
        }
        for event in events {
            if event.stage != self.stage {
                return Err(GeneratedError::Stage);
            }
            let action = &event.payload;
            let previous = match down.get(&action.slot) {
                Some(value) => value.as_ref(),
                None => self.down.get(&action.slot),
            };
            match action.edge {
                KeyEdge::Down => {
                    if previous.is_some()
                        || down.contains_key(&action.slot)
                        || self.hidden.contains_key(&action.slot)
                        || self.scopes.contains_key(&action.slot.batch)
                    {
                        return Err(GeneratedError::Lifecycle);
                    }
                    if !Arc::ptr_eq(&event.config, &self.config)
                        && !child_config.is_some_and(|config| Arc::ptr_eq(config, &event.config))
                    {
                        return Err(GeneratedError::Provenance);
                    }
                    if let ActionTarget::SetVariable(variable) = &action.target {
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
                        if !valid || !self.variables.contains_key(&variable.name) {
                            return Err(EngineError::InvalidVariable.into());
                        }
                    }
                    count += 1;
                    if count.saturating_add(temporary) > MAX_OUTPUT_OWNERS {
                        return Err(EngineError::ResourceLimit.into());
                    }
                    down.insert(
                        action.slot,
                        Some(Down {
                            device: event.device,
                            source: event.source,
                            config: Arc::clone(&event.config),
                            target: action.target.clone(),
                        }),
                    );
                }
                KeyEdge::Up => {
                    let previous = previous.ok_or(GeneratedError::Lifecycle)?;
                    if previous.device != event.device
                        || previous.source != event.source
                        || !Arc::ptr_eq(&previous.config, &event.config)
                        || previous.target != action.target
                    {
                        return Err(GeneratedError::Provenance);
                    }
                    count -= 1;
                    down.insert(action.slot, None);
                }
                KeyEdge::Repeat => return Err(GeneratedError::Lifecycle),
            }
            if let Some((name, value)) = variable_update(&action.target, action.edge) {
                variables.insert(name.to_owned(), value);
            }
        }
        Ok(Append {
            environment: self,
            down,
            variables,
        })
    }
    fn close_scope(&mut self, batch: action_plan::BatchId) {
        if let Some(scope) = self.scopes.remove(&batch) {
            self.references -= scope.hidden.len();
            for slot in scope.hidden {
                if let Some(count) = self.hidden.get_mut(&slot) {
                    *count -= 1;
                    if *count == 0 {
                        self.hidden.remove(&slot);
                    }
                }
            }
        }
    }
    fn remove(&mut self, predicate: impl Fn(&Down, ActionSlot) -> bool) -> Vec<ActionSlot> {
        let slots: Vec<_> = self
            .down
            .iter()
            .filter(|(slot, down)| predicate(down, **slot))
            .map(|(slot, _)| *slot)
            .collect();
        for slot in slots.iter().rev() {
            if let Some(down) = self.down.remove(slot)
                && let Some((name, value)) = variable_update(&down.target, KeyEdge::Up)
            {
                self.variables.insert(name.to_owned(), value);
            }
        }
        slots
    }
}
/// One destination stage's bounded causal queue and generated logical environment.
/// Input/OS execution, scopes, source allocation and original-up recipes remain caller-owned.
pub struct GeneratedStageQueue {
    environment: Environment,
    timeline: StageTimeline<PlannedAction>,
    identity: Arc<()>,
    now: MonoTime,
    delivery: crate::stage_delivery::DeliveryLedger,
}
impl GeneratedStageQueue {
    pub fn new(stage: Stage, config: Arc<ConfigSnapshot>) -> Result<Self, GeneratedError> {
        config.validate_timed_stages(true)?;
        let variables = config
            .variable_names()?
            .into_iter()
            .map(|name| (name, None))
            .collect();
        Ok(Self {
            environment: Environment {
                stage,
                config,
                down: BTreeMap::new(),
                variables,
                hidden: BTreeMap::new(),
                scopes: BTreeMap::new(),
                references: 0,
            },
            timeline: StageTimeline::new(),
            identity: Arc::new(()),
            now: MonoTime(0),
            delivery: crate::stage_delivery::DeliveryLedger::default(),
        })
    }
    pub(crate) fn advance(&mut self, at: MonoTime) -> Result<(), GeneratedError> {
        self.timeline.advance(at)?;
        self.now = at;
        Ok(())
    }
    pub(crate) fn check_device(
        &self,
        device: DeviceInstanceId,
        removed: bool,
    ) -> Result<(), GeneratedError> {
        Ok(self.timeline.check_device(device, removed)?)
    }
    pub fn reserved_slots(&self) -> usize {
        self.delivery.reserved()
    }
    pub fn pending_batches(&self) -> usize {
        self.delivery.batches()
    }
    pub fn delivery_progress(&self, batch: action_plan::BatchId) -> Option<DeliveryProgress> {
        self.delivery.progress(batch)
    }
    /// Call only after downstream bindings/actual scopes are complete. This
    /// checks queue delivery, not platform acceptance or downstream recovery.
    pub fn retire_batch(&mut self, batch: action_plan::BatchId) -> Result<(), GeneratedError> {
        if self.environment.scopes.contains_key(&batch) {
            return Err(GeneratedError::Lifecycle);
        }
        self.delivery.retire(batch)
    }
    pub fn queued_events(&self) -> usize {
        self.timeline.len()
    }
    pub fn logical_slots(&self) -> usize {
        self.environment.down.len()
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.timeline.next_deadline()
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        &self.environment.config
    }
    pub fn modifiers(&self) -> BTreeSet<KeyId> {
        self.environment
            .down
            .iter()
            .filter(|(slot, _)| !self.environment.hidden.contains_key(slot))
            .filter_map(|(_, down)| match down.target {
                ActionTarget::Key(key) if key.is_modifier() => Some(key),
                _ => None,
            })
            .collect()
    }
    pub fn variable_values(&self) -> BTreeMap<String, VariableValue> {
        self.environment
            .variables
            .iter()
            .filter_map(|(name, value)| value.clone().map(|value| (name.clone(), value)))
            .collect()
    }
    pub fn condition_truth(
        &self,
        condition: &Condition,
        context: &ContextSnapshot,
        device: DeviceInstanceId,
        at: MonoTime,
    ) -> Truth {
        variable_truth(&self.environment.variables, condition)
            .unwrap_or_else(|| condition.evaluate(context, device, at))
    }
    /// Select the manipulator writing into this queue, using generated state.
    pub fn select(
        &self,
        key: KeyId,
        context: &ContextSnapshot,
        device: DeviceInstanceId,
        at: MonoTime,
    ) -> Result<Option<RuleSelection>, GeneratedError> {
        let stage = match self.environment.stage {
            Stage::Complex => MappingStage::Simple,
            Stage::Output => MappingStage::Complex,
            Stage::Simple => return Err(GeneratedError::Stage),
        };
        Ok(rule_selection::select_rule(
            Arc::clone(self.snapshot()),
            stage,
            key,
            self.modifiers(),
            |condition| self.condition_truth(condition, context, device, at),
        ))
    }
    /// Prepare all logical effects before queue admission; commit only on success.
    pub fn enqueue(
        &mut self,
        events: Vec<StageEvent<PlannedAction>>,
    ) -> Result<(), RejectedGeneratedBatch> {
        self.prepare_enqueue(events)?.commit();
        Ok(())
    }
    /// Hold all validated logical/delivery/timeline changes until explicit commit.
    pub fn prepare_enqueue(
        &mut self,
        events: Vec<StageEvent<PlannedAction>>,
    ) -> Result<PreparedGeneratedAppend<'_>, RejectedGeneratedBatch> {
        let append = match self.environment.prepare(&events, 0, None) {
            Ok(append) => append,
            Err(error) => return Err(RejectedGeneratedBatch { error, events }),
        };
        let delivery = match self.delivery.prepare(&events, None) {
            Ok(delivery) => delivery,
            Err(error) => return Err(RejectedGeneratedBatch { error, events }),
        };
        let timeline =
            self.timeline
                .prepare_enqueue(events)
                .map_err(|rejected| RejectedGeneratedBatch {
                    error: rejected.error.into(),
                    events: rejected.events,
                })?;
        Ok(PreparedGeneratedAppend {
            append,
            delivery,
            timeline,
            scope: None,
        })
    }
    pub fn hidden_modifier_references(&self) -> usize {
        self.environment.references
    }
    /// Frozen scopes change logical visibility at generation. The original plan
    /// remains caller-owned for execution with its separate actual scope.
    pub fn enqueue_scoped(
        &mut self,
        plan: &modifier_scope::ScopedActionPlan,
    ) -> Result<ScopedGeneration, GeneratedError> {
        Ok(self.prepare_scoped(plan)?.commit())
    }
    /// The borrowed plan remains available for a separate execution admission.
    /// Abort produces no original-Up credential or persistent scope references.
    pub fn prepare_scoped(
        &mut self,
        plan: &modifier_scope::ScopedActionPlan,
    ) -> Result<PreparedScopedGeneration<'_>, GeneratedError> {
        self.prepare_scoped_inner(plan, None)
    }
    /// Generate a Complex child from an exclusively retained or jointly activated Simple parent.
    /// This preserves the parent's old Arc without relaxing ordinary root admission.
    /// Snapshot extra sources are charged here; do not also charge the same scope.
    /// Actual child/scopes must finish before the caller clears external registration.
    pub fn prepare_complex_child_scoped<'a, T>(
        &'a mut self,
        plan: &modifier_scope::ScopedActionPlan,
        registration: stage_bindings::Reservation<'a, T>,
    ) -> Result<PreparedGeneratedChild<'a, T>, GeneratedError> {
        if self.environment.stage != Stage::Complex {
            return Err(GeneratedError::Stage);
        }
        let extra = if plan.actions().events().is_empty() {
            0
        } else {
            plan.scope().reservation()
        };
        let registration = registration.with_extra_sources(extra)?;
        let child = registration.child_admission()?;
        let generated = self.prepare_child_scoped(plan, &child)?;
        Ok(PreparedGeneratedChild {
            generated,
            registration,
        })
    }
    pub(crate) fn prepare_child_scoped(
        &mut self,
        plan: &modifier_scope::ScopedActionPlan,
        child: &stage_bindings::ChildAdmission<'_>,
    ) -> Result<PreparedScopedGeneration<'_>, GeneratedError> {
        self.prepare_scoped_inner(plan, Some(child))
    }
    fn prepare_scoped_inner(
        &mut self,
        plan: &modifier_scope::ScopedActionPlan,
        child: Option<&stage_bindings::ChildAdmission<'_>>,
    ) -> Result<PreparedScopedGeneration<'_>, GeneratedError> {
        let actions = plan.actions();
        let spec = plan.scope();
        if actions.origin.stage != self.environment.stage {
            return Err(GeneratedError::Stage);
        }
        let slots = actions
            .events()
            .iter()
            .filter(|event| event.payload.edge == KeyEdge::Down)
            .count();
        if let Some(child) = child {
            if !child.matches(&actions.origin, slots)
                || (actions.origin.config.generation >= self.environment.config.generation
                    && !Arc::ptr_eq(&actions.origin.config, &self.environment.config))
            {
                return Err(GeneratedError::Provenance);
            }
            actions.origin.config.validate_timed_stages(true)?;
            actions.origin.config.variable_names()?;
        } else if !Arc::ptr_eq(&actions.origin.config, &self.environment.config) {
            return Err(GeneratedError::Provenance);
        }
        let nonempty = !actions.events().is_empty();
        if self.environment.scopes.contains_key(&actions.origin.batch) {
            return Err(GeneratedError::Lifecycle);
        }
        let persistent = nonempty && (spec.until_input_up() || !actions.releases().is_empty());
        if persistent && self.environment.scopes.len() >= MAX_BINDINGS {
            return Err(EngineError::ResourceLimit.into());
        }
        let mut hidden = Vec::new();
        let mut transient = 0usize;
        if nonempty {
            for (slot, down) in &self.environment.down {
                if let ActionTarget::Key(key) = down.target {
                    if spec.consumed().contains(&key) {
                        hidden.push(*slot);
                    } else if key.is_modifier()
                        && spec.policy()
                            == rule_selection::BranchModifierPolicy::DownSnapshotMinusConsumed
                        && !spec.snapshot().contains(&key)
                    {
                        transient += 1;
                    }
                }
            }
            if spec.policy() == rule_selection::BranchModifierPolicy::DownSnapshotMinusConsumed {
                transient += spec.consumed().len();
            }
        }
        if hidden.len().saturating_add(transient)
            > MAX_PERSISTENT_SUPPRESSION_REFERENCES.saturating_sub(self.environment.references)
        {
            return Err(EngineError::ResourceLimit.into());
        }
        let temporary = if nonempty { spec.reservation() } else { 0 };
        let append = self.environment.prepare(
            actions.events(),
            temporary,
            child.map(|_| &actions.origin.config),
        )?;
        let delivery = self.delivery.prepare(
            actions.events(),
            if nonempty {
                Some((actions.origin.batch, temporary))
            } else {
                None
            },
        )?;
        let timeline = self
            .timeline
            .prepare_enqueue(actions.events().to_vec())
            .map_err(|rejected| GeneratedError::Timeline(rejected.error))?;
        let scope = if persistent {
            if !spec.until_input_up() {
                hidden.clear();
            }
            Some((
                actions.origin.batch,
                LogicalScope {
                    hidden,
                    device: actions.origin.device,
                },
            ))
        } else {
            None
        };
        Ok(PreparedScopedGeneration {
            append: PreparedGeneratedAppend {
                append,
                delivery,
                timeline,
                scope,
            },
            saved: ScopedGeneration {
                queue: Arc::clone(&self.identity),
                plan: actions.clone(),
                up_generated: !persistent,
            },
        })
    }
    /// Append saved Up once, then restore logical scope even if timed delivery
    /// is still pending. Failure leaves recipe, scope, environment and queue unchanged.
    pub fn input_up_scoped(
        &mut self,
        saved: &mut ScopedGeneration,
        at: MonoTime,
    ) -> Result<bool, GeneratedError> {
        let Some(prepared) = self.prepare_input_up_scoped(saved, at)? else {
            return Ok(false);
        };
        prepared.commit();
        Ok(true)
    }
    /// Preflight saved Up without closing its logical scope or consuming its recipe.
    pub fn prepare_input_up_scoped<'a>(
        &'a mut self,
        saved: &'a mut ScopedGeneration,
        at: MonoTime,
    ) -> Result<Option<PreparedGeneratedUp<'a>>, GeneratedError> {
        if !Arc::ptr_eq(&saved.queue, &self.identity) {
            return Err(GeneratedError::Provenance);
        }
        if at < self.now {
            return Err(TimelineError::TimeReversed.into());
        }
        if saved.up_generated {
            return Ok(None);
        }
        if !self
            .environment
            .scopes
            .contains_key(&saved.plan.origin.batch)
        {
            return Err(GeneratedError::Lifecycle);
        }
        let events = saved.plan.input_up_events(at);
        let append = self.environment.prepare(&events, 0, None)?;
        let delivery = self.delivery.prepare(&events, None)?;
        let timeline = self
            .timeline
            .prepare_enqueue_at(events, at)
            .map_err(|rejected| GeneratedError::Timeline(rejected.error))?;
        Ok(Some(PreparedGeneratedUp {
            append,
            delivery,
            timeline,
            saved,
            now: &mut self.now,
            at,
        }))
    }
    /// Delivery does not replay logical assignments or modifier edges.
    pub fn poll(
        &mut self,
        at: MonoTime,
    ) -> Result<Option<DueEvent<PlannedAction>>, GeneratedError> {
        let due = self.timeline.pop_due(at)?;
        if let Some(due) = &due {
            self.delivery.delivered(&due.event);
        }
        self.now = at;
        Ok(due)
    }
    /// Timeline validates device history first. Returned slots/events require caller cleanup.
    pub fn observe_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
    ) -> Result<(Vec<ActionSlot>, Vec<StageEvent<PlannedAction>>), GeneratedError> {
        let events = self.timeline.observe_device(device, removed)?;
        self.delivery.remove_device(device, removed);
        let slots = self.environment.remove(|down, _| {
            down.device.id == device.id
                && (down.device.generation < device.generation
                    || (removed && down.device.generation == device.generation))
        });
        let scopes: Vec<_> = self
            .environment
            .scopes
            .iter()
            .filter(|(_, scope)| {
                scope.device.id == device.id
                    && (scope.device.generation < device.generation
                        || (removed && scope.device.generation == device.generation))
            })
            .map(|(batch, _)| *batch)
            .collect();
        for batch in scopes {
            self.environment.close_scope(batch);
        }
        Ok((slots, events))
    }
    pub fn cancel_batch(
        &mut self,
        batch: action_plan::BatchId,
    ) -> (Vec<ActionSlot>, Vec<StageEvent<PlannedAction>>) {
        let events = self
            .timeline
            .discard_where(|event| event.payload.slot.batch == batch);
        let slots = self.environment.remove(|_, slot| slot.batch == batch);
        self.environment.close_scope(batch);
        self.delivery.cancel(batch);
        (slots, events)
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), GeneratedError> {
        let names = self.check_activation(&config)?;
        self.commit_activation(config, names);
        Ok(())
    }
    pub(crate) fn check_activation(
        &self,
        config: &Arc<ConfigSnapshot>,
    ) -> Result<BTreeSet<String>, GeneratedError> {
        if config.generation <= self.environment.config.generation {
            return Err(EngineError::InvalidGeneration.into());
        }
        config.validate_timed_stages(true)?;
        let names = config.variable_names()?;
        if self
            .environment
            .variables
            .keys()
            .chain(names.iter())
            .collect::<BTreeSet<_>>()
            .len()
            > MAX_VARIABLES
        {
            return Err(EngineError::ResourceLimit.into());
        }
        Ok(names)
    }
    pub(crate) fn commit_activation(
        &mut self,
        config: Arc<ConfigSnapshot>,
        names: BTreeSet<String>,
    ) {
        for name in names {
            self.environment.variables.entry(name).or_insert(None);
        }
        self.environment.config = config;
    }
    pub fn reset(
        &mut self,
        at: MonoTime,
    ) -> Result<Vec<StageEvent<PlannedAction>>, GeneratedError> {
        let events = self.timeline.reset(at)?;
        self.now = at;
        self.delivery = crate::stage_delivery::DeliveryLedger::default();
        self.environment.down.clear();
        self.environment.scopes.clear();
        self.environment.hidden.clear();
        self.environment.references = 0;
        self.identity = Arc::new(());
        self.environment
            .variables
            .retain(|name, _| self.environment.config.references_variable(name));
        for value in self.environment.variables.values_mut() {
            *value = None;
        }
        Ok(events)
    }
}
