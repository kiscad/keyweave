//! Deterministic remapping. No OS input, file IO, threads or wall clock.
mod modifiers;
use keyweave_types::*;
pub use modifiers::{Modifier, ModifierFilter};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use thiserror::Error;

pub const MAX_BINDINGS: usize = 4096;
pub const MAX_OUTPUTS_PER_EVENT: usize = 1024;
pub const MAX_OUTPUT_OWNERS: usize = 16_384;
pub const MAX_DEVICE_IDENTITIES: usize = 4096;
pub const MAX_PERSISTENT_SUPPRESSION_REFERENCES: usize = 16_384;

#[derive(Clone, Debug)]
pub struct Mapping {
    pub id: String,
    pub from: KeyId,
    pub to: Vec<KeyId>,
    pub allow_extra_modifiers: bool,
    pub extras: MappingExtras,
}

#[derive(Clone, Debug)]
pub struct MappingExtras {
    pub from_modifiers: ModifierFilter,
    pub if_alone: Vec<KeyId>,
    pub after_key_up: Vec<KeyId>,
    pub alone_timeout_ns: u64,
}
impl Default for MappingExtras {
    fn default() -> Self {
        Self {
            from_modifiers: ModifierFilter::default(),
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
                    + usize::from(!self.extras.after_key_up.is_empty()))
    }
    fn actions(&self) -> impl Iterator<Item = KeyId> + '_ {
        self.to
            .iter()
            .chain(&self.extras.if_alone)
            .chain(&self.extras.after_key_up)
            .copied()
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
    /// Upper bound independent of modifier changes within a sequence. This is
    /// checked before changing ownership so resource failure cannot drop ups.
    pub fn expansion_bounds(&self) -> BTreeMap<KeyId, usize> {
        let mut complex = BTreeMap::<KeyId, usize>::new();
        for mapping in &self.complex {
            let count = mapping
                .actions()
                .count()
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
    Suppressed,
    Rule {
        stage: usize,
        index: usize,
        children: Vec<Bound>,
        down_at: MonoTime,
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
}

#[derive(Clone, Copy)]
struct DeviceState {
    generation: u64,
    removed: bool,
}

pub struct Engine {
    config: Arc<ConfigSnapshot>,
    bounds: BTreeMap<KeyId, usize>,
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
    pub fn new(config: Arc<ConfigSnapshot>) -> Self {
        Self {
            may_consume_modifiers: config
                .simple
                .iter()
                .chain(&config.complex)
                .any(|mapping| !mapping.extras.from_modifiers.mandatory.is_empty()),
            suppression_references: 0,
            bounds: config.expansion_bounds(),
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
        }
    }

    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), EngineError> {
        if config.generation <= self.config.generation {
            return Err(EngineError::InvalidGeneration);
        }
        self.bounds = config.expansion_bounds();
        self.may_consume_modifiers = config
            .simple
            .iter()
            .chain(&config.complex)
            .any(|mapping| !mapping.extras.from_modifiers.mandatory.is_empty());
        self.config = config;
        Ok(())
    }

    pub fn state(&self) -> EngineState {
        EngineState {
            bindings: self.active.len(),
            output_owners: self.owners.values().map(BTreeSet::len).sum(),
            pressed_outputs: self.emitted.iter().copied().collect(),
            binding_generations: self.active.values().map(|b| b.generation).collect(),
            pending_deadlines: 0,
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
        self.now = input.at_ns;
        self.last_seq = Some(input.seq);
        let mut step = EngineStep {
            capture: CaptureDecision::Capture,
            outputs: Vec::new(),
            next_deadline: None,
        };
        if input.provenance != Provenance::Physical {
            step.capture = CaptureDecision::Pass;
            return Ok(step);
        }
        match input.event {
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
                if edge == KeyEdge::Down && !self.active.contains_key(&identity) {
                    let bound = self.bounds.get(&key).copied().unwrap_or(1);
                    if self.active.len() >= MAX_BINDINGS
                        || bound.saturating_mul(2) > MAX_OUTPUTS_PER_EVENT
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
                        let mut next_source = self.next_source;
                        self.next_source += bound as u64;
                        self.reservations += bound;
                        let config = Arc::clone(&self.config);
                        let held =
                            self.press_stage(key, 0, &config, &mut next_source, &mut step.outputs);
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
                                binding.bound,
                                &binding.config,
                                &mut binding.next_source,
                                true,
                                &mut step.outputs,
                            );
                            self.reservations -= binding.reservation;
                        }
                    }
                    KeyEdge::Repeat => {
                        // Physical repeats never create ownership or duplicate a tap/macro.
                        // Platform backends choose a single OS-driven repeat source.
                    }
                }
            }
        }
        Ok(step)
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
        key: KeyId,
        stage: usize,
        config: &ConfigSnapshot,
        next_source: &mut u64,
        outputs: &mut Vec<OutputIntent>,
    ) -> Bound {
        let modifiers_at_down = self.current_modifiers();
        let mapping = match stage {
            0 => config.simple.iter().enumerate().find(|(_, mapping)| {
                mapping.from == key
                    && mapping
                        .extras
                        .from_modifiers
                        .consumed(&modifiers_at_down, mapping.allow_extra_modifiers)
                        .is_some()
            }),
            1 => config.complex.iter().enumerate().find(|(_, mapping)| {
                mapping.from == key
                    && mapping
                        .extras
                        .from_modifiers
                        .consumed(&modifiers_at_down, mapping.allow_extra_modifiers)
                        .is_some()
            }),
            _ => {
                if key.usage == 0 {
                    return Bound::Suppressed;
                }
                // The physical down reserved tokens for every possible down/up
                // action across both stages, including later alone/up actions.
                let source = SourceToken(*next_source);
                *next_source += 1;
                self.press(key, source, self.now, outputs);
                return Bound::Leaf(OwnedKey { key, source });
            }
        };
        let Some((index, mapping)) = mapping else {
            return self.press_stage(key, stage + 1, config, next_source, outputs);
        };
        let consumed = mapping
            .extras
            .from_modifiers
            .consumed(&modifiers_at_down, mapping.allow_extra_modifiers)
            .unwrap_or_default();
        let mut suppressed = self.hide_modifiers(&consumed);
        let children =
            self.press_actions(&mapping.to, stage + 1, config, next_source, false, outputs);
        if !mapping.to.last().is_some_and(|key| key.is_modifier()) {
            self.unhide_modifiers(std::mem::take(&mut suppressed));
        }
        Bound::Rule {
            stage,
            index,
            children,
            down_at: self.now,
            alone: true,
            modifiers_at_down,
            consumed,
            suppressed,
        }
    }

    fn press_actions(
        &mut self,
        actions: &[KeyId],
        stage: usize,
        config: &ConfigSnapshot,
        next_source: &mut u64,
        tap_all: bool,
        outputs: &mut Vec<OutputIntent>,
    ) -> Vec<Bound> {
        let mut held = Vec::new();
        for (index, key) in actions.iter().copied().enumerate() {
            let child = self.press_stage(key, stage, config, next_source, outputs);
            if !tap_all && index + 1 == actions.len() {
                held.push(child);
            } else {
                self.release_bound(child, config, next_source, true, outputs);
            }
        }
        held
    }

    fn release_bound(
        &mut self,
        bound: Bound,
        config: &ConfigSnapshot,
        next_source: &mut u64,
        extra_actions: bool,
        outputs: &mut Vec<OutputIntent>,
    ) {
        match bound {
            Bound::Leaf(owned) => self.release(owned.key, owned.source, self.now, outputs),
            Bound::Suppressed => {}
            Bound::Rule {
                stage,
                index,
                children,
                down_at,
                alone,
                modifiers_at_down,
                consumed,
                suppressed,
            } => {
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
                        let targets = self
                            .current_modifiers()
                            .difference(&modifiers_at_down)
                            .copied()
                            .collect();
                        let suppressed = self.hide_modifiers(&targets);
                        for key in &targets {
                            self.sync_key(*key, outputs);
                        }
                        let mut temporary = Vec::new();
                        for key in modifiers_at_down
                            .difference(&self.current_modifiers())
                            .copied()
                            .collect::<Vec<_>>()
                        {
                            let source = SourceToken(*next_source);
                            *next_source += 1;
                            self.press(key, source, self.now, outputs);
                            temporary.push(OwnedKey { key, source });
                        }
                        let mandatory_scope = self.hide_modifiers(&consumed);
                        self.press_actions(
                            &mapping.extras.if_alone,
                            stage + 1,
                            config,
                            next_source,
                            true,
                            outputs,
                        );
                        self.unhide_modifiers(mandatory_scope);
                        for owned in temporary.into_iter().rev() {
                            self.release(owned.key, owned.source, self.now, outputs);
                        }
                        self.unhide_modifiers(suppressed);
                        if !targets.is_empty() {
                            self.sync_modifiers(outputs);
                        }
                    }
                    let mandatory_scope = self.hide_modifiers(&consumed);
                    self.press_actions(
                        &mapping.extras.after_key_up,
                        stage + 1,
                        config,
                        next_source,
                        true,
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
        if let Some(owners) = self.owners.get_mut(&key) {
            owners.remove(&source);
            if owners.is_empty() {
                self.owners.remove(&key);
            }
        }
        self.hidden.remove(&(key, source));
        if key.is_modifier() {
            self.sync_modifiers(outputs);
        }
        self.sync_key(key, outputs);
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
