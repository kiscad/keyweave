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
    pub fn expansion_bound(&self, key: KeyId) -> usize {
        let simple = self
            .simple
            .iter()
            .find(|m| m.from == key)
            .map(|m| m.to.clone())
            .unwrap_or_else(|| vec![key]);
        simple
            .into_iter()
            .map(|key| {
                self.complex
                    .iter()
                    .filter(|m| m.from == key)
                    .map(|m| m.to.len())
                    .max()
                    .unwrap_or(1)
                    .max(1)
            })
            .fold(0, usize::saturating_add)
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
    outputs: Vec<OwnedKey>,
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
    active: BTreeMap<(DeviceInstanceId, KeyId), ActiveBinding>,
    owners: BTreeMap<KeyId, BTreeSet<SourceToken>>,
    latest_devices: BTreeMap<u64, u64>,
    retired: BTreeSet<DeviceInstanceId>,
    last_seq: Option<u64>,
    now: MonoTime,
    next_source: u64,
}

impl Engine {
    pub fn new(config: Arc<ConfigSnapshot>) -> Self {
        Self {
            config,
            active: BTreeMap::new(),
            owners: BTreeMap::new(),
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
        self.config = config;
        Ok(())
    }

    pub fn state(&self) -> EngineState {
        EngineState {
            bindings: self.active.len(),
            output_owners: self.owners.values().map(BTreeSet::len).sum(),
            pressed_outputs: self.owners.keys().copied().collect(),
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
            InputEvent::Key { key, edge } => {
                if self.retired.contains(&input.device)
                    || self
                        .latest_devices
                        .get(&input.device.id)
                        .is_some_and(|generation| *generation > input.device.generation)
                {
                    return Err(EngineError::RetiredDevice);
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
                let identity = (input.device, key);
                match edge {
                    KeyEdge::Down => {
                        if self.active.contains_key(&identity) {
                            return Ok(step);
                        }
                        if self.active.len() >= MAX_BINDINGS {
                            return Err(EngineError::ResourceLimit);
                        }
                        let bound = self.config.expansion_bound(key);
                        let owners: usize = self.owners.values().map(BTreeSet::len).sum();
                        if bound.saturating_mul(2) > MAX_OUTPUTS_PER_EVENT
                            || owners.saturating_add(bound) > MAX_OUTPUT_OWNERS
                            || self.next_source.checked_add(bound as u64).is_none()
                        {
                            return Err(EngineError::ResourceLimit);
                        }
                        let config = Arc::clone(&self.config);
                        let held = self.press_stage(key, 0, &config, &mut step.outputs);
                        self.active.insert(
                            identity,
                            ActiveBinding {
                                generation: self.config.generation,
                                outputs: held,
                            },
                        );
                    }
                    KeyEdge::Up => {
                        if let Some(binding) = self.active.remove(&identity) {
                            for output in binding.outputs.into_iter().rev() {
                                self.release(
                                    output.key,
                                    output.source,
                                    input.at_ns,
                                    &mut step.outputs,
                                );
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
                .owners
                .keys()
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
        self.active.clear();
        Ok(outputs)
    }

    fn release_device(
        &mut self,
        device: DeviceInstanceId,
        now: MonoTime,
        outputs: &mut Vec<OutputIntent>,
    ) {
        let keys: Vec<_> = self
            .active
            .keys()
            .filter(|(d, _)| *d == device)
            .copied()
            .collect();
        for identity in keys {
            if let Some(binding) = self.active.remove(&identity) {
                for output in binding.outputs.into_iter().rev() {
                    self.release(output.key, output.source, now, outputs);
                }
            }
        }
    }
    fn press_stage(
        &mut self,
        key: KeyId,
        stage: usize,
        config: &ConfigSnapshot,
        outputs: &mut Vec<OutputIntent>,
    ) -> Vec<OwnedKey> {
        // Each phase consumes complete down/up pairs from the preceding phase.
        // Flattening the two `to` arrays would incorrectly hold the last output
        // of an intermediate tap until the original physical key goes up.
        let mapping = match stage {
            0 => config.simple.iter().find(|mapping| mapping.from == key),
            1 => config.complex.iter().find(|mapping| {
                mapping.from == key
                    && (mapping.allow_extra_modifiers
                        || !self.owners.keys().any(|key| key.is_modifier()))
            }),
            _ => {
                if key.usage == 0 {
                    return Vec::new();
                }
                // handle() has reserved a conservative token/owner budget.
                let source = SourceToken(self.next_source);
                self.next_source += 1;
                self.press(key, source, self.now, outputs);
                return vec![OwnedKey { key, source }];
            }
        };
        let Some(mapping) = mapping else {
            return self.press_stage(key, stage + 1, config, outputs);
        };
        let mut held = Vec::new();
        for (index, key) in mapping.to.iter().copied().enumerate() {
            let children = self.press_stage(key, stage + 1, config, outputs);
            if index + 1 == mapping.to.len() {
                held = children;
            } else {
                for child in children.into_iter().rev() {
                    self.release(child.key, child.source, self.now, outputs);
                }
            }
        }
        held
    }
    fn press(
        &mut self,
        key: KeyId,
        source: SourceToken,
        now: MonoTime,
        outputs: &mut Vec<OutputIntent>,
    ) {
        let owners = self.owners.entry(key).or_default();
        if owners.insert(source) && owners.len() == 1 {
            outputs.push(OutputIntent {
                at_ns: now,
                key,
                edge: KeyEdge::Down,
            });
        }
    }
    fn release(
        &mut self,
        key: KeyId,
        source: SourceToken,
        now: MonoTime,
        outputs: &mut Vec<OutputIntent>,
    ) {
        if let Some(owners) = self.owners.get_mut(&key)
            && owners.remove(&source)
            && owners.is_empty()
        {
            self.owners.remove(&key);
            outputs.push(OutputIntent {
                at_ns: now,
                key,
                edge: KeyEdge::Up,
            });
        }
    }
}
