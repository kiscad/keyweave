//! Deterministic remapping. No OS input, file IO, threads or wall clock.
use keyweave_types::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use thiserror::Error;

pub const MAX_BINDINGS: usize = 4096;
pub const MAX_OUTPUTS_PER_EVENT: usize = 1024;
pub const MAX_OUTPUT_OWNERS: usize = 16_384;

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
    pub if_alone: Vec<KeyId>,
    pub after_key_up: Vec<KeyId>,
    pub alone_timeout_ns: u64,
}
impl Default for MappingExtras {
    fn default() -> Self {
        Self {
            if_alone: Vec::new(),
            after_key_up: Vec::new(),
            alone_timeout_ns: 1_000_000_000,
        }
    }
}
impl Mapping {
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
                .saturating_add(if mapping.extras.if_alone.is_empty() {
                    0
                } else {
                    8
                })
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
                .saturating_add(if mapping.extras.if_alone.is_empty() {
                    0
                } else {
                    8
                });
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
    #[error("configuration generation must increase")]
    InvalidGeneration,
}

pub struct Engine {
    config: Arc<ConfigSnapshot>,
    bounds: BTreeMap<KeyId, usize>,
    reservations: usize,
    alone_candidate: Option<(DeviceInstanceId, KeyId)>,
    active: BTreeMap<(DeviceInstanceId, KeyId), ActiveBinding>,
    owners: BTreeMap<KeyId, BTreeSet<SourceToken>>,
    hidden: BTreeMap<(KeyId, SourceToken), usize>,
    emitted: BTreeSet<KeyId>,
    latest_devices: BTreeMap<u64, u64>,
    retired: BTreeSet<DeviceInstanceId>,
    last_seq: Option<u64>,
    now: MonoTime,
    next_source: u64,
}

impl Engine {
    pub fn new(config: Arc<ConfigSnapshot>) -> Self {
        Self {
            bounds: config.expansion_bounds(),
            reservations: 0,
            alone_candidate: None,
            config,
            active: BTreeMap::new(),
            owners: BTreeMap::new(),
            hidden: BTreeMap::new(),
            emitted: BTreeSet::new(),
            latest_devices: BTreeMap::new(),
            retired: BTreeSet::new(),
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
                self.retired.insert(input.device);
                self.release_device(input.device, input.at_ns, &mut step.outputs);
            }
            InputEvent::Wheel {
                horizontal,
                vertical,
            } => {
                step.capture = CaptureDecision::Pass;
                if horizontal != 0 || vertical != 0 {
                    self.cancel_alone();
                }
            }
            InputEvent::PointerMove { .. } => step.capture = CaptureDecision::Pass,
            InputEvent::Key { key, edge } => {
                if self.retired.contains(&input.device)
                    || self
                        .latest_devices
                        .get(&input.device.id)
                        .is_some_and(|generation| *generation > input.device.generation)
                {
                    return Err(EngineError::RetiredDevice);
                }
                let identity = (input.device, key);
                if edge == KeyEdge::Down && !self.active.contains_key(&identity) {
                    let bound = self.bounds.get(&key).copied().unwrap_or(1);
                    if self.active.len() >= MAX_BINDINGS
                        || bound.saturating_mul(2) > MAX_OUTPUTS_PER_EVENT
                        || self.reservations.saturating_add(bound) > MAX_OUTPUT_OWNERS
                        || self.next_source.checked_add(bound as u64).is_none()
                    {
                        return Err(EngineError::ResourceLimit);
                    }
                }
                if self
                    .latest_devices
                    .get(&input.device.id)
                    .is_some_and(|generation| *generation < input.device.generation)
                {
                    let old: Vec<_> = self
                        .active
                        .keys()
                        .map(|(dev, _)| *dev)
                        .filter(|dev| dev.id == input.device.id && *dev != input.device)
                        .collect();
                    for dev in old {
                        self.retired.insert(dev);
                        self.release_device(dev, input.at_ns, &mut step.outputs);
                    }
                }
                self.latest_devices
                    .insert(input.device.id, input.device.generation);
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
        self.active.clear();
        self.reservations = 0;
        self.alone_candidate = None;
        Ok(outputs)
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
            0 => config
                .simple
                .iter()
                .enumerate()
                .find(|(_, mapping)| mapping.from == key),
            1 => config.complex.iter().enumerate().find(|(_, mapping)| {
                mapping.from == key
                    && (mapping.allow_extra_modifiers || modifiers_at_down.is_empty())
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
        let children =
            self.press_actions(&mapping.to, stage + 1, config, next_source, false, outputs);
        Bound::Rule {
            stage,
            index,
            children,
            down_at: self.now,
            alone: true,
            modifiers_at_down,
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
            } => {
                for child in children.into_iter().rev() {
                    self.release_bound(child, config, next_source, extra_actions, outputs);
                }
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
                        let mut suppressed = Vec::new();
                        for key in self
                            .current_modifiers()
                            .difference(&modifiers_at_down)
                            .copied()
                            .collect::<Vec<_>>()
                        {
                            for source in self
                                .owners
                                .get(&key)
                                .into_iter()
                                .flatten()
                                .copied()
                                .collect::<Vec<_>>()
                            {
                                *self.hidden.entry((key, source)).or_default() += 1;
                                suppressed.push((key, source));
                            }
                            self.sync_key(key, outputs);
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
                        self.press_actions(
                            &mapping.extras.if_alone,
                            stage + 1,
                            config,
                            next_source,
                            true,
                            outputs,
                        );
                        for owned in temporary.into_iter().rev() {
                            self.release(owned.key, owned.source, self.now, outputs);
                        }
                        for (key, source) in suppressed {
                            if let Some(depth) = self.hidden.get_mut(&(key, source)) {
                                *depth -= 1;
                                if *depth == 0 {
                                    self.hidden.remove(&(key, source));
                                }
                            }
                            self.sync_key(key, outputs);
                        }
                    }
                    self.press_actions(
                        &mapping.extras.after_key_up,
                        stage + 1,
                        config,
                        next_source,
                        true,
                        outputs,
                    );
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
        self.sync_key(key, outputs);
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
            outputs.push(OutputIntent {
                at_ns: self.now,
                key,
                edge: if down { KeyEdge::Down } else { KeyEdge::Up },
            });
        }
    }
}
