//! Deterministic remapping. No OS input, file IO, threads or wall clock.
pub mod action_plan;
mod conditions;
pub mod generated_stage;
pub mod modifier_scope;
mod modifiers;
pub mod rule_selection;
pub mod stage_bindings;
pub mod terminal_actor;
pub mod timeline;
pub use conditions::{Condition, InputSourceFilter, Pattern, Truth};
use keyweave_types::*;
pub use modifiers::{Modifier, ModifierFilter};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use thiserror::Error;

pub const MAX_BINDINGS: usize = 4096;
pub const MAX_OUTPUTS_PER_EVENT: usize = 1024;
pub const MAX_OUTPUT_OWNERS: usize = 16_384;
pub const MAX_DEVICE_IDENTITIES: usize = 4096;
pub const MAX_PERSISTENT_SUPPRESSION_REFERENCES: usize = 16_384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub target: ActionTarget,
    pub repeat: bool,
    pub lazy: bool,
    pub halt: bool,
    pub hold_down_ns: u64,
    pub modifiers: Vec<KeyId>,
    pub conditions: Vec<Condition>,
}
impl From<KeyId> for Action {
    fn from(key: KeyId) -> Self {
        Self {
            target: ActionTarget::Key(key),
            repeat: true,
            lazy: false,
            halt: false,
            hold_down_ns: 0,
            modifiers: Vec::new(),
            conditions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionTarget {
    Key(KeyId),
    SetVariable(VariableAction),
}
impl Action {
    pub fn key(&self) -> Option<KeyId> {
        if let ActionTarget::Key(key) = self.target {
            Some(key)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mapping {
    pub id: String,
    pub from: KeyId,
    pub to: Vec<Action>,
    pub allow_extra_modifiers: bool,
    pub extras: MappingExtras,
}

#[derive(Clone, Debug)]
pub struct DelayedAction {
    pub if_invoked: Vec<Action>,
    pub if_canceled: Vec<Action>,
    pub delay_ns: u64,
}
impl DelayedAction {
    fn enabled(&self) -> bool {
        !self.if_invoked.is_empty() || !self.if_canceled.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct MappingExtras {
    pub from_modifiers: ModifierFilter,
    pub conditions: Vec<Condition>,
    pub delayed: Option<DelayedAction>,
    pub if_held_down: Vec<Action>,
    pub held_threshold_ns: u64,
    pub if_alone: Vec<Action>,
    pub after_key_up: Vec<Action>,
    pub alone_timeout_ns: u64,
}
impl Default for MappingExtras {
    fn default() -> Self {
        Self {
            from_modifiers: ModifierFilter::default(),
            conditions: Vec::new(),
            delayed: None,
            if_held_down: Vec::new(),
            held_threshold_ns: 500_000_000,
            if_alone: Vec::new(),
            after_key_up: Vec::new(),
            alone_timeout_ns: 1_000_000_000,
        }
    }
}
impl Mapping {
    fn scope_budget(&self) -> usize {
        let mandatory = !self.extras.from_modifiers.mandatory.is_empty();
        usize::from(!self.extras.if_alone.is_empty()) * 8
            + usize::from(mandatory)
                * 8
                * (1 + usize::from(!self.extras.if_alone.is_empty())
                    + usize::from(!self.extras.after_key_up.is_empty())
                    + usize::from(!self.extras.if_held_down.is_empty()))
            + usize::from(!self.extras.if_held_down.is_empty())
            + usize::from(
                self.extras
                    .delayed
                    .as_ref()
                    .is_some_and(DelayedAction::enabled),
            ) * (9 + usize::from(mandatory) * 8)
    }
    /// All possible action branches, for backend admission before activation.
    pub fn actions(&self) -> impl Iterator<Item = &Action> + '_ {
        self.to
            .iter()
            .chain(&self.extras.if_held_down)
            .chain(&self.extras.if_alone)
            .chain(&self.extras.after_key_up)
            .chain(
                self.extras
                    .delayed
                    .iter()
                    .flat_map(|delayed| delayed.if_invoked.iter().chain(&delayed.if_canceled)),
            )
    }
}

#[derive(Clone, Debug)]
pub struct ConfigSnapshot {
    pub generation: ConfigGeneration,
    pub name: String,
    pub simple: Vec<Mapping>,
    pub complex: Vec<Mapping>,
}
impl ConfigSnapshot {
    pub fn empty(generation: ConfigGeneration) -> Self {
        Self {
            generation,
            name: "Passthrough".into(),
            simple: Vec::new(),
            complex: Vec::new(),
        }
    }
    fn max_deadline_delay(&self) -> u64 {
        self.simple
            .iter()
            .chain(&self.complex)
            .map(|mapping| {
                let held = if mapping.extras.if_held_down.is_empty() {
                    0
                } else {
                    mapping.extras.held_threshold_ns
                };
                held.max(
                    mapping
                        .extras
                        .delayed
                        .as_ref()
                        .filter(|delayed| delayed.enabled())
                        .map_or(0, |delayed| delayed.delay_ns),
                )
            })
            .max()
            .unwrap_or(0)
    }

    pub fn output_bounds(&self) -> BTreeMap<KeyId, usize> {
        let raw = self.expansion_bounds();
        let mut bounds = BTreeMap::<KeyId, usize>::new();
        let weights = |actions: &[Action]| {
            actions
                .iter()
                .map(|action| 1usize.saturating_add(action.modifiers.len()))
                .fold(0usize, usize::saturating_add)
        };
        for mapping in &self.complex {
            let tokens = raw.get(&mapping.from).copied().unwrap_or(1);
            let normal = weights(&mapping.to);
            let held = weights(&mapping.extras.if_held_down);
            let buffered = mapping
                .extras
                .delayed
                .as_ref()
                .is_some_and(DelayedAction::enabled)
                || !mapping.extras.if_alone.is_empty()
                || !mapping.extras.from_modifiers.mandatory.is_empty();
            let edges = tokens
                .saturating_mul(2)
                .saturating_add(if buffered { 0 } else { 8 })
                .max(
                    normal
                        .saturating_add(held)
                        .saturating_mul(2)
                        .saturating_add(8),
                );
            bounds
                .entry(mapping.from)
                .and_modify(|bound| *bound = (*bound).max(edges))
                .or_insert(edges);
        }
        for mapping in &self.simple {
            let edges = raw
                .get(&mapping.from)
                .copied()
                .unwrap_or(1)
                .saturating_mul(2)
                .saturating_add(8);
            bounds
                .entry(mapping.from)
                .and_modify(|bound| *bound = (*bound).max(edges))
                .or_insert(edges);
        }
        bounds
    }

    fn variable_references(&self) -> impl Iterator<Item = (&str, Option<&VariableValue>)> {
        self.simple.iter().chain(&self.complex).flat_map(|mapping| {
            let conditions = mapping
                .extras
                .conditions
                .iter()
                .chain(mapping.actions().flat_map(|action| &action.conditions))
                .filter_map(|condition| {
                    if let Condition::Variable { name, value, .. } = condition {
                        Some((name.as_str(), Some(value)))
                    } else {
                        None
                    }
                });
            let actions = mapping.actions().flat_map(|action| match &action.target {
                ActionTarget::Key(_) => Vec::new(),
                ActionTarget::SetVariable(variable) => match &variable.operation {
                    VariableOperation::Set {
                        value,
                        key_up_value,
                    } => vec![
                        (variable.name.as_str(), value.as_ref()),
                        (variable.name.as_str(), key_up_value.as_ref()),
                    ],
                    VariableOperation::Unset => vec![(variable.name.as_str(), None)],
                },
            });
            conditions.chain(actions)
        })
    }
    pub fn variable_names(&self) -> Result<BTreeSet<String>, EngineError> {
        let mut names = BTreeSet::new();
        for (name, value) in self.variable_references() {
            if !valid_variable_name(name) || value.is_some_and(|value| !value.bounded()) {
                return Err(EngineError::InvalidVariable);
            }
            names.insert(name.to_owned());
            if names.len() > MAX_VARIABLES {
                return Err(EngineError::ResourceLimit);
            }
        }
        for action in self
            .simple
            .iter()
            .chain(&self.complex)
            .flat_map(Mapping::actions)
        {
            if let ActionTarget::SetVariable(variable) = &action.target
                && (!action.modifiers.is_empty()
                    || matches!(
                        variable.operation,
                        VariableOperation::Set {
                            value: None,
                            key_up_value: None
                        }
                    ))
            {
                return Err(EngineError::InvalidVariable);
            }
        }
        Ok(names)
    }
    fn validate_timed_stages(&self, allow_action_timing: bool) -> Result<(), EngineError> {
        if !allow_action_timing
            && self
                .simple
                .iter()
                .chain(&self.complex)
                .flat_map(Mapping::actions)
                .any(|action| action.hold_down_ns != 0)
        {
            return Err(EngineError::UnsupportedActionTiming);
        }
        if self.simple.iter().any(|mapping| {
            !mapping.extras.if_held_down.is_empty()
                || mapping
                    .extras
                    .delayed
                    .as_ref()
                    .is_some_and(DelayedAction::enabled)
        }) {
            return Err(EngineError::InvalidMapping);
        }
        Ok(())
    }
    fn references_variable(&self, name: &str) -> bool {
        self.variable_references()
            .any(|(reference, _)| reference == name)
    }

    /// Upper bound independent of modifier changes within a sequence. This is
    /// checked before changing ownership so resource failure cannot drop ups.
    pub fn expansion_bounds(&self) -> BTreeMap<KeyId, usize> {
        let mut complex = BTreeMap::<KeyId, usize>::new();
        for mapping in &self.complex {
            let count = mapping
                .actions()
                .map(|action| 1usize.saturating_add(action.modifiers.len()))
                .fold(0, usize::saturating_add)
                .saturating_add(mapping.scope_budget())
                .max(1);
            complex
                .entry(mapping.from)
                .and_modify(|bound| *bound = (*bound).max(count))
                .or_insert(count);
        }
        let mut bounds = complex.clone();
        for mapping in &self.simple {
            let count = mapping
                .actions()
                .flat_map(|action| action.modifiers.iter().copied().chain(action.key()))
                .map(|key| complex.get(&key).copied().unwrap_or(1))
                .fold(0, usize::saturating_add)
                .saturating_add(mapping.scope_budget());
            bounds
                .entry(mapping.from)
                .and_modify(|bound| *bound = (*bound).max(count))
                .or_insert(count.max(1));
        }
        bounds
    }
}

#[derive(Clone, Debug)]
struct OwnedKey {
    key: KeyId,
    source: SourceToken,
    lazy_up: bool,
}

#[derive(Clone, Copy)]
struct KeyRequest {
    key: KeyId,
    lazy_down: bool,
    lazy_up: bool,
}
impl From<KeyId> for KeyRequest {
    fn from(key: KeyId) -> Self {
        Self {
            key,
            lazy_down: false,
            lazy_up: false,
        }
    }
}

#[derive(Clone, Debug)]
struct ActiveBinding {
    generation: ConfigGeneration,
    config: Arc<ConfigSnapshot>,
    bound: Bound,
    next_source: u64,
    reservation: usize,
}

#[derive(Clone, Debug)]
enum Bound {
    Leaf(OwnedKey),
    Variable {
        name: String,
        key_up_value: Option<VariableValue>,
    },
    Suppressed,
    Rule {
        stage: usize,
        index: usize,
        children: Vec<Bound>,
        // Main retained action, excluding temporary/scoped modifier bindings.
        repeat_child: Option<usize>,
        halted: Arc<AtomicBool>,
        down_at: MonoTime,
        deadline: Option<(MonoTime, DeadlineToken)>,
        alone: bool,
        modifiers_at_down: BTreeSet<KeyId>,
        consumed: BTreeSet<KeyId>,
        suppressed: Vec<(KeyId, SourceToken)>,
    },
}
impl Bound {
    fn unset_alone(&mut self) {
        if let Self::Rule {
            children, alone, ..
        } = self
        {
            *alone = false;
            for child in children {
                child.unset_alone();
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineState {
    pub bindings: usize,
    pub output_owners: usize,
    pub pressed_outputs: Vec<KeyId>,
    pub binding_generations: Vec<ConfigGeneration>,
    pub pending_deadlines: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_bindings: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineStep {
    pub capture: CaptureDecision,
    pub outputs: Vec<OutputIntent>,
    pub next_deadline: Option<MonoTime>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error("input time went backwards")]
    TimeBackwards,
    #[error("input sequence must strictly increase")]
    SequenceBackwards,
    #[error("input key identity belongs to a retired device generation")]
    RetiredDevice,
    #[error("bounded engine resource exhausted; runtime must reset and bypass")]
    ResourceLimit,
    #[error("device identity history exhausted; reset outputs, drain input and restart engine")]
    DeviceHistoryFull,
    #[error("configuration generation must increase")]
    InvalidGeneration,
    #[error("invalid, stale or oversized context snapshot")]
    InvalidContext,
    #[error("invalid variable name, value or action")]
    InvalidVariable,
    #[error("held and delayed actions require a complex mapping")]
    InvalidMapping,
    #[error("action duration requires the deferred stage actor, which is not yet connected")]
    UnsupportedActionTiming,
    #[error("a due deadline must be delivered before this physical event")]
    DeadlinePending,
    #[error("cancel delayed tasks individually before this down to keep output bounded")]
    CancellationPending,
    #[error("deadline is early or not the earliest scheduled token")]
    InvalidDeadline,
    #[error("deadline would overflow monotonic time")]
    TimeOverflow,
}

#[derive(Clone, Copy)]
struct DeviceState {
    generation: u64,
    removed: bool,
}

#[derive(Clone, Copy)]
struct DeadlineTask {
    identity: (DeviceInstanceId, KeyId),
    stage: usize,
    delayed: bool,
}

#[derive(Clone)]
struct DelayedPayload {
    halted: Arc<AtomicBool>,
    stage: usize,
    index: usize,
    modifiers_at_down: BTreeSet<KeyId>,
    consumed: BTreeSet<KeyId>,
}

fn variable_truth(
    variables: &BTreeMap<String, Option<VariableValue>>,
    condition: &Condition,
) -> Option<Truth> {
    let Condition::Variable {
        name,
        value,
        unless,
    } = condition
    else {
        return None;
    };
    let matched = variables
        .get(name)
        .and_then(Option::as_ref)
        .map_or(*value == VariableValue::Integer(0), |actual| {
            actual == value
        })
        != *unless;
    Some(if matched { Truth::True } else { Truth::False })
}

pub struct Engine {
    config: Arc<ConfigSnapshot>,
    variables: BTreeMap<String, Option<VariableValue>>,
    deadlines: BTreeMap<(MonoTime, DeadlineToken), DeadlineTask>,
    current_identity: (DeviceInstanceId, KeyId),
    delayed: BTreeMap<DeadlineToken, DelayedPayload>,
    pending: BTreeMap<(DeviceInstanceId, KeyId), ActiveBinding>,
    context: ContextSnapshot,
    current_device: DeviceInstanceId,
    bounds: BTreeMap<KeyId, usize>,
    output_bounds: BTreeMap<KeyId, usize>,
    reservations: usize,
    suppression_references: usize,
    may_consume_modifiers: bool,
    alone_candidate: Option<(DeviceInstanceId, KeyId)>,
    active: BTreeMap<(DeviceInstanceId, KeyId), ActiveBinding>,
    owners: BTreeMap<KeyId, BTreeSet<SourceToken>>,
    hidden: BTreeMap<(KeyId, SourceToken), usize>,
    emitted: BTreeSet<KeyId>,
    repeating_key: Option<KeyId>,
    devices: BTreeMap<u64, DeviceState>,
    last_seq: Option<u64>,
    now: MonoTime,
    next_source: u64,
}

impl Engine {
    /// Read the current retained terminal of this physical binding. Backends
    /// with a physical repeat source may resend it without replaying actions.
    /// Native repeats use the current output modifier state, just like a held
    /// keyboard key; this query does not recreate the original shortcut scope.
    pub fn physical_repeat_target(&self, device: DeviceInstanceId, key: KeyId) -> Option<KeyId> {
        let mut bound = &self.active.get(&(device, key))?.bound;
        loop {
            match bound {
                Bound::Rule {
                    children,
                    repeat_child,
                    ..
                } => {
                    bound = children.get((*repeat_child)?)?;
                }
                Bound::Leaf(owned) => {
                    return (owned.key.page == KEYBOARD_PAGE
                        && !owned.key.is_modifier()
                        && self.emitted.contains(&owned.key)
                        && self
                            .owners
                            .get(&owned.key)
                            .is_some_and(|owners| owners.contains(&owned.source))
                        && !self.hidden.contains_key(&(owned.key, owned.source)))
                    .then_some(owned.key);
                }
                Bound::Variable { .. } | Bound::Suppressed => return None,
            }
        }
    }
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, EngineError> {
        Self::new_inner(config, false)
    }
    fn new_inner(
        config: Arc<ConfigSnapshot>,
        allow_action_timing: bool,
    ) -> Result<Self, EngineError> {
        config.validate_timed_stages(allow_action_timing)?;
        let variables = config
            .variable_names()?
            .into_iter()
            .map(|name| (name, None))
            .collect();
        Ok(Self {
            variables,
            deadlines: BTreeMap::new(),
            delayed: BTreeMap::new(),
            pending: BTreeMap::new(),
            current_identity: (
                DeviceInstanceId {
                    id: 0,
                    generation: 0,
                },
                KeyId::new(7, 0),
            ),
            may_consume_modifiers: config
                .simple
                .iter()
                .chain(&config.complex)
                .any(|mapping| !mapping.extras.from_modifiers.mandatory.is_empty()),
            suppression_references: 0,
            context: ContextSnapshot::default(),
            current_device: DeviceInstanceId {
                id: 0,
                generation: 0,
            },
            bounds: config.expansion_bounds(),
            output_bounds: config.output_bounds(),
            reservations: 0,
            alone_candidate: None,
            config,
            active: BTreeMap::new(),
            owners: BTreeMap::new(),
            hidden: BTreeMap::new(),
            emitted: BTreeSet::new(),
            repeating_key: None,
            devices: BTreeMap::new(),
            last_seq: None,
            now: MonoTime(0),
            next_source: 1,
        })
    }

    pub fn next_deadline(&self) -> Option<(MonoTime, DeadlineToken)> {
        self.deadlines.first_key_value().map(|(key, _)| *key)
    }

    pub fn variable(&self, name: &str) -> VariableValue {
        self.variables
            .get(name)
            .and_then(|value| value.clone())
            .unwrap_or_default()
    }
    pub fn variable_values(&self) -> BTreeMap<String, VariableValue> {
        self.variables
            .iter()
            .filter_map(|(name, value)| value.clone().map(|value| (name.clone(), value)))
            .collect()
    }
    fn assign_variable(&mut self, name: &str, value: Option<VariableValue>) {
        // Construction/activation pre-reserve every name, including old bindings.
        if let Some(slot) = self.variables.get_mut(name) {
            *slot = value;
        }
    }

    pub fn context_revision(&self) -> u64 {
        self.context.revision
    }

    fn condition_truth(&self, condition: &Condition) -> Truth {
        variable_truth(&self.variables, condition)
            .unwrap_or_else(|| condition.evaluate(&self.context, self.current_device, self.now))
    }

    fn validate_context(
        &self,
        context: &ContextSnapshot,
        now: MonoTime,
    ) -> Result<(), EngineError> {
        let mut identities = BTreeSet::new();
        let mut strings = Vec::new();
        if let Some(application) = &context.application {
            strings.extend([&application.bundle_identifier, &application.file_path]);
        }
        if let Some(source) = &context.input_source {
            strings.extend([
                &source.language,
                &source.input_source_id,
                &source.input_mode_id,
            ]);
        }
        if strings
            .iter()
            .any(|value| value.as_ref().is_some_and(|value| value.len() > 4096))
        {
            return Err(EngineError::InvalidContext);
        }
        if context.revision <= self.context.revision
            || context.provider.is_empty()
            || context.provider.len() > 128
            || context.observed_at_ns > now
            || context.valid_until_ns < context.observed_at_ns
            || context.valid_until_ns.0 - context.observed_at_ns.0 > MAX_CONTEXT_LIFETIME_NS
            || context.devices.as_ref().is_some_and(|devices| {
                devices.len() > MAX_CONTEXT_DEVICES
                    || devices.iter().any(|entry| {
                        !identities.insert(entry.device.id)
                            || entry
                                .properties
                                .device_address
                                .as_ref()
                                .is_some_and(|address| address.len() > 256)
                            || self.devices.get(&entry.device.id).is_some_and(|state| {
                                state.generation > entry.device.generation
                                    || (state.generation == entry.device.generation
                                        && state.removed)
                            })
                    })
            })
        {
            return Err(EngineError::InvalidContext);
        }
        Ok(())
    }

    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), EngineError> {
        self.activate_inner(config, false)
    }
    fn activate_inner(
        &mut self,
        config: Arc<ConfigSnapshot>,
        allow_action_timing: bool,
    ) -> Result<(), EngineError> {
        if config.generation <= self.config.generation {
            return Err(EngineError::InvalidGeneration);
        }
        config.validate_timed_stages(allow_action_timing)?;
        let names = config.variable_names()?;
        if self
            .variables
            .keys()
            .cloned()
            .chain(names.iter().cloned())
            .collect::<BTreeSet<_>>()
            .len()
            > MAX_VARIABLES
        {
            return Err(EngineError::ResourceLimit);
        }
        for name in names {
            self.variables.entry(name).or_insert(None);
        }
        self.bounds = config.expansion_bounds();
        self.output_bounds = config.output_bounds();
        self.may_consume_modifiers = config
            .simple
            .iter()
            .chain(&config.complex)
            .any(|mapping| !mapping.extras.from_modifiers.mandatory.is_empty());
        self.config = config;
        Ok(())
    }

    /// Current admitted snapshot, independent of generations of held bindings.
    pub fn config_generation(&self) -> ConfigGeneration {
        self.config.generation
    }

    pub fn state(&self) -> EngineState {
        EngineState {
            bindings: self.active.len(),
            output_owners: self.owners.values().map(BTreeSet::len).sum(),
            pressed_outputs: self.emitted.iter().copied().collect(),
            binding_generations: self.active.values().map(|b| b.generation).collect(),
            pending_deadlines: self.deadlines.len(),
            pending_bindings: self.pending.len(),
        }
    }
    pub fn tracked_devices(&self) -> usize {
        self.devices.len()
    }
    pub fn suppression_references(&self) -> usize {
        self.suppression_references
    }

    pub fn handle(&mut self, input: InputEnvelope) -> Result<EngineStep, EngineError> {
        if input.at_ns < self.now {
            return Err(EngineError::TimeBackwards);
        }
        if self.last_seq.is_some_and(|seq| input.seq <= seq) {
            return Err(EngineError::SequenceBackwards);
        }
        if input.provenance == Provenance::Physical
            && let InputEvent::ContextChanged { context } = &input.event
        {
            self.validate_context(context, input.at_ns)?;
        }
        if input.provenance == Provenance::Physical {
            if let InputEvent::TimerFired { token } = input.event {
                if self
                    .deadlines
                    .iter()
                    .find(|((_, candidate), _)| *candidate == token)
                    .and_then(|(_, task)| {
                        self.active
                            .get(&task.identity)
                            .or_else(|| self.pending.get(&task.identity))
                    })
                    .is_some_and(|binding| {
                        input
                            .at_ns
                            .0
                            .checked_add(binding.config.max_deadline_delay())
                            .is_none()
                    })
                {
                    return Err(EngineError::TimeOverflow);
                }
                if let Some((deadline, _)) = self
                    .deadlines
                    .iter()
                    .find(|((_, candidate), _)| *candidate == token)
                    && (deadline.0 > input.at_ns || self.next_deadline() != Some(*deadline))
                {
                    return Err(EngineError::InvalidDeadline);
                }
            } else if matches!(
                input.event,
                InputEvent::Key { .. } | InputEvent::Wheel { .. } | InputEvent::PointerMove { .. }
            ) && self
                .next_deadline()
                .is_some_and(|(at, _)| at <= input.at_ns)
            {
                return Err(EngineError::DeadlinePending);
            }
        }
        if input.provenance == Provenance::Physical {
            if let InputEvent::CancelDelayed { token } = input.event {
                if self.deadlines.values().any(|task| task.delayed)
                    && self
                        .next_deadline()
                        .is_some_and(|(at, _)| at <= input.at_ns)
                {
                    return Err(EngineError::DeadlinePending);
                }
                if self
                    .deadlines
                    .iter()
                    .any(|((_, candidate), task)| *candidate == token && !task.delayed)
                {
                    return Err(EngineError::InvalidDeadline);
                }
            }
            if let InputEvent::Key {
                key,
                edge: KeyEdge::Down,
            } = input.event
                && !self.active.contains_key(&(input.device, key))
                && !self.delayed.is_empty()
                && (self
                    .cancellation_budget()
                    .saturating_mul(2)
                    .saturating_add(self.output_bounds.get(&key).copied().unwrap_or(10))
                    > MAX_OUTPUTS_PER_EVENT
                    || (!self.pending.is_empty()
                        && (self.active.len().saturating_add(self.pending.len()) >= MAX_BINDINGS
                            || self
                                .reservations
                                .saturating_add(self.bounds.get(&key).copied().unwrap_or(1))
                                > MAX_OUTPUT_OWNERS)))
            {
                return Err(EngineError::CancellationPending);
            }
        }
        self.now = input.at_ns;
        self.last_seq = Some(input.seq);
        let mut step = EngineStep {
            capture: CaptureDecision::Capture,
            outputs: Vec::new(),
            next_deadline: self.next_deadline().map(|(at, _)| at),
        };
        if input.provenance != Provenance::Physical {
            step.capture = CaptureDecision::Pass;
            return Ok(step);
        }
        self.current_device = input.device;
        match input.event {
            InputEvent::CancelDelayed { token } => {
                step.capture = CaptureDecision::Pass;
                self.execute_delayed(token, false, &mut step.outputs);
            }
            InputEvent::TimerFired { token } => {
                step.capture = CaptureDecision::Pass;
                self.fire_deadline(token, &mut step.outputs);
            }
            InputEvent::ContextChanged { context } => {
                self.context = context;
                step.capture = CaptureDecision::Pass;
            }
            InputEvent::Reset => step.outputs = self.reset(input.at_ns)?,
            InputEvent::DeviceRemoved => {
                self.update_device(input.device, true, &mut step.outputs)?;
            }
            InputEvent::Wheel {
                horizontal,
                vertical,
            } => {
                self.update_device(input.device, false, &mut step.outputs)?;
                step.capture = CaptureDecision::Pass;
                self.sync_pointer_modifiers(&mut step.outputs);
                if horizontal != 0 || vertical != 0 {
                    self.cancel_alone();
                }
            }
            InputEvent::PointerMove { .. } => {
                self.update_device(input.device, false, &mut step.outputs)?;
                step.capture = CaptureDecision::Pass;
                self.sync_pointer_modifiers(&mut step.outputs);
            }
            InputEvent::Key { key, edge } => {
                if self.devices.get(&input.device.id).is_some_and(|state| {
                    state.generation > input.device.generation
                        || (state.generation == input.device.generation && state.removed)
                }) {
                    return Err(EngineError::RetiredDevice);
                }
                let identity = (input.device, key);
                self.current_identity = identity;
                if edge == KeyEdge::Down && !self.active.contains_key(&identity) {
                    let max_threshold = self.config.max_deadline_delay();
                    if input.at_ns.0.checked_add(max_threshold).is_none() {
                        return Err(EngineError::TimeOverflow);
                    }
                    let bound = self.bounds.get(&key).copied().unwrap_or(1);
                    if self.active.len().saturating_add(self.pending.len()) >= MAX_BINDINGS
                        || self.output_bounds.get(&key).copied().unwrap_or(10)
                            > MAX_OUTPUTS_PER_EVENT
                        || self.reservations.saturating_add(bound) > MAX_OUTPUT_OWNERS
                        || self.next_source.checked_add(bound as u64).is_none()
                        || (self.may_consume_modifiers
                            && self.suppression_references.saturating_add(
                                self.current_modifiers()
                                    .iter()
                                    .map(|key| self.owners.get(key).map_or(0, BTreeSet::len))
                                    .sum::<usize>()
                                    .saturating_add(bound)
                                    .saturating_mul(2),
                            ) > MAX_PERSISTENT_SUPPRESSION_REFERENCES)
                    {
                        return Err(EngineError::ResourceLimit);
                    }
                }
                self.update_device(input.device, false, &mut step.outputs)?;
                match edge {
                    KeyEdge::Down => {
                        if self.active.contains_key(&identity) {
                            return Ok(step);
                        }
                        let bound = self.bounds.get(&key).copied().unwrap_or(1);
                        self.cancel_alone();
                        self.cancel_tasks(None, &mut step.outputs);
                        self.current_identity = identity;
                        let mut next_source = self.next_source;
                        self.next_source += bound as u64;
                        self.reservations += bound;
                        let config = Arc::clone(&self.config);
                        let held = self.press_stage(
                            key.into(),
                            0,
                            &config,
                            &mut next_source,
                            &mut step.outputs,
                        );
                        self.active.insert(
                            identity,
                            ActiveBinding {
                                generation: self.config.generation,
                                config,
                                bound: held,
                                next_source,
                                reservation: bound,
                            },
                        );
                        // Every new physical down cancels its predecessor, so
                        // only the most recent binding can still be alone.
                        self.alone_candidate = Some(identity);
                    }
                    KeyEdge::Up => {
                        if self.alone_candidate == Some(identity) {
                            self.alone_candidate = None;
                        }
                        if let Some(mut binding) = self.active.remove(&identity) {
                            self.release_bound(
                                std::mem::replace(&mut binding.bound, Bound::Suppressed),
                                &binding.config,
                                &mut binding.next_source,
                                true,
                                &mut step.outputs,
                            );
                            if self.has_tasks(identity) {
                                self.pending.insert(identity, binding);
                            } else {
                                self.reservations -= binding.reservation;
                            }
                            if !self
                                .active
                                .keys()
                                .any(|(device, _)| *device == input.device)
                            {
                                self.sync_modifiers(&mut step.outputs);
                            }
                        }
                    }
                    KeyEdge::Repeat => {
                        // Physical repeats never create ownership or duplicate a tap/macro.
                        // Platform backends choose a single OS-driven repeat source.
                    }
                }
            }
        }
        step.next_deadline = self.next_deadline().map(|(at, _)| at);
        Ok(step)
    }

    pub fn next_delayed(&self) -> Option<DeadlineToken> {
        self.deadlines
            .iter()
            .find(|(_, task)| task.delayed)
            .map(|((_, token), _)| *token)
    }
    fn has_tasks(&self, identity: (DeviceInstanceId, KeyId)) -> bool {
        self.deadlines
            .values()
            .any(|task| task.identity == identity)
    }
    fn cancellation_budget(&self) -> usize {
        self.deadlines
            .values()
            .filter(|task| task.delayed)
            .map(|task| task.identity)
            .collect::<BTreeSet<_>>()
            .iter()
            .filter_map(|identity| {
                self.active
                    .get(identity)
                    .or_else(|| self.pending.get(identity))
            })
            .map(|binding| binding.reservation)
            .sum()
    }
    fn take_delayed(&mut self, token: DeadlineToken) -> Option<(DeadlineTask, DelayedPayload)> {
        let deadline = self
            .deadlines
            .keys()
            .find(|(_, candidate)| *candidate == token)
            .copied()?;
        let task = *self.deadlines.get(&deadline)?;
        if !task.delayed {
            return None;
        }
        let payload = self.delayed.remove(&token)?;
        self.deadlines.remove(&deadline);
        Some((task, payload))
    }
    fn execute_delayed(
        &mut self,
        token: DeadlineToken,
        invoked: bool,
        outputs: &mut Vec<OutputIntent>,
    ) {
        let Some((task, payload)) = self.take_delayed(token) else {
            return;
        };
        let was_active = self.active.contains_key(&task.identity);
        let Some(mut binding) = self
            .active
            .remove(&task.identity)
            .or_else(|| self.pending.remove(&task.identity))
        else {
            return;
        };
        let previous_device = self.current_device;
        let previous_identity = self.current_identity;
        self.current_device = task.identity.0;
        self.current_identity = task.identity;
        self.delayed_actions(
            &payload,
            invoked,
            &binding.config,
            &mut binding.next_source,
            outputs,
        );
        if was_active {
            self.active.insert(task.identity, binding);
        } else if self.has_tasks(task.identity) {
            self.pending.insert(task.identity, binding);
        } else {
            self.reservations -= binding.reservation;
        }
        if !self
            .active
            .keys()
            .any(|(device, _)| *device == task.identity.0)
        {
            self.sync_modifiers(outputs);
        }
        self.current_device = previous_device;
        self.current_identity = previous_identity;
    }
    fn cancel_tasks(&mut self, stage: Option<usize>, outputs: &mut Vec<OutputIntent>) {
        let tasks: Vec<_> = self
            .deadlines
            .iter()
            .filter(|(_, task)| stage.is_none_or(|stage| task.stage == stage))
            .map(|(deadline, task)| (*deadline, *task))
            .collect();
        for (deadline, task) in tasks {
            if task.delayed {
                self.execute_delayed(deadline.1, false, outputs);
            } else {
                self.deadlines.remove(&deadline);
            }
        }
    }
    fn cancel_stage(
        &mut self,
        stage: usize,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        outputs: &mut Vec<OutputIntent>,
    ) {
        let tasks: Vec<_> = self
            .deadlines
            .iter()
            .filter(|(_, task)| task.stage == stage)
            .map(|(deadline, task)| (*deadline, *task))
            .collect();
        for (deadline, task) in tasks {
            if task.delayed && task.identity == self.current_identity {
                if let Some((_, payload)) = self.take_delayed(deadline.1) {
                    self.delayed_actions(&payload, false, config, next_source, outputs);
                }
            } else if task.delayed {
                self.execute_delayed(deadline.1, false, outputs);
            } else {
                self.deadlines.remove(&deadline);
            }
        }
    }
    fn delayed_actions(
        &mut self,
        payload: &DelayedPayload,
        invoked: bool,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        outputs: &mut Vec<OutputIntent>,
    ) {
        let mapping = if payload.stage == 0 {
            &config.simple[payload.index]
        } else {
            &config.complex[payload.index]
        };
        if let Some(delayed) = &mapping.extras.delayed {
            let actions = if invoked {
                &delayed.if_invoked
            } else {
                &delayed.if_canceled
            };
            self.snapshot_actions(
                actions,
                payload.stage + 1,
                config,
                next_source,
                (
                    &payload.modifiers_at_down,
                    &payload.consumed,
                    &payload.halted,
                ),
                outputs,
            );
        }
    }
    fn snapshot_actions(
        &mut self,
        actions: &[Action],
        stage: usize,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        modifiers: (&BTreeSet<KeyId>, &BTreeSet<KeyId>, &AtomicBool),
        outputs: &mut Vec<OutputIntent>,
    ) {
        let (modifiers_at_down, consumed, halted) = modifiers;
        let targets = self
            .current_modifiers()
            .difference(modifiers_at_down)
            .copied()
            .collect();
        let suppressed = self.hide_modifiers(&targets);
        let mut temporary = Vec::new();
        for key in modifiers_at_down
            .difference(&self.current_modifiers())
            .copied()
            .collect::<Vec<_>>()
        {
            let source = SourceToken(*next_source);
            *next_source += 1;
            self.owners.entry(key).or_default().insert(source);
            temporary.push((key, source));
        }
        let mandatory_scope = self.hide_modifiers(consumed);
        self.press_actions(actions, stage, config, next_source, (true, halted), outputs);
        self.unhide_modifiers(mandatory_scope);
        for (key, source) in temporary.into_iter().rev() {
            self.remove_owner(key, source);
        }
        self.unhide_modifiers(suppressed);
    }

    fn fire_deadline(&mut self, token: DeadlineToken, outputs: &mut Vec<OutputIntent>) {
        let Some((deadline, identity)) = self
            .deadlines
            .first_key_value()
            .filter(|((_, candidate), _)| *candidate == token)
            .map(|(deadline, task)| (*deadline, task.identity))
        else {
            return;
        };
        if self
            .deadlines
            .get(&deadline)
            .is_some_and(|task| task.delayed)
        {
            self.execute_delayed(token, true, outputs);
            return;
        }
        self.deadlines.remove(&deadline);
        if let Some(mut binding) = self.active.remove(&identity) {
            self.current_device = identity.0;
            self.current_identity = identity;
            self.replace_held(
                &mut binding.bound,
                token,
                &binding.config,
                &mut binding.next_source,
                outputs,
            );
            self.active.insert(identity, binding);
        }
    }
    fn replace_held(
        &mut self,
        bound: &mut Bound,
        token: DeadlineToken,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        outputs: &mut Vec<OutputIntent>,
    ) -> bool {
        let Bound::Rule {
            stage,
            index,
            children,
            repeat_child,
            deadline,
            consumed,
            suppressed,
            halted,
            ..
        } = bound
        else {
            return false;
        };
        if deadline.is_some_and(|(_, candidate)| candidate == token) {
            *deadline = None;
            for child in std::mem::take(children).into_iter().rev() {
                self.release_bound(child, config, next_source, true, outputs);
            }
            self.unhide_modifiers(std::mem::take(suppressed));
            let mapping = if *stage == 0 {
                &config.simple[*index]
            } else {
                &config.complex[*index]
            };
            *suppressed = self.hide_modifiers(consumed);
            let actions =
                rule_selection::filter_actions(&mapping.extras.if_held_down, |condition| {
                    self.condition_truth(condition)
                });
            let last_is_modifier = actions
                .last()
                .is_some_and(|action| action.key().is_some_and(KeyId::is_modifier));
            (*children, *repeat_child) = self.press_filtered_actions(
                &actions,
                *stage + 1,
                config,
                next_source,
                (false, halted),
                outputs,
            );
            if !last_is_modifier {
                self.unhide_modifiers(std::mem::take(suppressed));
            }
            return true;
        }
        children
            .iter_mut()
            .any(|child| self.replace_held(child, token, config, next_source, outputs))
    }

    pub fn reset(&mut self, now: MonoTime) -> Result<Vec<OutputIntent>, EngineError> {
        if now < self.now {
            return Err(EngineError::TimeBackwards);
        }
        self.now = now;
        let mut outputs = Vec::new();
        // Non-modifiers release before modifiers, preserving shortcut release ordering.
        for modifier in [false, true] {
            for key in self
                .emitted
                .iter()
                .filter(|key| key.is_modifier() == modifier)
                .copied()
            {
                outputs.push(OutputIntent {
                    at_ns: now,
                    key,
                    edge: KeyEdge::Up,
                });
            }
        }
        self.variables
            .retain(|name, _| self.config.references_variable(name));
        for value in self.variables.values_mut() {
            *value = None;
        }
        self.deadlines.clear();
        self.delayed.clear();
        self.pending.clear();
        self.owners.clear();
        self.hidden.clear();
        self.emitted.clear();
        self.repeating_key = None;
        self.active.clear();
        self.reservations = 0;
        self.suppression_references = 0;
        self.alone_candidate = None;
        Ok(outputs)
    }

    fn update_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        outputs: &mut Vec<OutputIntent>,
    ) -> Result<(), EngineError> {
        if let Some(previous) = self.devices.get(&device.id).copied() {
            if previous.generation > device.generation
                || (previous.generation == device.generation && previous.removed)
            {
                return if removed {
                    Ok(())
                } else {
                    Err(EngineError::RetiredDevice)
                };
            }
            if previous.generation < device.generation {
                self.release_device(
                    DeviceInstanceId {
                        id: device.id,
                        generation: previous.generation,
                    },
                    self.now,
                    outputs,
                );
            }
        } else if self.devices.len() >= MAX_DEVICE_IDENTITIES {
            return Err(EngineError::DeviceHistoryFull);
        }
        if removed {
            self.release_device(device, self.now, outputs);
        }
        if let Some(inventory) = &mut self.context.devices {
            if removed {
                inventory.retain(|entry| {
                    entry.device.id != device.id || entry.device.generation > device.generation
                });
            } else if let Some(entry) = inventory
                .iter_mut()
                .find(|entry| entry.device.id == device.id)
            {
                if entry.device.generation < device.generation {
                    *entry = DeviceContext {
                        device,
                        properties: DeviceProperties::default(),
                    };
                }
            } else if inventory.len() < MAX_CONTEXT_DEVICES {
                inventory.push(DeviceContext {
                    device,
                    properties: DeviceProperties::default(),
                });
            } else {
                self.context.devices = None;
            }
        }
        self.devices.insert(
            device.id,
            DeviceState {
                generation: device.generation,
                removed,
            },
        );
        Ok(())
    }

    fn release_device(
        &mut self,
        device: DeviceInstanceId,
        _now: MonoTime,
        outputs: &mut Vec<OutputIntent>,
    ) {
        if self
            .alone_candidate
            .is_some_and(|(candidate, _)| candidate == device)
        {
            self.alone_candidate = None;
        }
        let tokens: Vec<_> = self
            .deadlines
            .iter()
            .filter(|(_, task)| task.identity.0 == device)
            .map(|(deadline, _)| *deadline)
            .collect();
        for deadline in tokens {
            self.deadlines.remove(&deadline);
            self.delayed.remove(&deadline.1);
        }
        let pending: Vec<_> = self
            .pending
            .keys()
            .filter(|(source, _)| *source == device)
            .copied()
            .collect();
        for identity in pending {
            if let Some(binding) = self.pending.remove(&identity) {
                self.reservations -= binding.reservation;
            }
        }
        let keys: Vec<_> = self
            .active
            .keys()
            .filter(|(d, _)| *d == device)
            .copied()
            .collect();
        for identity in keys {
            if let Some(mut binding) = self.active.remove(&identity) {
                self.release_bound(
                    binding.bound,
                    &binding.config,
                    &mut binding.next_source,
                    false,
                    outputs,
                );
                self.reservations -= binding.reservation;
            }
        }
        self.sync_modifiers(outputs);
    }
    fn cancel_alone(&mut self) {
        if let Some(identity) = self.alone_candidate.take()
            && let Some(binding) = self.active.get_mut(&identity)
        {
            binding.bound.unset_alone();
        }
    }
    fn press_stage(
        &mut self,
        request: KeyRequest,
        stage: usize,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        outputs: &mut Vec<OutputIntent>,
    ) -> Bound {
        if stage <= 1 {
            self.cancel_stage(stage, config, next_source, outputs);
        }
        let key = request.key;
        if stage >= 2 {
            if key.usage == 0 {
                self.sync_modifiers(outputs);
                return Bound::Suppressed;
            }
            // Physical down already reserved all source identities.
            let source = SourceToken(*next_source);
            *next_source += 1;
            if request.lazy_down && key.is_modifier() {
                self.owners.entry(key).or_default().insert(source);
            } else {
                self.press(key, source, self.now, outputs);
            }
            return Bound::Leaf(OwnedKey {
                key,
                source,
                lazy_up: request.lazy_up,
            });
        }
        let stage_kind = if stage == 0 {
            rule_selection::MappingStage::Simple
        } else {
            rule_selection::MappingStage::Complex
        };
        let selection = rule_selection::select_rule(
            Arc::clone(config),
            stage_kind,
            key,
            self.current_modifiers(),
            |condition| self.condition_truth(condition),
        );
        let Some(selection) = selection else {
            return self.press_stage(request, stage + 1, config, next_source, outputs);
        };
        let mapping = selection.mapping();
        let index = selection.index();
        let modifiers_at_down = selection.modifiers_at_down().clone();
        let consumed = selection.consumed_modifiers().clone();
        let actions: Vec<_> = selection.actions().collect();
        let mut suppressed = self.hide_modifiers(&consumed);
        let last_is_modifier = actions
            .last()
            .is_some_and(|action| action.key().is_some_and(KeyId::is_modifier));
        let halted = Arc::new(AtomicBool::new(false));
        let (children, repeat_child) = self.press_filtered_actions(
            &actions,
            stage + 1,
            config,
            next_source,
            (false, &halted),
            outputs,
        );
        if !last_is_modifier {
            self.unhide_modifiers(std::mem::take(&mut suppressed));
        }
        let deadline = if mapping.extras.if_held_down.is_empty() {
            None
        } else {
            let token = DeadlineToken(*next_source);
            *next_source += 1;
            let deadline = (
                MonoTime(self.now.0.saturating_add(mapping.extras.held_threshold_ns)),
                token,
            );
            self.deadlines.insert(
                deadline,
                DeadlineTask {
                    identity: self.current_identity,
                    stage,
                    delayed: false,
                },
            );
            Some(deadline)
        };
        if let Some(delayed) = &mapping.extras.delayed
            && delayed.enabled()
        {
            let token = DeadlineToken(*next_source);
            *next_source += 1;
            let deadline = (MonoTime(self.now.0.saturating_add(delayed.delay_ns)), token);
            self.deadlines.insert(
                deadline,
                DeadlineTask {
                    identity: self.current_identity,
                    stage,
                    delayed: true,
                },
            );
            self.delayed.insert(
                token,
                DelayedPayload {
                    halted: Arc::clone(&halted),
                    stage,
                    index,
                    modifiers_at_down: modifiers_at_down.clone(),
                    consumed: consumed.clone(),
                },
            );
        }
        Bound::Rule {
            stage,
            index,
            children,
            repeat_child,
            deadline,
            halted,
            down_at: self.now,
            alone: true,
            modifiers_at_down,
            consumed,
            suppressed,
        }
    }

    fn press_actions(
        &mut self,
        actions: &[Action],
        stage: usize,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        policy: (bool, &AtomicBool),
        outputs: &mut Vec<OutputIntent>,
    ) -> (Vec<Bound>, Option<usize>) {
        let (_, halted) = policy;
        if halted.load(Ordering::Relaxed) {
            return (Vec::new(), None);
        }
        let actions =
            rule_selection::filter_actions(actions, |condition| self.condition_truth(condition));
        self.press_filtered_actions(&actions, stage, config, next_source, policy, outputs)
    }
    fn press_filtered_actions(
        &mut self,
        actions: &[&Action],
        stage: usize,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        policy: (bool, &AtomicBool),
        outputs: &mut Vec<OutputIntent>,
    ) -> (Vec<Bound>, Option<usize>) {
        let (tap_all, halted) = policy;
        if halted.load(Ordering::Relaxed) {
            return (Vec::new(), None);
        }
        let mut held = Vec::new();
        let mut repeat_child = None;
        for (index, action) in actions.iter().enumerate() {
            let mut modifiers = Vec::new();
            for key in &action.modifiers {
                modifiers.push(self.press_stage(
                    KeyRequest {
                        key: *key,
                        lazy_down: tap_all
                            || action.lazy
                            || !action.key().is_some_and(KeyId::is_modifier),
                        lazy_up: true,
                    },
                    stage,
                    config,
                    next_source,
                    outputs,
                ));
            }
            let child = match &action.target {
                ActionTarget::Key(key) => self.press_stage(
                    KeyRequest {
                        key: *key,
                        lazy_down: action.lazy,
                        lazy_up: action.lazy,
                    },
                    stage,
                    config,
                    next_source,
                    outputs,
                ),
                ActionTarget::SetVariable(variable) => {
                    self.sync_modifiers(outputs);
                    let key_up_value = match &variable.operation {
                        VariableOperation::Set {
                            value,
                            key_up_value,
                        } => {
                            if let Some(value) = value {
                                self.assign_variable(&variable.name, Some(value.clone()));
                            }
                            key_up_value.clone()
                        }
                        VariableOperation::Unset => {
                            self.assign_variable(&variable.name, None);
                            None
                        }
                    };
                    Bound::Variable {
                        name: variable.name.clone(),
                        key_up_value,
                    }
                }
            };
            if action.halt {
                halted.store(true, Ordering::Relaxed);
            }
            if !tap_all && action.repeat && index + 1 == actions.len() {
                repeat_child = Some(held.len());
                held.push(child);
            } else {
                self.release_bound(child, config, next_source, true, outputs);
            }
            if !tap_all
                && index + 1 == actions.len()
                && action.key().is_some_and(KeyId::is_modifier)
            {
                // Modifier scope up follows the main modifier up.
                repeat_child = repeat_child.map(|index| index + modifiers.len());
                modifiers.append(&mut held);
                held = modifiers;
            } else {
                for modifier in modifiers {
                    self.release_bound(modifier, config, next_source, true, outputs);
                }
            }
        }
        (held, repeat_child)
    }

    fn release_bound(
        &mut self,
        bound: Bound,
        config: &Arc<ConfigSnapshot>,
        next_source: &mut u64,
        extra_actions: bool,
        outputs: &mut Vec<OutputIntent>,
    ) {
        match bound {
            Bound::Leaf(owned) => {
                if owned.lazy_up && owned.key.is_modifier() {
                    self.remove_owner(owned.key, owned.source);
                } else {
                    self.release(owned.key, owned.source, self.now, outputs);
                }
            }
            Bound::Variable { name, key_up_value } => {
                if let Some(value) = key_up_value {
                    self.assign_variable(&name, Some(value));
                }
            }
            Bound::Suppressed => {}
            Bound::Rule {
                stage,
                index,
                children,
                halted,
                down_at,
                deadline,
                alone,
                modifiers_at_down,
                consumed,
                suppressed,
                ..
            } => {
                if let Some(deadline) = deadline {
                    self.deadlines.remove(&deadline);
                }
                for child in children.into_iter().rev() {
                    self.release_bound(child, config, next_source, extra_actions, outputs);
                }
                self.unhide_modifiers(suppressed);
                if extra_actions {
                    let mapping = if stage == 0 {
                        &config.simple[index]
                    } else {
                        &config.complex[index]
                    };
                    if alone
                        && !mapping.extras.if_alone.is_empty()
                        && self.now.0 - down_at.0 < mapping.extras.alone_timeout_ns
                    {
                        self.snapshot_actions(
                            &mapping.extras.if_alone,
                            stage + 1,
                            config,
                            next_source,
                            (&modifiers_at_down, &consumed, &halted),
                            outputs,
                        );
                    }
                    let mandatory_scope = self.hide_modifiers(&consumed);
                    self.press_actions(
                        &mapping.extras.after_key_up,
                        stage + 1,
                        config,
                        next_source,
                        (true, &halted),
                        outputs,
                    );
                    self.unhide_modifiers(mandatory_scope);
                }
            }
        }
    }
    fn press(
        &mut self,
        key: KeyId,
        source: SourceToken,
        _now: MonoTime,
        outputs: &mut Vec<OutputIntent>,
    ) {
        self.owners.entry(key).or_default().insert(source);
        self.sync_modifiers(outputs);
        self.sync_key(key, outputs);
    }
    fn release(
        &mut self,
        key: KeyId,
        source: SourceToken,
        _now: MonoTime,
        outputs: &mut Vec<OutputIntent>,
    ) {
        self.remove_owner(key, source);
        if key.is_modifier() {
            self.sync_modifiers(outputs);
        }
        self.sync_key(key, outputs);
    }
    fn remove_owner(&mut self, key: KeyId, source: SourceToken) {
        if let Some(owners) = self.owners.get_mut(&key) {
            owners.remove(&source);
            if owners.is_empty() {
                self.owners.remove(&key);
            }
        }
        self.hidden.remove(&(key, source));
    }
    fn hide_modifiers(&mut self, keys: &BTreeSet<KeyId>) -> Vec<(KeyId, SourceToken)> {
        let mut suppressed = Vec::new();
        for key in keys {
            for source in self.owners.get(key).into_iter().flatten() {
                if !self.hidden.contains_key(&(*key, *source)) {
                    suppressed.push((*key, *source));
                }
            }
        }
        for owner in &suppressed {
            self.hidden.insert(*owner, 1);
        }
        self.suppression_references += suppressed.len();
        suppressed
    }
    fn unhide_modifiers(&mut self, suppressed: Vec<(KeyId, SourceToken)>) {
        self.suppression_references -= suppressed.len();
        for owner in suppressed {
            self.hidden.remove(&owner);
        }
    }
    fn sync_modifiers(&mut self, outputs: &mut Vec<OutputIntent>) {
        for usage in 0xe0..=0xe7 {
            self.sync_key(
                KeyId {
                    page: KEYBOARD_PAGE,
                    usage,
                },
                outputs,
            );
        }
    }
    fn sync_pointer_modifiers(&mut self, outputs: &mut Vec<OutputIntent>) {
        if self.repeating_key.is_none() && !self.emitted.iter().any(|key| key.page == BUTTON_PAGE) {
            self.sync_modifiers(outputs);
        }
    }
    fn visible(&self, key: KeyId) -> bool {
        self.owners.get(&key).is_some_and(|owners| {
            owners
                .iter()
                .any(|source| !self.hidden.contains_key(&(key, *source)))
        })
    }
    fn current_modifiers(&self) -> BTreeSet<KeyId> {
        (0xe0..=0xe7)
            .map(|usage| KeyId {
                page: KEYBOARD_PAGE,
                usage,
            })
            .filter(|key| self.visible(*key))
            .collect()
    }
    fn sync_key(&mut self, key: KeyId, outputs: &mut Vec<OutputIntent>) {
        let down = self.visible(key);
        let changed = if down {
            self.emitted.insert(key)
        } else {
            self.emitted.remove(&key)
        };
        if changed {
            if !key.is_modifier() {
                if down {
                    self.repeating_key = Some(key);
                } else if self.repeating_key == Some(key) {
                    self.repeating_key = None;
                }
            }
            outputs.push(OutputIntent {
                at_ns: self.now,
                key,
                edge: if down { KeyEdge::Down } else { KeyEdge::Up },
            });
        }
    }
}
