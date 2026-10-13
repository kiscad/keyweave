//! Explicit-clock, bounded IO coordinator. Does not open devices or spawn threads.
mod machine;
use crate::{
    capture::{CaptureDevice, CaptureError, CapturePhase, CaptureSource},
    emergency::{EmergencyChord, EmergencyError},
    events::{self, Event, FrameDecoder, Notice, ProtocolError},
    keys::{self, IdentityPolicy, KeyError, KeyNormalizer, NormalizedKey},
    output::{DeviceSpec, RepeatPolicy},
    pump::{FrameBackend, OutputPump, Phase, PumpError},
    read::ReadBatch,
};
use keyweave_engine::{ConfigSnapshot, Engine, EngineError, EngineState};
use keyweave_types::*;
use machine::Machine;
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    Legacy,
    StagedKeyboard,
}
impl ExecutionMode {
    /// Pure snapshot validation, independent of actual input/output readiness.
    pub fn validate_snapshot(self, config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
        match self {
            Self::Legacy => {
                Engine::new(config)?;
            }
            Self::StagedKeyboard => {
                keyweave_engine::physical_controller::PhysicalStageController::new(config)?;
            }
        }
        Ok(())
    }
}
pub const MAX_DEVICES: usize = 64;
pub const MAX_WORK_PER_POLL: usize = 64;
pub const MAX_RECOVERY_POLLS: usize = 8;
const DEVICE_CONTEXT_LIFETIME_NS: u64 = 3_600_000_000_000;
const DEVICE_CONTEXT_REFRESH_NS: u64 = DEVICE_CONTEXT_LIFETIME_NS / 2;
pub use keyweave_progress::ProgressPhase as RuntimePhase;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Pause,
    Emergency,
    InputFault,
    InputLost,
    EngineFault,
    OutputFault,
    ClockFault,
    ControlFault,
    NoDevices,
    EmergencyUnavailable,
}
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Emergency(#[from] EmergencyError),
    #[error("no captured device can produce the emergency chord")]
    EmergencyUnavailable,
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Staged(#[from] keyweave_engine::physical_controller::PhysicalControllerError),
    #[error("keyboard runtime does not support staged pointer input or delivery")]
    StagedPointer,
    #[error(transparent)]
    Output(#[from] PumpError),
    #[error("invalid startup resources or duplicate device identity")]
    InvalidStartup,
    #[error("runtime requires kernel-driven repeat")]
    RepeatPolicy,
    #[error("configuration key cannot be emitted: {0}")]
    ConfigurationKey(KeyId),
    #[error("native input record violates advertised capabilities")]
    InvalidRecord,
    #[error("native input state was lost")]
    InputLost,
    #[error("input source closed")]
    InputClosed,
    #[error("output frame is incomplete")]
    IncompleteOutput,
    #[error("clock moved backwards")]
    ClockBackwards,
    #[error("runtime is stopped")]
    Stopped,
    #[error("sequence or progress identity exhausted")]
    IdentityExhausted,
    #[error("device context validity would overflow monotonic time")]
    ContextTimeOverflow,
    #[error("device instance is unknown, retired or not captured")]
    DeviceNotCaptured,
    #[error("physical key was unexpectedly bypassed by engine")]
    UnexpectedPass,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollReport {
    pub phase: RuntimePhase,
    pub work: usize,
    pub pending: bool,
    pub progress_epoch: u64,
}
/// Control transaction status, not proof that queued output ups were accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemovalReport {
    pub device: DeviceInstanceId,
    pub remaining_devices: usize,
    pub phase: RuntimePhase,
}
/// Snapshot taken before ledger destruction. Ownership release is not proof
/// of compositor processing, native ungrab success or global key recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminationReport {
    pub reason: StopReason,
    pub input_owners_released: bool,
    pub engine_reset_confirmed: bool,
    pub recovery_failed: bool,
    pub recovery_budget_exhausted: bool,
    pub submitted_remaining: bool,
    pub report_pending: bool,
    pub acceptance_unknown: bool,
    pub output_recovery_confirmed: bool,
    pub output_owner_dropped: bool,
    pub backend_transition_confirmed: bool,
}
struct Input<S: CaptureSource> {
    id: DeviceInstanceId,
    capture: CaptureDevice<S>,
    decoder: FrameDecoder,
    normalizer: KeyNormalizer,
    emergency: EmergencyChord,
}
pub struct Runtime<S: CaptureSource, B: FrameBackend> {
    inputs: Vec<Input<S>>,
    output: OutputPump<B>,
    engine: Machine,
    raw: VecDeque<(usize, Event)>,
    logical: Option<(usize, InputEvent)>,
    cursor: usize,
    seq: u64,
    now: MonoTime,
    progress_epoch: u64,
    phase: RuntimePhase,
    reason: Option<StopReason>,
    recovery_polls: usize,
    shutdown_engine_reset_ok: bool,
    shutdown_recovery_failed: bool,
    termination: Option<TerminationReport>,
    captured_devices: Vec<DeviceContext>,
    context_revision: u64,
    context_refresh_at: MonoTime,
}
/// Check all output branches before bootstrap grabs input and before hot activation.
pub fn validate_config(config: &ConfigSnapshot, spec: &DeviceSpec) -> Result<(), RuntimeError> {
    let codes: BTreeSet<_> = spec.codes().collect();
    for key in config
        .simple
        .iter()
        .chain(&config.complex)
        .flat_map(|mapping| mapping.actions())
        .flat_map(|action| {
            action
                .key()
                .into_iter()
                .chain(action.modifiers.iter().copied())
        })
    {
        if !keys::output_code(key).is_ok_and(|code| codes.contains(&code)) {
            return Err(RuntimeError::ConfigurationKey(key));
        }
    }
    Ok(())
}
/// Cold keyboard/output validation only; no capture or consumer-ready proof.
pub fn preflight_staged_config(
    config: Arc<ConfigSnapshot>,
    spec: &DeviceSpec,
) -> Result<(), RuntimeError> {
    if spec.repeat() != RepeatPolicy::Kernel {
        return Err(RuntimeError::RepeatPolicy);
    }
    validate_config(&config, spec)?;
    Machine::staged(config, &[]).map(drop)
}
impl<S: CaptureSource, B: FrameBackend> Runtime<S, B> {
    /// Takes ownership of already armed inputs. Caller must finish bootstrap
    /// readiness and neutral policy before constructing this coordinator.
    pub fn new(
        inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        backend: B,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_mode(inputs, backend, config, ExecutionMode::Legacy)
    }
    /// Explicit staged keyboard mode; native bootstrap/worker do not select it yet.
    pub fn new_staged(
        inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        backend: B,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_mode(inputs, backend, config, ExecutionMode::StagedKeyboard)
    }
    pub fn new_with_mode(
        mut inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        backend: B,
        config: Arc<ConfigSnapshot>,
        mode: ExecutionMode,
    ) -> Result<Self, RuntimeError> {
        let prepared = (|| {
            if inputs.is_empty() || inputs.len() > MAX_DEVICES {
                return Err(RuntimeError::InvalidStartup);
            }
            let output_path = backend
                .input_syspath()
                .ok_or(RuntimeError::InvalidStartup)?;
            let mut ids = BTreeSet::new();
            let mut paths = BTreeSet::new();
            for (id, input) in &inputs {
                if input.phase() != CapturePhase::Captured
                    || input.output_syspath() != Some(output_path)
                    || input
                        .probe()
                        .is_some_and(|probe| probe.input_syspath == output_path)
                    || !ids.insert(id.id)
                    || !input
                        .probe()
                        .is_some_and(|probe| paths.insert(probe.input_syspath.clone()))
                {
                    return Err(RuntimeError::InvalidStartup);
                }
                let codes: BTreeSet<_> = backend.spec().codes().collect();
                if input.probe().is_none_or(|probe| {
                    probe
                        .capabilities
                        .keys
                        .codes()
                        .any(|code| !codes.contains(&code))
                }) {
                    return Err(RuntimeError::InvalidStartup);
                }
            }
            if !inputs.iter().any(|(_, input)| {
                input
                    .probe()
                    .is_some_and(|probe| EmergencyChord::supported(&probe.capabilities.keys))
            }) {
                return Err(RuntimeError::EmergencyUnavailable);
            }
            if backend.spec().repeat() != RepeatPolicy::Kernel {
                return Err(RuntimeError::RepeatPolicy);
            }
            validate_config(&config, backend.spec())?;
            let engine = if mode == ExecutionMode::StagedKeyboard {
                let devices: Vec<_> = inputs.iter().map(|(id, _)| *id).collect();
                Machine::staged(config, &devices)?
            } else {
                Machine::Legacy(Box::new(Engine::new(config)?))
            };
            // Copy only verified metadata; names/bus/path do not prove other fields.
            let captured_devices = inputs
                .iter()
                .map(|(device, input)| {
                    let probe = input.probe().ok_or(RuntimeError::InvalidStartup)?;
                    Ok(DeviceContext {
                        device: *device,
                        properties: DeviceProperties {
                            vendor_id: Some(u32::from(probe.id.vendor)),
                            product_id: Some(u32::from(probe.id.product)),
                            ..Default::default()
                        },
                    })
                })
                .collect::<Result<Vec<_>, RuntimeError>>()?;
            Ok((engine, captured_devices))
        })();
        let (engine, captured_devices) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                for (_, input) in &mut inputs {
                    input.close();
                }
                drop(backend);
                return Err(error);
            }
        };
        let mut output = OutputPump::new(backend);
        if let Err(error) = output.resume() {
            for (_, input) in &mut inputs {
                input.close();
            }
            output.destroy_backend()?;
            return Err(error.into());
        }
        Ok(Self {
            inputs: inputs
                .into_iter()
                .map(|(id, capture)| Input {
                    id,
                    capture,
                    decoder: FrameDecoder::new(),
                    normalizer: KeyNormalizer::new(IdentityPolicy::CanonicalEvdev),
                    emergency: EmergencyChord::new(),
                })
                .collect(),
            output,
            engine,
            raw: VecDeque::new(),
            logical: None,
            cursor: 0,
            seq: 0,
            now: MonoTime(0),
            progress_epoch: 0,
            phase: RuntimePhase::Active,
            reason: None,
            recovery_polls: 0,
            shutdown_engine_reset_ok: false,
            shutdown_recovery_failed: false,
            termination: None,
            captured_devices,
            context_revision: 0,
            context_refresh_at: MonoTime(0),
        })
    }
    pub fn phase(&self) -> RuntimePhase {
        self.phase
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn backend(&self) -> Option<&B> {
        self.output.backend()
    }
    pub fn stop_reason(&self) -> Option<StopReason> {
        self.reason
    }
    /// None until explicit termination has released the output owner. Repeated
    /// reads retain pre-destruction facts, independent of cleared submitted state.
    pub fn termination_report(&self) -> Option<TerminationReport> {
        self.termination
    }
    pub fn progress_epoch(&self) -> u64 {
        self.progress_epoch
    }
    /// Metadata only; does not constitute a completed poll or readiness grant.
    pub fn execution_mode(&self) -> ExecutionMode {
        self.engine.execution_mode()
    }
    pub fn config_generation(&self) -> ConfigGeneration {
        self.engine.config_generation()
    }

    /// Model diagnostics, separate from native submitted state. Staged mode
    /// counts queued stage work/timers rather than legacy DeadlineTokens.
    pub fn engine_state(&self) -> EngineState {
        self.engine.state()
    }
    pub fn submitted(&self) -> &BTreeSet<KeyId> {
        self.output.ledger().submitted()
    }
    pub fn pending_keys(&self) -> usize {
        self.raw.len() + usize::from(self.logical.is_some())
    }
    pub fn next_wakeup(&self) -> Option<MonoTime> {
        match self.phase {
            RuntimePhase::Stopped => None,
            RuntimePhase::Recovering => Some(self.now),
            RuntimePhase::Active if self.pending_keys() != 0 => Some(self.now),
            RuntimePhase::Active => self
                .engine
                .next_deadline()
                .into_iter()
                .chain(self.output.ledger().next_deadline())
                .chain(Some(self.context_refresh_at))
                .chain(
                    self.inputs
                        .iter()
                        .filter_map(|input| input.emergency.next_deadline()),
                )
                .min(),
        }
    }
    fn dispatch(&mut self, index: usize, event: InputEvent) -> Result<(), RuntimeError> {
        let seq = self
            .seq
            .checked_add(1)
            .ok_or(RuntimeError::IdentityExhausted)?;
        let key_event = matches!(event, InputEvent::Key { .. });
        let step = self.engine.handle(InputEnvelope {
            seq,
            at_ns: self.now,
            device: self.inputs[index].id,
            provenance: Provenance::Physical,
            event,
        })?;
        self.seq = seq;
        if key_event && step.capture != CaptureDecision::Capture {
            return Err(RuntimeError::UnexpectedPass);
        }
        self.output.enqueue(&step.outputs)?;
        Ok(())
    }
    fn refresh_device_context(&mut self) -> Result<bool, RuntimeError> {
        if self.now < self.context_refresh_at {
            return Ok(false);
        }
        let revision = self
            .context_revision
            .checked_add(1)
            .ok_or(RuntimeError::IdentityExhausted)?;
        let valid_until_ns = self
            .now
            .checked_add(DEVICE_CONTEXT_LIFETIME_NS)
            .ok_or(RuntimeError::ContextTimeOverflow)?;
        let refresh_at = self
            .now
            .checked_add(DEVICE_CONTEXT_REFRESH_NS)
            .ok_or(RuntimeError::ContextTimeOverflow)?;
        let context = ContextSnapshot {
            revision,
            provider: "linux-owned-capture".into(),
            observed_at_ns: self.now,
            valid_until_ns,
            devices: Some(self.captured_devices.clone()),
            device_inventory_scope: DeviceInventoryScope::Partial,
            application: None,
            input_source: None,
        };
        self.dispatch(0, InputEvent::ContextChanged { context })?;
        self.context_revision = revision;
        self.context_refresh_at = refresh_at;
        Ok(true)
    }
    fn validate_frame(&self, index: usize, frame: &[Event]) -> Result<(), RuntimeError> {
        let caps = &self.inputs[index]
            .capture
            .probe()
            .ok_or(RuntimeError::InputClosed)?
            .capabilities;
        for event in frame {
            if !caps.events.contains(event.kind) {
                return Err(RuntimeError::InvalidRecord);
            }
            let valid = match event.kind {
                events::EV_KEY => caps.keys.contains(event.code) && (0..=2).contains(&event.value),
                4 => caps.misc.contains(event.code) && matches!(event.code, 4 | 5), // no HID inference from MSC
                17 => caps.leds.contains(event.code) && (0..=1).contains(&event.value),
                20 => event.code <= 1 && event.value >= 0, // physical repeat parameters are metadata
                _ => false,
            };
            if !valid {
                return Err(RuntimeError::InvalidRecord);
            }
        }
        Ok(())
    }
    fn read_one(&mut self) -> Result<(), RuntimeError> {
        // Closed tombstones retain stable indices for every buffered event.
        let index = (0..self.inputs.len())
            .map(|offset| (self.cursor + offset) % self.inputs.len())
            .find(|index| self.inputs[*index].capture.phase() == CapturePhase::Captured)
            .ok_or(RuntimeError::InputClosed)?;
        self.cursor = (index + 1) % self.inputs.len();
        match self.inputs[index].capture.read_batch()? {
            ReadBatch::Closed => Err(RuntimeError::InputClosed),
            ReadBatch::WouldBlock => Ok(()),
            ReadBatch::Events(events) => {
                let notices = self.inputs[index].decoder.ingest(&events)?;
                if notices
                    .iter()
                    .any(|notice| !matches!(notice, Notice::Frame(_)))
                {
                    return Err(RuntimeError::InputLost);
                }
                // Validate the complete read before mutating engine/normalizer state.
                for notice in &notices {
                    if let Notice::Frame(frame) = notice {
                        self.validate_frame(index, frame)?;
                    }
                }
                let count = notices
                    .iter()
                    .map(|notice| match notice {
                        Notice::Frame(frame) => frame
                            .iter()
                            .filter(|event| event.kind == events::EV_KEY)
                            .count(),
                        _ => 0,
                    })
                    .sum::<usize>();
                if self.raw.len() + count > events::MAX_NATIVE_BATCH {
                    return Err(ProtocolError::Capacity.into());
                }
                for notice in &notices {
                    if let Notice::Frame(frame) = notice {
                        for event in frame.iter().filter(|event| event.kind == events::EV_KEY) {
                            self.inputs[index].emergency.observe(
                                event.code,
                                event.value,
                                self.now,
                            )?;
                        }
                    }
                }
                for notice in notices {
                    if let Notice::Frame(frame) = notice {
                        self.raw.extend(
                            frame
                                .into_iter()
                                .filter(|event| event.kind == events::EV_KEY)
                                .map(|event| (index, event)),
                        );
                    }
                }
                Ok(())
            }
        }
    }
    fn active_poll(&mut self) -> Result<usize, RuntimeError> {
        for input in &mut self.inputs {
            if input.emergency.advance(self.now)? {
                self.stop(StopReason::Emergency);
                return Ok(0);
            }
        }
        let mut work = usize::from(self.refresh_device_context()?);
        if self.pending_keys() == 0
            && !self
                .output
                .ledger()
                .next_deadline()
                .is_some_and(|at| at <= self.now)
        {
            self.read_one()?;
            work += 1;
        }
        while work < MAX_WORK_PER_POLL {
            if self
                .output
                .ledger()
                .next_deadline()
                .is_some_and(|at| at <= self.now)
            {
                self.output.poll(self.now)?;
                if self.output.phase() != Phase::Normal {
                    return Err(RuntimeError::IncompleteOutput);
                }
            } else if self.engine.stage_due(self.now) {
                let step = self
                    .engine
                    .poll_stage(self.now)?
                    .ok_or(RuntimeError::UnexpectedPass)?;
                self.output.enqueue(&step.outputs)?;
            } else if let Some(token) = self.engine.legacy_timer_due(self.now) {
                self.dispatch(0, InputEvent::TimerFired { token })?;
            } else if let Some((index, event)) = self.logical.clone() {
                match self.dispatch(index, event) {
                    Ok(()) => self.logical = None,
                    Err(RuntimeError::Engine(EngineError::CancellationPending)) => {
                        let token = self
                            .engine
                            .next_delayed()
                            .ok_or(EngineError::InvalidDeadline)?;
                        self.dispatch(index, InputEvent::CancelDelayed { token })?;
                    }
                    Err(error) => return Err(error),
                }
            } else if let Some((index, event)) = self.raw.pop_front() {
                if let NormalizedKey::Event { identity, edge } =
                    self.inputs[index].normalizer.process(event, None)?
                {
                    self.logical = Some((
                        index,
                        InputEvent::Key {
                            key: identity.key,
                            edge,
                        },
                    ));
                }
            } else {
                break;
            }
            work += 1;
        }
        Ok(work)
    }
    fn stop(&mut self, reason: StopReason) {
        if self.phase != RuntimePhase::Active {
            return;
        }
        // Restore physical access before any potentially failing output cleanup.
        for input in &mut self.inputs {
            input.capture.close();
            input.normalizer.reset();
            input.emergency.reset();
            input.decoder = FrameDecoder::new();
        }
        self.raw.clear();
        self.logical = None;
        self.shutdown_engine_reset_ok = self.engine.reset(self.now).is_ok(); // not an acceptance ledger
        self.reason = Some(reason);
        self.phase = RuntimePhase::Recovering;
        let begin_failed =
            self.output.phase() == Phase::Normal && self.output.begin_recovery(self.now).is_err();
        self.shutdown_recovery_failed |= begin_failed;
        if begin_failed || matches!(self.output.phase(), Phase::Quarantined | Phase::Detached) {
            let _ = self.destroy_for_stop();
            self.phase = RuntimePhase::Stopped;
        }
    }
    fn destroy_for_stop(&mut self) -> Result<(), PumpError> {
        let phase = self.output.phase();
        let submitted_remaining = !self.output.ledger().submitted().is_empty();
        let report_pending = phase == Phase::ReportPending;
        let acceptance_unknown = phase == Phase::Quarantined;
        let output_recovery_confirmed =
            phase == Phase::Recovery && !submitted_remaining && !self.shutdown_recovery_failed;
        let result = self.output.destroy_backend();
        // Preserve uncertainty even though destroy_backend clears the ledger.
        self.termination = Some(TerminationReport {
            reason: self.reason.unwrap_or(StopReason::ControlFault),
            input_owners_released: self
                .inputs
                .iter()
                .all(|input| input.capture.phase() == CapturePhase::Closed),
            engine_reset_confirmed: self.shutdown_engine_reset_ok,
            recovery_failed: self.shutdown_recovery_failed,
            recovery_budget_exhausted: self.recovery_polls >= MAX_RECOVERY_POLLS
                && !output_recovery_confirmed,
            submitted_remaining,
            report_pending,
            acceptance_unknown,
            output_recovery_confirmed,
            output_owner_dropped: self.output.spec().is_none(),
            backend_transition_confirmed: result.is_ok(),
        });
        result
    }
    fn recover_poll(&mut self) -> Result<usize, RuntimeError> {
        self.recovery_polls += 1;
        let result = self.output.poll(self.now);
        self.shutdown_recovery_failed |= result.is_err();
        if result.is_err()
            || (self.output.phase() != Phase::ReportPending
                && self.output.ledger().submitted().is_empty())
            || self.recovery_polls >= MAX_RECOVERY_POLLS
        {
            self.destroy_for_stop()?;
            self.phase = RuntimePhase::Stopped;
        }
        result?;
        Ok(1)
    }
    pub fn poll(&mut self, now: MonoTime) -> Result<PollReport, RuntimeError> {
        if self.phase == RuntimePhase::Stopped {
            return Err(RuntimeError::Stopped);
        }
        if now < self.now {
            self.stop(StopReason::ClockFault);
            return Err(RuntimeError::ClockBackwards);
        }
        if self.progress_epoch == u64::MAX {
            self.stop(StopReason::ControlFault);
            return Err(RuntimeError::IdentityExhausted);
        }
        self.now = now;
        let result = if self.phase == RuntimePhase::Active {
            self.active_poll()
        } else {
            self.recover_poll()
        };
        self.progress_epoch += 1;
        match result {
            Ok(work) => Ok(PollReport {
                phase: self.phase,
                work,
                pending: self.next_wakeup().is_some_and(|at| at <= self.now),
                progress_epoch: self.progress_epoch,
            }),
            Err(error) => {
                let reason = match &error {
                    RuntimeError::InputLost => StopReason::InputLost,
                    RuntimeError::Emergency(_)
                    | RuntimeError::IdentityExhausted
                    | RuntimeError::ContextTimeOverflow => StopReason::ControlFault,
                    RuntimeError::Capture(_)
                    | RuntimeError::Protocol(_)
                    | RuntimeError::Key(_)
                    | RuntimeError::InvalidRecord
                    | RuntimeError::InputClosed => StopReason::InputFault,
                    RuntimeError::Output(_) | RuntimeError::IncompleteOutput => {
                        StopReason::OutputFault
                    }
                    _ => StopReason::EngineFault,
                };
                self.stop(reason);
                Err(error)
            }
        }
    }
    /// Trusted lifecycle/user controller only. Match the full instance and call
    /// before ordinary poll/due work. This does not detect or authenticate unplug.
    /// No rearm or slot reuse: a new device requires separate startup admission.
    pub fn remove_device(
        &mut self,
        device: DeviceInstanceId,
        now: MonoTime,
    ) -> Result<RemovalReport, RuntimeError> {
        if self.phase != RuntimePhase::Active {
            return Err(RuntimeError::Stopped);
        }
        let index = self
            .inputs
            .iter()
            .position(|input| input.id == device && input.capture.phase() == CapturePhase::Captured)
            .ok_or(RuntimeError::DeviceNotCaptured)?;
        if now < self.now {
            self.stop(StopReason::ClockFault);
            return Err(RuntimeError::ClockBackwards);
        }
        self.now = now;
        // Physical client release precedes engine/output cleanup. Neither the
        // normalizer nor discarded undelivered down events synthesize an up.
        let input = &mut self.inputs[index];
        input.capture.close();
        input.normalizer.reset();
        input.decoder = FrameDecoder::new();
        input.emergency.reset();
        self.raw.retain(|(slot, _)| *slot != index);
        if self
            .logical
            .as_ref()
            .is_some_and(|(slot, _)| *slot == index)
        {
            self.logical = None;
        }
        self.captured_devices.retain(|entry| entry.device != device);
        self.context_refresh_at = now;
        let remaining_devices = self
            .inputs
            .iter()
            .filter(|input| input.capture.phase() == CapturePhase::Captured)
            .count();
        if remaining_devices == 0 {
            self.stop(StopReason::NoDevices);
        } else if !self.inputs.iter().any(|input| {
            input
                .capture
                .probe()
                .is_some_and(|probe| EmergencyChord::supported(&probe.capabilities.keys))
        }) {
            self.stop(StopReason::EmergencyUnavailable);
        } else if let Err(error) = self.dispatch(index, InputEvent::DeviceRemoved) {
            let reason = match &error {
                RuntimeError::Output(_) | RuntimeError::IncompleteOutput => StopReason::OutputFault,
                RuntimeError::IdentityExhausted => StopReason::ControlFault,
                _ => StopReason::EngineFault,
            };
            self.stop(reason);
            return Err(error);
        }
        Ok(RemovalReport {
            device,
            remaining_devices: self
                .inputs
                .iter()
                .filter(|input| input.capture.phase() == CapturePhase::Captured)
                .count(),
            phase: self.phase,
        })
    }
    pub fn pause(&mut self, now: MonoTime) -> Result<(), RuntimeError> {
        if now < self.now {
            self.stop(StopReason::ClockFault);
            return Err(RuntimeError::ClockBackwards);
        }
        self.now = now;
        self.stop(StopReason::Pause);
        Ok(())
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn external_fault(&mut self, reason: StopReason) {
        if self.phase == RuntimePhase::Active {
            self.stop(reason);
        }
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn finish_recovery(&mut self) {
        for _ in 0..MAX_RECOVERY_POLLS {
            if self.phase != RuntimePhase::Recovering {
                break;
            }
            let _ = self.poll(self.now);
        }
    }
    /// Admission failures leave the active configuration and bindings untouched.
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
        if self.phase != RuntimePhase::Active {
            return Err(RuntimeError::Stopped);
        }
        validate_config(&config, self.output.spec().ok_or(RuntimeError::Stopped)?)?;
        self.engine.activate(config)?;
        Ok(())
    }
}
#[cfg(target_os = "linux")]
impl Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice> {
    pub(crate) fn wait_native<R, W>(
        &self,
        client: &crate::progress::ProgressClient<R, W>,
        clock: &mut crate::reactor::MonotonicClock,
    ) -> std::io::Result<crate::worker::WorkerWake>
    where
        R: std::io::Read + std::os::fd::AsFd,
        W: std::io::Write + std::os::fd::AsFd,
    {
        self.wait_native_with_control(client, clock, false)
    }
    pub(crate) fn wait_native_with_control<R, W>(
        &self,
        client: &crate::progress::ProgressClient<R, W>,
        clock: &mut crate::reactor::MonotonicClock,
        control: bool,
    ) -> std::io::Result<crate::worker::WorkerWake>
    where
        R: std::io::Read + std::os::fd::AsFd,
        W: std::io::Write + std::os::fd::AsFd,
    {
        self.wait_native_until(client, clock, control, None)
    }
    pub(crate) fn wait_native_until<R, W>(
        &self,
        client: &crate::progress::ProgressClient<R, W>,
        clock: &mut crate::reactor::MonotonicClock,
        control: bool,
        extra_deadline: Option<MonoTime>,
    ) -> std::io::Result<crate::worker::WorkerWake>
    where
        R: std::io::Read + std::os::fd::AsFd,
        W: std::io::Write + std::os::fd::AsFd,
    {
        use crate::reactor::{self, Interest, WaitRequest};
        let mut requests = Vec::with_capacity(reactor::MAX_WAIT_FDS);
        // Control slots stay at 0/1; all subsequent slots belong to devices.
        requests.extend(client.wait_requests_with_control(control));
        for input in &self.inputs {
            if let Some(fd) = input.capture.wait_fd() {
                requests.push(WaitRequest {
                    fd,
                    interest: Interest::Read,
                });
            }
        }
        let output_index = requests.len();
        if let Some(fd) = self.output.wait_fd() {
            // Normal output deadlines are already immediate when due. Do not
            // watch a permanently writable uinput when there is no queued work.
            requests.push(WaitRequest {
                fd,
                interest: Interest::Errors,
            });
        }
        let wake = reactor::wait(
            clock,
            &requests,
            self.next_wakeup().into_iter().chain(extra_deadline).min(),
            reactor::HOUSEKEEPING_NS,
        )?;
        let failure = (0..requests.len())
            .find(|index| wake.readiness(*index).is_some_and(|ready| ready.fault()))
            .map(|index| {
                if index < 2 {
                    StopReason::ControlFault
                } else if index == output_index {
                    StopReason::OutputFault
                } else {
                    StopReason::InputFault
                }
            });
        Ok(crate::worker::WorkerWake {
            at_ns: wake.at_ns,
            failure,
        })
    }
}
impl<S: CaptureSource, B: FrameBackend> Drop for Runtime<S, B> {
    fn drop(&mut self) {
        for input in &mut self.inputs {
            input.capture.close();
        }
        let _ = self.output.destroy_backend();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        capture::{ArmResult, OutputNodeReady},
        events::{FrameWrite, NativeKey},
        output::WriteError,
        probe::{Bits, Capabilities, DeviceId, Probe},
    };
    use keyweave_engine::{DelayedAction, Mapping};
    use std::{
        cell::RefCell,
        io,
        path::{Path, PathBuf},
        rc::Rc,
    };
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        #[cfg(target_os = "linux")]
        ConsumerCheck,
        #[cfg(target_os = "linux")]
        ConsumerClose,
        #[cfg(target_os = "linux")]
        ExtensionTick,
        #[cfg(target_os = "linux")]
        ExtensionClose,
        InputClose(u64),
        #[cfg(target_os = "linux")]
        TerminalWrite,
        Submit(Vec<NativeKey>, bool),
        Flush,
        OutputClose,
    }
    type Trace = Rc<RefCell<Vec<Call>>>;
    #[cfg(target_os = "linux")]
    struct ConsumerGate {
        remaining: usize,
        trace: Trace,
    }
    #[cfg(target_os = "linux")]
    impl crate::worker::PollControl<Source, Backend> for ConsumerGate {
        fn before_poll(
            &mut self,
            _: &Runtime<Source, Backend>,
            _: &mut crate::reactor::MonotonicClock,
            now: MonoTime,
        ) -> Result<MonoTime, crate::worker::WorkerError> {
            self.trace.borrow_mut().push(Call::ConsumerCheck);
            if self.remaining == 0 {
                return Err(crate::console::ConsoleLeaseError::NotReady.into());
            }
            self.remaining -= 1;
            Ok(now)
        }
        fn next_wakeup(&self) -> Option<MonoTime> {
            Some(MonoTime(42))
        }
    }
    #[cfg(target_os = "linux")]
    impl Drop for ConsumerGate {
        fn drop(&mut self) {
            self.trace.borrow_mut().push(Call::ConsumerClose);
        }
    }
    #[cfg(target_os = "linux")]
    struct BlockedReader;
    #[cfg(target_os = "linux")]
    impl io::Read for BlockedReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }
    struct Source {
        id: u64,
        probe: Probe,
        reads: VecDeque<io::Result<ReadBatch>>,
        trace: Trace,
    }
    impl CaptureSource for Source {
        fn probe(&self) -> &Probe {
            &self.probe
        }
        fn pressed_keys(&mut self) -> io::Result<Bits> {
            Ok(Bits::empty(768).unwrap())
        }
        fn set_grab(&mut self, _: bool) -> io::Result<()> {
            Ok(())
        }
        fn read_batch(&mut self) -> io::Result<ReadBatch> {
            self.reads.pop_front().unwrap_or(Ok(ReadBatch::WouldBlock))
        }
    }
    impl Drop for Source {
        fn drop(&mut self) {
            self.trace.borrow_mut().push(Call::InputClose(self.id));
        }
    }
    struct Backend {
        spec: DeviceSpec,
        trace: Trace,
        receipts: VecDeque<Result<FrameWrite, WriteError>>,
        flush: bool,
        path: PathBuf,
    }
    impl FrameBackend for Backend {
        fn spec(&self) -> &DeviceSpec {
            &self.spec
        }
        fn input_syspath(&self) -> Option<&Path> {
            Some(&self.path)
        }
        fn submit(
            &mut self,
            edges: &[NativeKey],
            recovery: bool,
        ) -> io::Result<Result<FrameWrite, WriteError>> {
            self.trace
                .borrow_mut()
                .push(Call::Submit(edges.to_vec(), recovery));
            Ok(self
                .receipts
                .pop_front()
                .unwrap_or(Ok(FrameWrite::Complete {
                    accepted: edges.len(),
                })))
        }
        fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
            self.trace.borrow_mut().push(Call::Flush);
            Ok(Ok(self.flush))
        }
    }
    impl Drop for Backend {
        fn drop(&mut self) {
            self.trace.borrow_mut().push(Call::OutputClose);
        }
    }
    fn bits(count: usize, codes: &[u16]) -> Bits {
        let mut bits = Bits::empty(count).unwrap();
        for code in codes {
            bits.set(*code).unwrap();
        }
        bits
    }
    fn probe(path: &str) -> Probe {
        Probe {
            id: DeviceId {
                bus: 6,
                vendor: 0,
                product: 0,
                version: 1,
            },
            name: None,
            input_syspath: crate::mock_syspath(path),
            capabilities: Capabilities {
                events: bits(32, &[0, 1, 20]),
                keys: bits(768, &keys::native_codes().collect::<Vec<_>>()),
                misc: bits(8, &[]),
                leds: bits(16, &[]),
                properties: bits(32, &[]),
            },
        }
    }
    fn resources(
        reads: Vec<Vec<io::Result<ReadBatch>>>,
    ) -> (
        Vec<(DeviceInstanceId, CaptureDevice<Source>)>,
        Backend,
        Trace,
    ) {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let backend = Backend {
            spec: DeviceSpec::new(
                "mock",
                &keys::native_codes().collect::<Vec<_>>(),
                RepeatPolicy::Kernel,
                false,
            )
            .unwrap(),
            trace: trace.clone(),
            receipts: VecDeque::new(),
            flush: true,
            path: crate::mock_syspath("/sys/devices/virtual/input/input999"),
        };
        let output_node = probe(backend.path.to_str().unwrap());
        let proof = OutputNodeReady::verify(&backend.spec, &backend.path, &output_node).unwrap();
        let inputs = reads
            .into_iter()
            .enumerate()
            .map(|(index, reads)| {
                let id = (index + 1) as u64;
                let mut physical = probe(&format!("/sys/devices/mock/input/input{id}"));
                physical.id.vendor = u16::try_from(index + 1).unwrap();
                physical.id.product = u16::try_from(index + 100).unwrap();
                physical.capabilities.events.set(4).unwrap();
                physical.capabilities.misc = bits(8, &[4, 5]);
                let mut capture = CaptureDevice::new(Source {
                    id,
                    probe: physical,
                    reads: reads.into(),
                    trace: trace.clone(),
                });
                assert_eq!(capture.arm(&proof).unwrap(), ArmResult::Captured);
                (DeviceInstanceId { id, generation: 1 }, capture)
            })
            .collect();
        (inputs, backend, trace)
    }
    fn config(generation: u64, to: &str) -> Arc<ConfigSnapshot> {
        Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(generation),
            name: "runtime".into(),
            simple: vec![Mapping {
                id: "map".into(),
                from: "a".parse().unwrap(),
                to: vec![to.parse::<KeyId>().unwrap().into()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            }],
            complex: vec![],
        })
    }
    fn frame(code: u16, value: i32) -> io::Result<ReadBatch> {
        Ok(ReadBatch::Events(vec![
            Event::output(1, code, value),
            Event::output(0, 0, 0),
        ]))
    }
    fn make_runtime(
        reads: Vec<Vec<io::Result<ReadBatch>>>,
        config: Arc<ConfigSnapshot>,
    ) -> (Runtime<Source, Backend>, Trace) {
        let (inputs, backend, trace) = resources(reads);
        (Runtime::new(inputs, backend, config).unwrap(), trace)
    }
    fn submits(trace: &Trace) -> Vec<(Vec<NativeKey>, bool)> {
        trace
            .borrow()
            .iter()
            .filter_map(|call| {
                if let Call::Submit(edges, recovery) = call {
                    Some((edges.clone(), *recovery))
                } else {
                    None
                }
            })
            .collect()
    }
    fn staged_config() -> Arc<ConfigSnapshot> {
        let mut cfg = (*config(1, "b")).clone();
        cfg.simple[0].to = vec![
            keyweave_engine::Action {
                hold_down_ns: 50,
                .."b".parse::<KeyId>().unwrap().into()
            },
            "c".parse::<KeyId>().unwrap().into(),
        ];
        Arc::new(cfg)
    }
    fn make_staged(
        reads: Vec<Vec<io::Result<ReadBatch>>>,
        cfg: Arc<ConfigSnapshot>,
    ) -> (Runtime<Source, Backend>, Trace) {
        let (inputs, backend, trace) = resources(reads);
        (Runtime::new_staged(inputs, backend, cfg).unwrap(), trace)
    }
    fn native_edges(trace: &Trace) -> Vec<(u16, i32)> {
        submits(trace)
            .into_iter()
            .flat_map(|(events, _)| events.into_iter().map(|event| (event.code, event.value)))
            .collect()
    }
    #[test]
    fn staged_array_early_up_preserves_duration_and_kernel_repeat_emits_no_extra_packet() {
        let (mut runtime, trace) = make_staged(
            vec![vec![frame(30, 1), frame(30, 2), frame(30, 0)]],
            staged_config(),
        );
        runtime.poll(MonoTime(100)).unwrap();
        assert_eq!(native_edges(&trace), [(48, 1)]);
        runtime.poll(MonoTime(110)).unwrap();
        runtime.poll(MonoTime(120)).unwrap();
        assert_eq!(native_edges(&trace), [(48, 1)]);
        assert_eq!(runtime.engine_state().pending_bindings, 1);
        runtime.poll(MonoTime(149)).unwrap();
        runtime.poll(MonoTime(150)).unwrap();
        assert_eq!(native_edges(&trace), [(48, 1), (48, 0), (46, 1), (46, 0)]);
        assert!(runtime.submitted().is_empty());
        assert_eq!(runtime.engine_state().pending_bindings, 0);
        assert_eq!(runtime.seq, 4); // one context + Down/Repeat/Up, no fake timer events
    }
    #[test]
    fn staged_large_immediate_array_yields_budget_and_continues_without_reregistering_root() {
        let mut cfg = (*config(1, "b")).clone();
        cfg.simple[0].to = vec![
            keyweave_engine::Action {
                repeat: false,
                .."b".parse::<KeyId>().unwrap().into()
            };
            512
        ];
        let (mut runtime, trace) =
            make_staged(vec![vec![frame(30, 1), frame(30, 0)]], Arc::new(cfg));
        let first = runtime.poll(MonoTime(100)).unwrap();
        assert_eq!(first.work, MAX_WORK_PER_POLL);
        assert!(first.pending);
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        assert!(runtime.engine_state().pending_deadlines > 0);
        for _ in 0..100 {
            let report = runtime.poll(MonoTime(100)).unwrap();
            assert!(report.work <= MAX_WORK_PER_POLL);
            if !report.pending && runtime.engine_state().bindings == 0 {
                break;
            }
        }
        assert_eq!(runtime.engine_state().bindings, 0);
        assert_eq!(runtime.engine_state().pending_bindings, 0);
        assert_eq!(runtime.engine_state().pending_deadlines, 0);
        assert_eq!(native_edges(&trace).len(), 1024);
        assert!(
            native_edges(&trace)
                .chunks_exact(2)
                .all(|pair| pair == [(48, 1), (48, 0)])
        );
        assert!(runtime.submitted().is_empty());
        assert_eq!(runtime.seq, 3);
        assert_eq!(runtime.stop_reason(), None);
    }
    #[test]
    fn staged_reload_keeps_original_future_array_and_up_then_new_root_uses_new_generation() {
        let (mut runtime, trace) = make_staged(
            vec![vec![frame(30, 1), frame(30, 0), frame(30, 1), frame(30, 0)]],
            staged_config(),
        );
        runtime.poll(MonoTime(100)).unwrap();
        runtime.activate(config(2, "d")).unwrap();
        runtime.poll(MonoTime(110)).unwrap();
        runtime.poll(MonoTime(150)).unwrap();
        runtime.poll(MonoTime(160)).unwrap();
        runtime.poll(MonoTime(170)).unwrap();
        assert_eq!(
            native_edges(&trace),
            [(48, 1), (48, 0), (46, 1), (46, 0), (32, 1), (32, 0)]
        );
        assert_eq!(runtime.config_generation(), ConfigGeneration(2));
    }
    #[test]
    fn staged_pause_cancels_future_array_and_closes_input_before_known_accepted_recovery() {
        let (mut runtime, trace) = make_staged(vec![vec![frame(30, 1)]], staged_config());
        runtime.poll(MonoTime(100)).unwrap();
        let before = trace.borrow().len();
        runtime.pause(MonoTime(110)).unwrap();
        assert_eq!(trace.borrow()[before], Call::InputClose(1));
        runtime.poll(MonoTime(110)).unwrap();
        assert_eq!(native_edges(&trace), [(48, 1), (48, 0)]);
        assert!(submits(&trace).last().unwrap().1);
        assert_eq!(runtime.engine_state().pending_deadlines, 0);
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert!(
            runtime
                .termination_report()
                .unwrap()
                .output_recovery_confirmed
        );
    }
    #[test]
    fn staged_default_cold_gate_is_retained_and_explicit_preflight_does_not_touch_devices() {
        let (inputs, backend, trace) = resources(vec![vec![]]);
        assert!(preflight_staged_config(staged_config(), &backend.spec).is_ok());
        assert!(matches!(
            Runtime::new(inputs, backend, staged_config()),
            Err(RuntimeError::Engine(EngineError::UnsupportedActionTiming))
        ));
        assert!(submits(&trace).is_empty());
        assert!(trace.borrow().contains(&Call::InputClose(1)));
        assert_eq!(trace.borrow().last(), Some(&Call::OutputClose));
    }
    #[test]
    fn staged_report_pending_stops_before_future_array_and_flush_precedes_recovery_up() {
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.push_back(Ok(FrameWrite::Incomplete {
            accepted: 1,
            report_pending: true,
        }));
        let mut runtime = Runtime::new_staged(inputs, backend, staged_config()).unwrap();
        assert!(matches!(
            runtime.poll(MonoTime(100)),
            Err(RuntimeError::IncompleteOutput)
        ));
        assert_eq!(runtime.phase(), RuntimePhase::Recovering);
        assert_eq!(runtime.output.phase(), Phase::ReportPending);
        assert!(runtime.submitted().contains(&"b".parse().unwrap()));
        runtime.poll(MonoTime(110)).unwrap();
        runtime.poll(MonoTime(111)).unwrap();
        assert_eq!(native_edges(&trace), [(48, 1), (48, 0)]);
        let events = trace.borrow();
        let close = events
            .iter()
            .position(|call| *call == Call::InputClose(1))
            .unwrap();
        let flush = events.iter().position(|call| *call == Call::Flush).unwrap();
        let recovery = events
            .iter()
            .position(|call| matches!(call, Call::Submit(_, true)))
            .unwrap();
        assert!(close < flush && flush < recovery);
        assert_eq!(runtime.engine_state().pending_deadlines, 0);
        assert!(
            runtime
                .termination_report()
                .unwrap()
                .output_recovery_confirmed
        );
    }
    #[test]
    fn staged_unknown_acceptance_preserves_unknown_fact_and_never_submits_speculative_up() {
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)]]);
        backend
            .receipts
            .push_back(Err(WriteError::UnknownAcceptance(io::Error::other(
                "synthetic staged",
            ))));
        let mut runtime = Runtime::new_staged(inputs, backend, staged_config()).unwrap();
        assert!(runtime.poll(MonoTime(100)).is_err());
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert_eq!(native_edges(&trace), [(48, 1)]);
        let report = runtime.termination_report().unwrap();
        assert!(report.acceptance_unknown);
        assert!(!report.output_recovery_confirmed);
        assert!(report.input_owners_released);
        assert!(trace.borrow().contains(&Call::InputClose(1)));
    }
    #[test]
    fn staged_trigger_device_removal_restores_other_device_source_through_ordered_output_pump() {
        let mut cfg = (*config(1, "right_control")).clone();
        cfg.simple[0]
            .extras
            .from_modifiers
            .mandatory
            .insert(keyweave_engine::Modifier::LeftShift);
        cfg.complex.push(Mapping {
            id: "source".into(),
            from: "left_shift".parse().unwrap(),
            to: vec!["x".parse::<KeyId>().unwrap().into()],
            allow_extra_modifiers: true,
            extras: Default::default(),
        });
        let (mut runtime, trace) = make_staged(
            vec![
                vec![frame(42, 1), Ok(ReadBatch::WouldBlock), frame(42, 0)],
                vec![Ok(ReadBatch::WouldBlock), frame(30, 1)],
            ],
            Arc::new(cfg),
        );
        runtime.poll(MonoTime(100)).unwrap();
        runtime.poll(MonoTime(105)).unwrap();
        runtime.poll(MonoTime(110)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        runtime.poll(MonoTime(115)).unwrap();
        runtime.remove_device(instance(2), MonoTime(120)).unwrap();
        runtime.poll(MonoTime(120)).unwrap();
        assert_eq!(
            native_edges(&trace),
            [(45, 1), (45, 0), (97, 1), (97, 0), (45, 1)]
        );
        assert_eq!(runtime.engine_state().output_owners, 1);
        assert_eq!(
            runtime.engine_state().pressed_outputs,
            ["x".parse::<KeyId>().unwrap()]
        );
        assert!(trace.borrow().contains(&Call::InputClose(2)));
        assert!(!trace.borrow().contains(&Call::InputClose(1)));
        runtime.poll(MonoTime(130)).unwrap();
        assert_eq!(native_edges(&trace).last(), Some(&(45, 0)));
        assert_eq!(runtime.engine_state().bindings, 0);
    }
    #[test]
    fn actual_runtime_frames_repeat_and_old_config_release() {
        let (mut runtime, trace) = make_runtime(
            vec![vec![frame(30, 1), frame(30, 2), frame(30, 0)]],
            config(1, "b"),
        );
        runtime.poll(MonoTime(0)).unwrap();
        runtime.activate(config(2, "c")).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        runtime.poll(MonoTime(2)).unwrap();
        assert_eq!(
            submits(&trace),
            vec![
                (vec![NativeKey { code: 48, value: 1 }], false),
                (vec![NativeKey { code: 48, value: 0 }], false)
            ]
        );
        assert_eq!(runtime.engine_state().bindings, 0);
        assert!(runtime.submitted().is_empty());
        assert_eq!(runtime.progress_epoch(), 3);
    }
    #[test]
    fn two_devices_share_owner_without_premature_output_up() {
        let (mut runtime, trace) = make_runtime(
            vec![
                vec![frame(30, 1), frame(30, 0)],
                vec![frame(30, 1), frame(30, 0)],
            ],
            config(1, "b"),
        );
        for now in 0..4 {
            runtime.poll(MonoTime(now)).unwrap();
            if now == 2 {
                assert_eq!(runtime.submitted().len(), 1);
            }
        }
        assert_eq!(
            submits(&trace),
            vec![
                (vec![NativeKey { code: 48, value: 1 }], false),
                (vec![NativeKey { code: 48, value: 0 }], false)
            ]
        );
    }
    #[test]
    fn partial_frames_do_not_match_and_msc_does_not_become_hid_hint() {
        let (mut runtime, trace) = make_runtime(
            vec![vec![
                Ok(ReadBatch::Events(vec![
                    Event::output(4, 4, 0x70004),
                    Event::output(1, 30, 1),
                ])),
                Ok(ReadBatch::Events(vec![Event::output(0, 0, 0)])),
            ]],
            config(1, "b"),
        );
        runtime.poll(MonoTime(0)).unwrap();
        assert!(submits(&trace).is_empty());
        runtime.poll(MonoTime(1)).unwrap();
        assert_eq!(submits(&trace)[0].0, vec![NativeKey { code: 48, value: 1 }]);
    }
    #[test]
    fn malformed_full_frame_does_not_emit_its_valid_prefix() {
        let (mut runtime, trace) = make_runtime(
            vec![vec![Ok(ReadBatch::Events(vec![
                Event::output(1, 30, 1),
                Event::output(1, 767, 1),
                Event::output(0, 0, 0),
            ]))]],
            config(1, "b"),
        );
        assert!(matches!(
            runtime.poll(MonoTime(0)),
            Err(RuntimeError::InvalidRecord)
        ));
        assert!(submits(&trace).is_empty());
        assert_eq!(runtime.stop_reason(), Some(StopReason::InputFault));
        assert_eq!(trace.borrow()[0], Call::InputClose(1));
    }
    #[test]
    fn syn_dropped_stops_all_inputs_before_output_cleanup_and_deadline() {
        let mut cfg = (*config(1, "b")).clone();
        let mut mapping = cfg.simple.pop().unwrap();
        mapping.extras.if_held_down = vec!["c".parse::<KeyId>().unwrap().into()];
        mapping.extras.held_threshold_ns = 5;
        cfg.complex.push(mapping);
        let (mut runtime, trace) = make_runtime(
            vec![
                vec![
                    frame(30, 1),
                    Ok(ReadBatch::Events(vec![
                        Event::output(0, 3, 0),
                        Event::output(0, 0, 0),
                    ])),
                ],
                vec![],
            ],
            Arc::new(cfg),
        );
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        assert!(matches!(
            runtime.poll(MonoTime(5)),
            Err(RuntimeError::InputLost)
        ));
        assert_eq!(runtime.stop_reason(), Some(StopReason::InputLost));
        assert_eq!(runtime.engine_state().bindings, 0);
        assert!(runtime.engine.next_deadline().is_none());
        runtime.poll(MonoTime(6)).unwrap();
        assert_eq!(
            submits(&trace),
            vec![
                (vec![NativeKey { code: 48, value: 1 }], false),
                (vec![NativeKey { code: 48, value: 0 }], true)
            ]
        );
        let calls = trace.borrow();
        let up = calls
            .iter()
            .position(|call| matches!(call, Call::Submit(_, true)))
            .unwrap();
        assert!(
            calls[..up].contains(&Call::InputClose(1))
                && calls[..up].contains(&Call::InputClose(2))
        );
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
    }
    #[test]
    fn held_timer_runs_before_physical_up_at_exact_boundary() {
        let mut cfg = (*config(1, "b")).clone();
        let mut mapping = cfg.simple.pop().unwrap();
        mapping.extras.if_held_down = vec!["c".parse::<KeyId>().unwrap().into()];
        mapping.extras.held_threshold_ns = 5;
        cfg.complex.push(mapping);
        let (mut runtime, trace) =
            make_runtime(vec![vec![frame(30, 1), frame(30, 0)]], Arc::new(cfg));
        runtime.poll(MonoTime(0)).unwrap();
        assert_eq!(runtime.next_wakeup(), Some(MonoTime(5)));
        runtime.poll(MonoTime(5)).unwrap();
        let edges: Vec<_> = submits(&trace)
            .into_iter()
            .flat_map(|(edges, _)| edges)
            .collect();
        assert_eq!(
            edges,
            vec![
                NativeKey { code: 48, value: 1 },
                NativeKey { code: 48, value: 0 },
                NativeKey { code: 46, value: 1 },
                NativeKey { code: 46, value: 0 }
            ]
        );
    }
    #[test]
    fn delayed_deadline_survives_up_and_uses_idle_poll() {
        let mut cfg = (*config(1, "b")).clone();
        let mut mapping = cfg.simple.pop().unwrap();
        mapping.extras.delayed = Some(DelayedAction {
            if_invoked: vec!["c".parse::<KeyId>().unwrap().into()],
            if_canceled: vec![],
            delay_ns: 5,
        });
        cfg.complex.push(mapping);
        let (mut runtime, trace) =
            make_runtime(vec![vec![frame(30, 1), frame(30, 0)]], Arc::new(cfg));
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        assert_eq!(runtime.engine_state().pending_bindings, 1);
        runtime.poll(MonoTime(5)).unwrap();
        assert_eq!(runtime.engine_state().pending_bindings, 0);
        assert_eq!(
            submits(&trace).last().unwrap().0,
            vec![
                NativeKey { code: 46, value: 1 },
                NativeKey { code: 46, value: 0 }
            ]
        );
    }
    #[test]
    fn read_error_and_pause_close_before_recovering_submitted_keys() {
        for read in [Err(io::Error::other("fault")), Ok(ReadBatch::Closed)] {
            let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1), read]], config(1, "b"));
            runtime.poll(MonoTime(0)).unwrap();
            assert!(runtime.poll(MonoTime(1)).is_err());
            assert_eq!(trace.borrow()[1], Call::InputClose(1));
            runtime.poll(MonoTime(2)).unwrap();
            assert_eq!(runtime.phase(), RuntimePhase::Stopped);
            assert!(runtime.submitted().is_empty());
        }
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        runtime.poll(MonoTime(0)).unwrap();
        runtime.pause(MonoTime(1)).unwrap();
        assert_eq!(runtime.stop_reason(), Some(StopReason::Pause));
        assert_eq!(trace.borrow()[1], Call::InputClose(1));
        runtime.poll(MonoTime(2)).unwrap();
    }
    #[test]
    fn known_short_flushes_report_after_capture_close_before_up() {
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.push_back(Ok(FrameWrite::Incomplete {
            accepted: 1,
            report_pending: true,
        }));
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        assert!(matches!(
            runtime.poll(MonoTime(0)),
            Err(RuntimeError::IncompleteOutput)
        ));
        assert_eq!(runtime.phase(), RuntimePhase::Recovering);
        runtime.poll(MonoTime(1)).unwrap();
        runtime.poll(MonoTime(2)).unwrap();
        assert_eq!(
            *trace.borrow(),
            vec![
                Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
                Call::InputClose(1),
                Call::Flush,
                Call::Submit(vec![NativeKey { code: 48, value: 0 }], true),
                Call::OutputClose
            ]
        );
    }
    #[test]
    fn unknown_write_closes_capture_then_output_without_guessed_up() {
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)]]);
        backend
            .receipts
            .push_back(Err(WriteError::UnknownAcceptance(io::Error::other(
                "ambiguous",
            ))));
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        assert!(runtime.poll(MonoTime(0)).is_err());
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert_eq!(
            trace.borrow().as_slice(),
            &[
                Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
                Call::InputClose(1),
                Call::OutputClose
            ]
        );
    }
    #[test]
    fn failed_report_recovery_has_a_finite_attempt_budget() {
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.push_back(Ok(FrameWrite::Incomplete {
            accepted: 1,
            report_pending: true,
        }));
        backend.flush = false;
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        assert!(runtime.poll(MonoTime(0)).is_err());
        for now in 1..=MAX_RECOVERY_POLLS {
            runtime.poll(MonoTime(now as u64)).unwrap();
        }
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert_eq!(
            trace
                .borrow()
                .iter()
                .filter(|call| **call == Call::Flush)
                .count(),
            MAX_RECOVERY_POLLS
        );
        assert_eq!(trace.borrow().last(), Some(&Call::OutputClose));
    }
    #[test]
    fn config_and_output_identity_are_admitted_before_runtime_accepts_input() {
        let (inputs, backend, trace) = resources(vec![vec![]]);
        assert!(matches!(
            Runtime::new(inputs, backend, config(1, "button1")),
            Err(RuntimeError::ConfigurationKey(_))
        ));
        assert_eq!(
            *trace.borrow(),
            vec![Call::InputClose(1), Call::OutputClose]
        );
        let (inputs, mut backend, trace) = resources(vec![vec![]]);
        backend.path = crate::mock_syspath("/sys/devices/virtual/input/input1000");
        assert!(matches!(
            Runtime::new(inputs, backend, config(1, "b")),
            Err(RuntimeError::InvalidStartup)
        ));
        assert_eq!(
            *trace.borrow(),
            vec![Call::InputClose(1), Call::OutputClose]
        );
        let (mut runtime, _) = make_runtime(vec![vec![frame(30, 1), frame(30, 0)]], config(1, "b"));
        runtime.poll(MonoTime(0)).unwrap();
        assert!(runtime.activate(config(2, "button1")).is_err());
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        runtime.poll(MonoTime(1)).unwrap();
        assert!(runtime.submitted().is_empty());
    }
    #[test]
    fn pending_work_is_bounded_and_no_extra_input_read_occurs_until_drained() {
        let events: Vec<_> = (0..128)
            .flat_map(|_| [Event::output(1, 30, 1), Event::output(1, 30, 0)])
            .chain([Event::output(0, 0, 0)])
            .collect();
        let (mut runtime, _) = make_runtime(
            vec![vec![
                Ok(ReadBatch::Events(events)),
                Err(io::Error::other("next read")),
            ]],
            config(1, "b"),
        );
        let report = runtime.poll(MonoTime(0)).unwrap();
        assert_eq!(report.work, MAX_WORK_PER_POLL);
        assert!(report.pending);
        assert!(runtime.pending_keys() > 0);
        while runtime.pending_keys() != 0 || runtime.output.ledger().queued_edges() != 0 {
            assert!(runtime.poll(MonoTime(1)).unwrap().work <= MAX_WORK_PER_POLL);
        }
        assert!(runtime.poll(MonoTime(2)).is_err());
    }
    #[test]
    fn drop_and_clock_fault_release_physical_access_first() {
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        runtime.poll(MonoTime(10)).unwrap();
        assert!(matches!(
            runtime.poll(MonoTime(9)),
            Err(RuntimeError::ClockBackwards)
        ));
        assert_eq!(runtime.stop_reason(), Some(StopReason::ClockFault));
        drop(runtime);
        assert_eq!(trace.borrow()[1], Call::InputClose(1));
        assert_eq!(trace.borrow().last(), Some(&Call::OutputClose));
        let (runtime, trace) = make_runtime(vec![vec![], vec![]], config(1, "b"));
        drop(runtime);
        assert_eq!(
            *trace.borrow(),
            vec![Call::InputClose(1), Call::InputClose(2), Call::OutputClose]
        );
    }
    #[test]
    fn large_delayed_cancellation_retries_saved_key_without_reusing_sequence() {
        use keyweave_engine::{Action, MappingExtras};
        let actions = |key: &str| {
            (0..249)
                .map(|index| {
                    let mut action: Action = key.parse::<KeyId>().unwrap().into();
                    action.modifiers = vec![
                        if index % 2 == 0 {
                            "left_control"
                        } else {
                            "left_shift"
                        }
                        .parse()
                        .unwrap(),
                    ];
                    action
                })
                .collect::<Vec<_>>()
        };
        let cfg = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "large cancellation".into(),
            simple: vec![],
            complex: vec![
                Mapping {
                    id: "a".into(),
                    from: "a".parse().unwrap(),
                    to: vec!["b".parse::<KeyId>().unwrap().into()],
                    allow_extra_modifiers: true,
                    extras: MappingExtras {
                        delayed: Some(DelayedAction {
                            if_invoked: vec![],
                            if_canceled: actions("d"),
                            delay_ns: 500,
                        }),
                        ..Default::default()
                    },
                },
                Mapping {
                    id: "e".into(),
                    from: "e".parse().unwrap(),
                    to: actions("f"),
                    allow_extra_modifiers: true,
                    extras: Default::default(),
                },
            ],
        });
        let (mut runtime, trace) =
            make_runtime(vec![vec![frame(30, 1), frame(30, 0), frame(18, 1)]], cfg);
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        assert_eq!(runtime.engine_state().pending_bindings, 1);
        runtime.poll(MonoTime(2)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        assert_eq!(runtime.engine_state().pending_bindings, 0);
        assert_eq!(runtime.seq, 5); // includes the initial bounded device-context event // down, up, CancelDelayed, then retried e down
        let edges: Vec<_> = submits(&trace)
            .into_iter()
            .flat_map(|(edges, _)| edges)
            .collect();
        assert_eq!(edges.len(), 1992); // initial b down/up plus 996 cancellation and 994 new macro
        assert!(edges.iter().any(|edge| edge.code == 32));
        assert!(edges.iter().any(|edge| edge.code == 33));
    }
    #[test]
    fn duplicate_unarmed_or_external_repeat_resources_cannot_start() {
        for mode in 0..3 {
            let (mut inputs, mut backend, trace) = resources(vec![vec![], vec![]]);
            match mode {
                0 => inputs[1].0.id = inputs[0].0.id,
                1 => inputs[1].1.release().unwrap(),
                _ => {
                    backend.spec = DeviceSpec::new(
                        "mock",
                        &keys::native_codes().collect::<Vec<_>>(),
                        RepeatPolicy::External,
                        false,
                    )
                    .unwrap()
                }
            }
            assert!(Runtime::new(inputs, backend, config(1, "b")).is_err());
            assert_eq!(
                *trace.borrow(),
                vec![Call::InputClose(1), Call::InputClose(2), Call::OutputClose]
            );
        }
    }
    fn emergency_frame(codes: &[u16], value: i32) -> io::Result<ReadBatch> {
        Ok(ReadBatch::Events(
            codes
                .iter()
                .map(|code| Event::output(1, *code, value))
                .chain([Event::output(0, 0, 0)])
                .collect(),
        ))
    }
    #[test]
    fn emergency_cannot_be_overridden_by_rules_that_swallow_every_chord_key() {
        let config = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "swallow emergency".into(),
            complex: vec![],
            simple: ["left_control", "left_alt", "left_shift", "f12"]
                .into_iter()
                .map(|name| Mapping {
                    id: name.into(),
                    from: name.parse().unwrap(),
                    to: vec![],
                    allow_extra_modifiers: true,
                    extras: Default::default(),
                })
                .collect(),
        });
        let (mut runtime, trace) =
            make_runtime(vec![vec![emergency_frame(&[29, 56, 42, 88], 1)]], config);
        runtime.poll(MonoTime(0)).unwrap();
        assert!(submits(&trace).is_empty());
        assert_eq!(
            runtime.next_wakeup(),
            Some(MonoTime(crate::emergency::HOLD_NS))
        );
        runtime
            .poll(MonoTime(crate::emergency::HOLD_NS - 1))
            .unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        let report = runtime.poll(MonoTime(crate::emergency::HOLD_NS)).unwrap();
        assert_eq!(report.work, 0);
        assert_eq!(runtime.stop_reason(), Some(StopReason::Emergency));
        assert_eq!(runtime.phase(), RuntimePhase::Recovering);
        assert_eq!(trace.borrow()[0], Call::InputClose(1));
        runtime.poll(MonoTime(crate::emergency::HOLD_NS)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert_eq!(trace.borrow().last(), Some(&Call::OutputClose));
    }
    #[test]
    fn emergency_release_before_threshold_cancels_and_repress_restarts() {
        let hold = crate::emergency::HOLD_NS;
        let (mut runtime, _) = make_runtime(
            vec![vec![
                emergency_frame(&[97, 100, 54, 88], 1),
                frame(88, 0),
                frame(88, 1),
            ]],
            Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))),
        );
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(hold - 1)).unwrap();
        assert!(
            runtime
                .inputs
                .iter()
                .all(|input| input.emergency.next_deadline().is_none())
        );
        assert_eq!(runtime.next_wakeup(), Some(runtime.context_refresh_at));
        runtime.poll(MonoTime(hold)).unwrap();
        assert_eq!(runtime.next_wakeup(), Some(MonoTime(2 * hold)));
        runtime.poll(MonoTime(2 * hold - 1)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        runtime.poll(MonoTime(2 * hold)).unwrap();
        assert_eq!(runtime.stop_reason(), Some(StopReason::Emergency));
    }
    #[test]
    fn emergency_does_not_combine_keys_from_two_devices() {
        let (mut runtime, _) = make_runtime(
            vec![vec![emergency_frame(&[29, 56, 42], 1)], vec![frame(88, 1)]],
            Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))),
        );
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        assert!(
            runtime
                .inputs
                .iter()
                .all(|input| input.emergency.next_deadline().is_none())
        );
        assert_eq!(runtime.next_wakeup(), Some(runtime.context_refresh_at));
        runtime
            .poll(MonoTime(2 * crate::emergency::HOLD_NS))
            .unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
    }
    #[test]
    fn emergency_wins_over_same_time_held_timer_and_queued_release() {
        let hold = crate::emergency::HOLD_NS;
        let cfg = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "same deadline".into(),
            simple: vec![],
            complex: vec![Mapping {
                id: "f12".into(),
                from: "f12".parse().unwrap(),
                to: vec!["b".parse::<KeyId>().unwrap().into()],
                allow_extra_modifiers: true,
                extras: keyweave_engine::MappingExtras {
                    if_held_down: vec!["c".parse::<KeyId>().unwrap().into()],
                    held_threshold_ns: hold,
                    ..Default::default()
                },
            }],
        });
        let (mut runtime, trace) = make_runtime(
            vec![vec![emergency_frame(&[29, 56, 42, 88], 1), frame(88, 0)]],
            cfg,
        );
        runtime.poll(MonoTime(0)).unwrap();
        assert_eq!(runtime.engine.next_deadline().unwrap(), MonoTime(hold));
        runtime.poll(MonoTime(hold)).unwrap();
        assert_eq!(runtime.stop_reason(), Some(StopReason::Emergency));
        assert!(runtime.engine.next_deadline().is_none());
        assert!(
            !submits(&trace)
                .iter()
                .flat_map(|(edges, _)| edges)
                .any(|edge| edge.code == 46)
        );
        assert_eq!(trace.borrow().last(), Some(&Call::InputClose(1)));
    }
    #[test]
    fn incomplete_frame_cannot_arm_and_time_overflow_stops_capture() {
        let hold = crate::emergency::HOLD_NS;
        let (mut runtime, _) = make_runtime(
            vec![vec![Ok(ReadBatch::Events(
                [29, 56, 42, 88]
                    .into_iter()
                    .map(|code| Event::output(1, code, 1))
                    .collect(),
            ))]],
            Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))),
        );
        runtime.poll(MonoTime(0)).unwrap();
        assert!(
            runtime
                .inputs
                .iter()
                .all(|input| input.emergency.next_deadline().is_none())
        );
        assert_eq!(runtime.next_wakeup(), Some(runtime.context_refresh_at));
        runtime.poll(MonoTime(hold)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        let (mut runtime, trace) = make_runtime(
            vec![vec![emergency_frame(&[29, 56, 42, 88], 1)]],
            Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))),
        );
        assert!(matches!(
            runtime.poll(MonoTime(u64::MAX)),
            Err(RuntimeError::ContextTimeOverflow)
        ));
        assert_eq!(runtime.stop_reason(), Some(StopReason::ControlFault));
        assert_eq!(trace.borrow()[0], Call::InputClose(1));
        assert!(submits(&trace).is_empty());
    }
    #[test]
    fn startup_rejects_capture_without_a_reachable_physical_emergency_chord() {
        let (_, backend, trace) = resources(vec![]);
        let output = probe(backend.path.to_str().unwrap());
        let proof = OutputNodeReady::verify(&backend.spec, &backend.path, &output).unwrap();
        let mut physical = probe("/sys/devices/mock/input/input1");
        physical.capabilities.keys = bits(768, &[113]);
        let mut input = CaptureDevice::new(Source {
            id: 1,
            probe: physical,
            reads: VecDeque::new(),
            trace: trace.clone(),
        });
        input.arm(&proof).unwrap();
        assert!(matches!(
            Runtime::new(
                vec![(
                    DeviceInstanceId {
                        id: 1,
                        generation: 1
                    },
                    input
                )],
                backend,
                Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)))
            ),
            Err(RuntimeError::EmergencyUnavailable)
        ));
        assert_eq!(
            *trace.borrow(),
            vec![Call::InputClose(1), Call::OutputClose]
        );
    }
    struct ProgressWriter {
        bytes: Rc<RefCell<Vec<u8>>>,
        chunk: usize,
        block_once: bool,
    }
    impl io::Write for ProgressWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.block_once {
                self.block_once = false;
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            let count = bytes.len().min(self.chunk);
            self.bytes.borrow_mut().extend_from_slice(&bytes[..count]);
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn progress_response_requires_a_completed_poll_after_challenge_receive() {
        use crate::progress::{Challenge, ProgressClient, Receipt};
        let (mut runtime, _) = make_runtime(vec![vec![]], config(1, "b"));
        runtime.poll(MonoTime(0)).unwrap(); // Old progress predates the challenge.
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let writer = ProgressWriter {
            bytes: bytes.clone(),
            chunk: 40,
            block_once: false,
        };
        let challenge = Challenge { run_id: 7, id: 1 };
        let mut client =
            ProgressClient::new(io::Cursor::new(challenge.encode()), writer, 7).unwrap();
        assert!(client.receive(&runtime).unwrap());
        assert!(!client.publish(&runtime).unwrap());
        assert!(!client.flush().unwrap());
        assert!(bytes.borrow().is_empty());
        runtime.poll(MonoTime(1)).unwrap();
        assert!(client.publish(&runtime).unwrap());
        let receipt = Receipt::decode(&bytes.borrow()).unwrap();
        assert_eq!(receipt.baseline, 1);
        assert_eq!(receipt.epoch, 2);
        assert_eq!(receipt.challenge, challenge);
        assert!(!client.publish(&runtime).unwrap());
        assert_eq!(bytes.borrow().len(), 40);
    }
    #[test]
    fn partial_progress_stays_on_original_epoch_and_next_challenge_takes_new_baseline() {
        use crate::progress::{Challenge, ProgressClient, Receipt};
        let (mut runtime, _) = make_runtime(vec![vec![]], config(1, "b"));
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let requests: Vec<_> = [
            Challenge { run_id: 7, id: 1 },
            Challenge { run_id: 7, id: 2 },
        ]
        .into_iter()
        .flat_map(Challenge::encode)
        .collect();
        let writer = ProgressWriter {
            bytes: bytes.clone(),
            chunk: 3,
            block_once: true,
        };
        let mut client = ProgressClient::new(io::Cursor::new(requests), writer, 7).unwrap();
        client.receive(&runtime).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        assert!(!client.publish(&runtime).unwrap());
        assert!(bytes.borrow().is_empty());
        assert!(!client.publish(&runtime).unwrap());
        runtime.poll(MonoTime(1)).unwrap();
        assert!(!client.receive(&runtime).unwrap());
        for _ in 0..13 {
            client.flush().unwrap();
        }
        let first = Receipt::decode(&bytes.borrow()).unwrap();
        assert_eq!(first.epoch, 1);
        assert_eq!(first.baseline, 0);
        assert!(client.receive(&runtime).unwrap());
        assert!(!client.publish(&runtime).unwrap());
        runtime.poll(MonoTime(2)).unwrap();
        client.publish(&runtime).unwrap();
        for _ in 0..13 {
            client.flush().unwrap();
        }
        let second = Receipt::decode(&bytes.borrow()[40..]).unwrap();
        assert_eq!(second.baseline, 2);
        assert_eq!(second.epoch, 3);
        assert_eq!(second.challenge.id, 2);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_pipe_hangup_stops_capture_before_releasing_submitted_keys() {
        use crate::{
            progress::{Challenge, ProgressClient, Receipt},
            reactor,
            worker::{WorkerError, WorkerWake, run_with_wait},
        };
        use std::{
            io::{Read, Write},
            os::fd::AsFd,
        };
        let (reader, mut parent_writer) = io::pipe().unwrap();
        let (mut parent_reader, writer) = io::pipe().unwrap();
        crate::ioctl::nonblocking(reader.as_fd()).unwrap();
        crate::ioctl::nonblocking(writer.as_fd()).unwrap();
        parent_writer
            .write_all(&Challenge { run_id: 7, id: 1 }.encode())
            .unwrap();
        let mut parent_writer = Some(parent_writer);
        let client = ProgressClient::new(reader, writer, 7).unwrap();
        let (runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        let result = run_with_wait(runtime, client, |runtime, client, clock| {
            assert_eq!(runtime.progress_epoch(), 1);
            assert!(runtime.submitted().contains(&"b".parse().unwrap()));
            drop(parent_writer.take());
            let wake = reactor::wait(
                clock,
                &client.wait_requests(),
                runtime.next_wakeup(),
                reactor::HOUSEKEEPING_NS,
            )?;
            assert!(wake.readiness(0).unwrap().hangup);
            Ok(WorkerWake {
                at_ns: wake.at_ns,
                failure: Some(StopReason::ControlFault),
            })
        });
        assert!(matches!(
            result,
            Err(WorkerError::Endpoint(StopReason::ControlFault))
        ));
        let mut bytes = [0; 40];
        parent_reader.read_exact(&mut bytes).unwrap();
        let receipt = Receipt::decode(&bytes).unwrap();
        assert_eq!(
            (receipt.baseline, receipt.epoch, receipt.phase),
            (0, 1, RuntimePhase::Active)
        );
        assert_eq!(
            *trace.borrow(),
            vec![
                Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
                Call::InputClose(1),
                Call::Submit(vec![NativeKey { code: 48, value: 0 }], true),
                Call::OutputClose
            ]
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_wait_failure_recovers_without_republishing_or_rereading_input() {
        use crate::{
            progress::{Challenge, ProgressClient},
            worker::{WorkerError, run_with_wait},
        };
        let (runtime, trace) = make_runtime(vec![vec![frame(30, 1), frame(30, 0)]], config(1, "b"));
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let client = ProgressClient::new(
            io::Cursor::new(Challenge { run_id: 7, id: 1 }.encode()),
            ProgressWriter {
                bytes: bytes.clone(),
                chunk: 40,
                block_once: false,
            },
            7,
        )
        .unwrap();
        let result = run_with_wait(runtime, client, |_, _, _| {
            Err(io::Error::from_raw_os_error(libc::EIO))
        });
        assert!(
            matches!(result, Err(WorkerError::System(error)) if error.raw_os_error() == Some(libc::EIO))
        );
        assert_eq!(bytes.borrow().len(), 40);
        assert_eq!(
            *trace.borrow(),
            vec![
                Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
                Call::InputClose(1),
                Call::Submit(vec![NativeKey { code: 48, value: 0 }], true),
                Call::OutputClose
            ]
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn consumer_gate_failure_precedes_first_input_and_provider_closes_last() {
        use crate::{
            progress::ProgressClient,
            reactor::MonotonicClock,
            worker::{WorkerError, run_with_poll_control},
        };
        let (runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        let gate = ConsumerGate {
            remaining: 0,
            trace: trace.clone(),
        };
        let report = run_with_poll_control(
            runtime,
            ProgressClient::new(BlockedReader, io::sink(), 7).unwrap(),
            true,
            MonotonicClock::new(),
            gate,
            |_, _, _, _| panic!("consumer failure cannot wait"),
        );
        assert!(matches!(
            report.outcome,
            Err(WorkerError::Consumer(
                crate::console::ConsoleLeaseError::NotReady
            ))
        ));
        assert_eq!(
            *trace.borrow(),
            vec![
                Call::ConsumerCheck,
                Call::InputClose(1),
                Call::OutputClose,
                Call::ConsumerClose
            ]
        );
        let terminal = report.termination.unwrap();
        assert_eq!(terminal.reason, StopReason::ControlFault);
        assert!(
            terminal.input_owners_released
                && terminal.output_owner_dropped
                && terminal.output_recovery_confirmed
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn active_consumer_loss_preserves_error_and_closes_input_before_saved_up() {
        use crate::{
            progress::ProgressClient,
            reactor::MonotonicClock,
            worker::{WorkerError, WorkerWake, run_with_poll_control},
        };
        let (runtime, trace) = make_runtime(
            vec![vec![
                frame(30, 1),
                Err(io::Error::other("must not read after consumer loss")),
            ]],
            config(1, "b"),
        );
        let gate = ConsumerGate {
            remaining: 1,
            trace: trace.clone(),
        };
        let report = run_with_poll_control(
            runtime,
            ProgressClient::new(BlockedReader, io::sink(), 7).unwrap(),
            true,
            MonotonicClock::new(),
            gate,
            |runtime, _, clock, deadline| {
                assert_eq!(runtime.progress_epoch(), 1);
                assert_eq!(deadline, Some(MonoTime(42)));
                Ok(WorkerWake {
                    at_ns: clock.now()?,
                    failure: None,
                })
            },
        );
        assert!(matches!(
            report.outcome,
            Err(WorkerError::Consumer(
                crate::console::ConsoleLeaseError::NotReady
            ))
        ));
        assert_eq!(
            *trace.borrow(),
            vec![
                Call::ConsumerCheck,
                Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
                Call::ConsumerCheck,
                Call::InputClose(1),
                Call::Submit(vec![NativeKey { code: 48, value: 0 }], true),
                Call::OutputClose,
                Call::ConsumerClose
            ]
        );
        let terminal = report.termination.unwrap();
        assert!(terminal.output_recovery_confirmed && !terminal.recovery_failed);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn stop_wins_before_consumer_query_and_recovers_saved_output() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            reactor::MonotonicClock,
            worker::run_with_poll_control,
        };
        let (mut runtime, trace) = make_runtime(
            vec![vec![
                frame(30, 1),
                Err(io::Error::other("must not read after stop")),
            ]],
            config(1, "b"),
        );
        runtime.poll(MonoTime(0)).unwrap();
        let gate = ConsumerGate {
            remaining: 0,
            trace: trace.clone(),
        };
        let report = run_with_poll_control(
            runtime,
            ProgressClient::new(
                io::Cursor::new(StopRequest { run_id: 7, id: 1 }.encode()),
                io::sink(),
                7,
            )
            .unwrap(),
            true,
            MonotonicClock::new(),
            gate,
            |_, _, _, _| panic!("stop cannot wait"),
        );
        assert!(matches!(report.outcome, Ok(StopReason::Pause)));
        assert!(!trace.borrow().contains(&Call::ConsumerCheck));
        assert_eq!(trace.borrow().last(), Some(&Call::ConsumerClose));
        assert!(report.termination.unwrap().output_recovery_confirmed);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_bad_challenge_closes_input_before_any_engine_or_wait_work() {
        use crate::{
            progress::{Challenge, ClientError, ProgressClient},
            worker::{WorkerError, run_with_wait},
        };
        let (runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        let bytes = Rc::new(RefCell::new(Vec::new()));
        let client = ProgressClient::new(
            io::Cursor::new(Challenge { run_id: 8, id: 1 }.encode()),
            ProgressWriter {
                bytes: bytes.clone(),
                chunk: 40,
                block_once: false,
            },
            7,
        )
        .unwrap();
        let result = run_with_wait(runtime, client, |_, _, _| {
            panic!("invalid protocol must not enter wait")
        });
        assert!(matches!(
            result,
            Err(WorkerError::Progress(ClientError::Protocol(_)))
        ));
        assert!(bytes.borrow().is_empty());
        assert_eq!(
            *trace.borrow(),
            vec![Call::InputClose(1), Call::OutputClose]
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_deadline_wake_drives_emergency_and_finite_recovery() {
        use crate::{
            progress::ProgressClient,
            worker::{WorkerWake, run_with_wait},
        };
        use std::os::fd::AsFd;
        let (reader, _parent_writer) = io::pipe().unwrap();
        crate::ioctl::nonblocking(reader.as_fd()).unwrap();
        let (runtime, trace) = make_runtime(
            vec![vec![emergency_frame(&[29, 56, 42, 88], 1)]],
            Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))),
        );
        let client = ProgressClient::new(reader, io::sink(), 7).unwrap();
        let mut waits = 0;
        let result = run_with_wait(runtime, client, |runtime, _, _| {
            waits += 1;
            assert!(waits <= MAX_RECOVERY_POLLS + 1);
            Ok(WorkerWake {
                at_ns: runtime.next_wakeup().unwrap(),
                failure: None,
            })
        })
        .unwrap();
        assert_eq!(result, StopReason::Emergency);
        let calls = trace.borrow();
        let close = calls
            .iter()
            .position(|call| matches!(call, Call::InputClose(_)))
            .unwrap();
        let recovery = calls
            .iter()
            .position(|call| matches!(call, Call::Submit(_, true)))
            .unwrap();
        assert!(close < recovery);
        assert_eq!(calls.last(), Some(&Call::OutputClose));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_runtime_read_fault_retains_original_error_and_finishes_recovery() {
        use crate::{
            progress::ProgressClient,
            worker::{WorkerError, WorkerWake, run_with_wait},
        };
        use std::os::fd::AsFd;
        let (reader, _parent_writer) = io::pipe().unwrap();
        crate::ioctl::nonblocking(reader.as_fd()).unwrap();
        let (runtime, trace) = make_runtime(
            vec![vec![
                frame(30, 1),
                Err(io::Error::from_raw_os_error(libc::EIO)),
            ]],
            config(1, "b"),
        );
        let client = ProgressClient::new(reader, io::sink(), 7).unwrap();
        let mut waits = 0;
        let result = run_with_wait(runtime, client, |_, _, clock| {
            waits += 1;
            Ok(WorkerWake {
                at_ns: clock.now()?,
                failure: None,
            })
        });
        assert_eq!(waits, 1);
        assert!(
            matches!(result, Err(WorkerError::Runtime(RuntimeError::Capture(CaptureError::Read(error)))) if error.raw_os_error() == Some(libc::EIO))
        );
        assert_eq!(
            *trace.borrow(),
            vec![
                Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
                Call::InputClose(1),
                Call::Submit(vec![NativeKey { code: 48, value: 0 }], true),
                Call::OutputClose
            ]
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn pending_progress_watches_write_only_until_original_frame_is_flushed() {
        use crate::{
            progress::{Challenge, ProgressClient},
            reactor::Interest,
        };
        use std::{
            io::{Read, Write},
            os::fd::AsFd,
        };
        let (reader, mut parent_writer) = io::pipe().unwrap();
        let (mut parent_reader, mut writer) = io::pipe().unwrap();
        for fd in [reader.as_fd(), writer.as_fd(), parent_reader.as_fd()] {
            crate::ioctl::nonblocking(fd).unwrap();
        }
        let mut full = false;
        for _ in 0..256 {
            match writer.write(&[0; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    full = true;
                    break;
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert!(full, "pipe buffer exceeded test bound");
        parent_writer
            .write_all(&Challenge { run_id: 7, id: 1 }.encode())
            .unwrap();
        let (mut runtime, _) = make_runtime(vec![vec![]], config(1, "b"));
        let mut client = ProgressClient::new(reader, writer, 7).unwrap();
        assert_eq!(
            client.wait_requests().map(|request| request.interest),
            [Interest::Read, Interest::Errors]
        );
        client.receive(&runtime).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        assert!(!client.publish(&runtime).unwrap());
        assert_eq!(
            client.wait_requests().map(|request| request.interest),
            [Interest::Errors, Interest::Write]
        );
        let mut drained = false;
        for _ in 0..256 {
            match parent_reader.read(&mut [0; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    drained = true;
                    break;
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert!(drained);
        assert!(client.flush().unwrap());
        assert_eq!(
            client.wait_requests().map(|request| request.interest),
            [Interest::Read, Interest::Errors]
        );
    }
    #[test]
    fn progress_client_rejects_wrong_run_and_skipped_challenge_without_writing() {
        use crate::progress::{Challenge, ClientError, ProgressClient, ProgressError};
        let (runtime, _) = make_runtime(vec![vec![]], config(1, "b"));
        for challenge in [
            Challenge { run_id: 8, id: 1 },
            Challenge { run_id: 7, id: 2 },
        ] {
            let bytes = Rc::new(RefCell::new(Vec::new()));
            let writer = ProgressWriter {
                bytes: bytes.clone(),
                chunk: 40,
                block_once: false,
            };
            let mut client =
                ProgressClient::new(io::Cursor::new(challenge.encode()), writer, 7).unwrap();
            assert!(matches!(
                client.receive(&runtime),
                Err(ClientError::Protocol(ProgressError::Identity))
            ));
            assert!(bytes.borrow().is_empty());
        }
    }
    fn device_config(
        generation: u64,
        to: &str,
        properties: DeviceProperties,
        unless: bool,
        exists: bool,
    ) -> Arc<ConfigSnapshot> {
        let mut cfg = (*config(generation, to)).clone();
        cfg.simple[0]
            .extras
            .conditions
            .push(keyweave_engine::Condition::Device {
                identifiers: vec![properties],
                unless,
                exists,
            });
        Arc::new(cfg)
    }
    #[test]
    fn owned_probe_vendor_product_map_only_the_matching_device() {
        let cfg = device_config(
            1,
            "b",
            DeviceProperties {
                vendor_id: Some(1),
                product_id: Some(100),
                ..Default::default()
            },
            false,
            false,
        );
        let (mut runtime, trace) = make_runtime(
            vec![
                vec![frame(30, 1), frame(30, 0)],
                vec![frame(30, 1), frame(30, 0)],
            ],
            cfg,
        );
        for time in 0..4 {
            runtime.poll(MonoTime(time)).unwrap();
        }
        let edges: Vec<_> = submits(&trace)
            .into_iter()
            .flat_map(|(edges, _)| edges)
            .collect();
        assert_eq!(
            edges,
            vec![
                NativeKey { code: 48, value: 1 },
                NativeKey { code: 30, value: 1 },
                NativeKey { code: 48, value: 0 },
                NativeKey { code: 30, value: 0 }
            ]
        );
        assert_eq!(
            runtime.captured_devices[0].device,
            DeviceInstanceId {
                id: 1,
                generation: 1
            }
        );
        assert_eq!(runtime.captured_devices[1].properties.vendor_id, Some(2));
        assert_eq!(runtime.engine.context_revision(), 1);
    }
    #[test]
    fn captured_inventory_proves_known_presence_but_not_uncaptured_absence() {
        for (vendor, unless, expected) in [
            (2, false, 48),
            (99, false, 30),
            (99, true, 30),
            (2, true, 30),
        ] {
            let cfg = device_config(
                1,
                "b",
                DeviceProperties {
                    vendor_id: Some(vendor),
                    ..Default::default()
                },
                unless,
                true,
            );
            let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)], vec![]], cfg);
            runtime.poll(MonoTime(0)).unwrap();
            assert_eq!(
                submits(&trace)[0].0,
                vec![NativeKey {
                    code: expected,
                    value: 1
                }]
            );
        }
    }
    #[test]
    fn unproven_properties_do_not_enable_unless_and_are_not_guessed() {
        let cfg = device_config(
            1,
            "b",
            DeviceProperties {
                is_built_in_keyboard: Some(true),
                ..Default::default()
            },
            true,
            false,
        );
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], cfg);
        runtime.poll(MonoTime(0)).unwrap();
        assert_eq!(submits(&trace)[0].0, vec![NativeKey { code: 30, value: 1 }]);
        let p = &runtime.captured_devices[0].properties;
        assert_eq!(
            p,
            &DeviceProperties {
                vendor_id: Some(1),
                product_id: Some(100),
                ..Default::default()
            }
        );
    }
    #[test]
    fn context_uses_absolute_clock_and_renews_before_expiry_in_poll_budget() {
        let (mut runtime, trace) = make_runtime(vec![vec![]], config(1, "b"));
        let start = 10 * DEVICE_CONTEXT_LIFETIME_NS;
        let report = runtime.poll(MonoTime(start)).unwrap();
        assert_eq!(report.work, 2); // bounded context update plus one input read
        assert_eq!(runtime.engine.context_revision(), 1);
        assert_eq!(
            runtime.next_wakeup(),
            Some(MonoTime(start + DEVICE_CONTEXT_REFRESH_NS))
        );
        runtime
            .poll(MonoTime(start + DEVICE_CONTEXT_REFRESH_NS - 1))
            .unwrap();
        assert_eq!(runtime.engine.context_revision(), 1);
        runtime
            .poll(MonoTime(start + DEVICE_CONTEXT_REFRESH_NS))
            .unwrap();
        assert_eq!(runtime.engine.context_revision(), 2);
        assert_eq!(
            runtime.next_wakeup(),
            Some(MonoTime(start + 2 * DEVICE_CONTEXT_REFRESH_NS))
        );
        assert!(trace.borrow().is_empty());
        assert_eq!(runtime.progress_epoch(), 3); // only completed polls, not context updates
    }
    #[test]
    fn refresh_and_hot_config_keep_original_conditional_key_up_binding() {
        let old = device_config(
            1,
            "b",
            DeviceProperties {
                vendor_id: Some(1),
                ..Default::default()
            },
            false,
            false,
        );
        let (mut runtime, trace) =
            make_runtime(vec![vec![frame(30, 1), frame(30, 0), frame(30, 1)]], old);
        runtime.poll(MonoTime(0)).unwrap();
        runtime
            .activate(device_config(
                2,
                "c",
                DeviceProperties {
                    vendor_id: Some(1),
                    ..Default::default()
                },
                false,
                false,
            ))
            .unwrap();
        runtime.poll(MonoTime(DEVICE_CONTEXT_REFRESH_NS)).unwrap();
        runtime
            .poll(MonoTime(DEVICE_CONTEXT_REFRESH_NS + 1))
            .unwrap();
        let edges: Vec<_> = submits(&trace)
            .into_iter()
            .flat_map(|(edges, _)| edges)
            .collect();
        assert_eq!(
            edges,
            vec![
                NativeKey { code: 48, value: 1 },
                NativeKey { code: 48, value: 0 },
                NativeKey { code: 46, value: 1 }
            ]
        );
        assert_eq!(runtime.context_revision, 2);
        assert_eq!(runtime.config_generation(), ConfigGeneration(2));
    }
    #[test]
    fn context_revision_and_sequence_exhaustion_close_capture_before_output() {
        for revision in [false, true] {
            let (mut runtime, trace) = make_runtime(vec![vec![]], config(1, "b"));
            if revision {
                runtime.context_revision = u64::MAX;
            } else {
                runtime.seq = u64::MAX;
            }
            assert!(matches!(
                runtime.poll(MonoTime(0)),
                Err(RuntimeError::IdentityExhausted)
            ));
            assert_eq!(runtime.stop_reason(), Some(StopReason::ControlFault));
            assert_eq!(trace.borrow()[0], Call::InputClose(1));
            assert!(submits(&trace).is_empty());
            assert_eq!(runtime.context_refresh_at, MonoTime(0));
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn controlled_worker_stop_cancels_due_held_before_any_new_input_and_closes_before_up() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            worker::run_with_wait_control,
        };
        let mut cfg = (*config(1, "b")).clone();
        let mut mapping = cfg.simple.pop().unwrap();
        mapping.extras.if_held_down = vec!["c".parse::<KeyId>().unwrap().into()];
        mapping.extras.held_threshold_ns = 5;
        cfg.complex.push(mapping);
        let (mut runtime, trace) = make_runtime(
            vec![vec![
                frame(30, 1),
                Err(io::Error::other("must not read after stop")),
            ]],
            Arc::new(cfg),
        );
        runtime.poll(MonoTime(0)).unwrap();
        let epoch = runtime.progress_epoch();
        let client = ProgressClient::new(
            io::Cursor::new(StopRequest { run_id: 7, id: 1 }.encode()),
            io::sink(),
            7,
        )
        .unwrap();
        let reason = run_with_wait_control(runtime, client, true, |_, _, _| {
            panic!("stop must not wait")
        })
        .unwrap();
        assert_eq!(reason, StopReason::Pause);
        let calls = trace.borrow();
        let close = calls
            .iter()
            .position(|c| *c == Call::InputClose(1))
            .unwrap();
        let recovery = calls
            .iter()
            .position(|c| matches!(c, Call::Submit(_, true)))
            .unwrap();
        assert!(close < recovery);
        assert_eq!(calls.last(), Some(&Call::OutputClose));
        assert_eq!(epoch, 1);
        assert_eq!(
            submits(&trace),
            vec![
                (vec![NativeKey { code: 48, value: 1 }], false),
                (vec![NativeKey { code: 48, value: 0 }], true)
            ]
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn controlled_stop_preempts_pending_reply_without_flushing_it() {
        use crate::{
            progress::{Challenge, ProgressClient, StopRequest},
            worker::run_with_wait_control,
        };
        use std::cell::Cell;
        struct Blocked(std::rc::Rc<Cell<usize>>);
        impl io::Write for Blocked {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                self.0.set(self.0.get() + 1);
                Err(io::ErrorKind::WouldBlock.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("no stdio flush")
            }
        }
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        let mut bytes = Challenge { run_id: 7, id: 1 }.encode().to_vec();
        bytes.extend(StopRequest { run_id: 7, id: 2 }.encode());
        let writes = std::rc::Rc::new(Cell::new(0));
        let mut client =
            ProgressClient::new(io::Cursor::new(bytes), Blocked(writes.clone()), 7).unwrap();
        assert!(client.receive(&runtime).unwrap());
        runtime.poll(MonoTime(0)).unwrap();
        assert!(!client.publish(&runtime).unwrap());
        assert_eq!(writes.get(), 1);
        assert_eq!(
            run_with_wait_control(runtime, client, true, |_, _, _| panic!(
                "no wait after Stop"
            ))
            .unwrap(),
            StopReason::Pause
        );
        assert_eq!(writes.get(), 1);
        assert_eq!(trace.borrow().last(), Some(&Call::OutputClose));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn default_worker_rejects_stop_and_controlled_worker_rejects_wrong_run_before_poll() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            worker::{WorkerError, run_with_wait_control},
        };
        for (control, run) in [(false, 7), (true, 8)] {
            let (runtime, trace) = make_runtime(
                vec![vec![Err(io::Error::other("must not read"))]],
                config(1, "b"),
            );
            let client = ProgressClient::new(
                io::Cursor::new(StopRequest { run_id: run, id: 1 }.encode()),
                io::sink(),
                7,
            )
            .unwrap();
            assert!(matches!(
                run_with_wait_control(runtime, client, control, |_, _, _| panic!(
                    "invalid request cannot wait"
                )),
                Err(WorkerError::Progress(_))
            ));
            assert_eq!(trace.borrow()[0], Call::InputClose(1));
            assert!(submits(&trace).is_empty());
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn controlled_worker_completes_short_recovery_only_after_report_flush() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            worker::run_with_wait_control,
        };
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Ok(FrameWrite::Incomplete {
                accepted: 1,
                report_pending: true,
            }),
        ]);
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        let client = ProgressClient::new(
            io::Cursor::new(StopRequest { run_id: 7, id: 1 }.encode()),
            io::sink(),
            7,
        )
        .unwrap();
        assert_eq!(
            run_with_wait_control(runtime, client, true, |_, _, _| panic!("no wait")).unwrap(),
            StopReason::Pause
        );
        let calls = trace.borrow();
        let close = calls
            .iter()
            .position(|c| *c == Call::InputClose(1))
            .unwrap();
        let recovery = calls
            .iter()
            .position(|c| matches!(c, Call::Submit(_, true)))
            .unwrap();
        assert!(close < recovery);
        assert!(calls.contains(&Call::Flush));
        assert_eq!(calls.last(), Some(&Call::OutputClose));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn controlled_worker_preserves_unknown_acceptance_recovery_error() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            worker::{WorkerError, run_with_wait_control},
        };
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Err(WriteError::UnknownAcceptance(io::Error::other(
                "synthetic recovery fault",
            ))),
        ]);
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        let client = ProgressClient::new(
            io::Cursor::new(StopRequest { run_id: 7, id: 1 }.encode()),
            io::sink(),
            7,
        )
        .unwrap();
        assert!(matches!(
            run_with_wait_control(runtime, client, true, |_, _, _| panic!("no wait")),
            Err(WorkerError::Runtime(RuntimeError::Output(_)))
        ));
        let calls = trace.borrow();
        let close = calls
            .iter()
            .position(|c| *c == Call::InputClose(1))
            .unwrap();
        let recovery = calls
            .iter()
            .position(|c| matches!(c, Call::Submit(_, true)))
            .unwrap();
        assert!(close < recovery);
        assert_eq!(calls.last(), Some(&Call::OutputClose));
        assert!(!calls.contains(&Call::Flush));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn controlled_wait_reads_stop_while_legacy_wait_has_a_pending_ticket() {
        use crate::progress::{Challenge, ProgressClient};
        use crate::reactor::Interest;
        use std::os::fd::AsFd;
        let (reader, mut writer) = io::pipe().unwrap();
        crate::ioctl::nonblocking(reader.as_fd()).unwrap();
        let (parent_reader, child_writer) = io::pipe().unwrap();
        let (runtime, _) = make_runtime(vec![vec![]], config(1, "b"));
        use io::Write;
        writer
            .write_all(&Challenge { run_id: 7, id: 1 }.encode())
            .unwrap();
        let mut client = ProgressClient::new(reader, child_writer, 7).unwrap();
        assert!(client.receive(&runtime).unwrap());
        assert_eq!(client.wait_requests()[0].interest, Interest::Errors);
        assert_eq!(
            client.wait_requests_with_control(true)[0].interest,
            Interest::Read
        );
        drop(parent_reader);
    }
    #[test]
    fn termination_samples_accepted_recovery_before_ledger_destruction() {
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        assert!(runtime.termination_report().is_none());
        runtime.poll(MonoTime(0)).unwrap();
        runtime.pause(MonoTime(1)).unwrap();
        assert!(runtime.termination_report().is_none());
        runtime.poll(MonoTime(1)).unwrap();
        let report = runtime.termination_report().unwrap();
        assert_eq!(report.reason, StopReason::Pause);
        assert!(
            report.input_owners_released
                && report.engine_reset_confirmed
                && report.output_recovery_confirmed
        );
        assert!(report.output_owner_dropped && report.backend_transition_confirmed);
        assert!(
            !report.recovery_failed
                && !report.recovery_budget_exhausted
                && !report.report_pending
                && !report.acceptance_unknown
                && !report.submitted_remaining
        );
        assert!(runtime.submitted().is_empty());
        assert_eq!(runtime.termination_report(), Some(report));
        assert!(matches!(
            runtime.poll(MonoTime(2)),
            Err(RuntimeError::Stopped)
        ));
        assert_eq!(runtime.termination_report(), Some(report));
        let calls = trace.borrow();
        assert!(
            calls
                .iter()
                .position(|c| *c == Call::InputClose(1))
                .unwrap()
                < calls
                    .iter()
                    .position(|c| matches!(c, Call::Submit(_, true)))
                    .unwrap()
        );
    }
    #[test]
    fn unknown_acceptance_with_empty_known_prefix_never_becomes_recovered() {
        let (inputs, mut backend, _) = resources(vec![vec![frame(30, 1)]]);
        backend
            .receipts
            .push_back(Err(WriteError::UnknownAcceptance(io::Error::other(
                "synthetic",
            ))));
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        assert!(runtime.poll(MonoTime(0)).is_err());
        let report = runtime.termination_report().unwrap();
        assert_eq!(report.reason, StopReason::OutputFault);
        assert!(
            report.acceptance_unknown
                && report.output_owner_dropped
                && report.backend_transition_confirmed
        );
        assert!(!report.submitted_remaining && !report.output_recovery_confirmed);
        assert!(runtime.submitted().is_empty());
    }
    #[test]
    fn report_budget_exhaustion_preserves_held_key_before_destroy() {
        let (inputs, mut backend, _) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.push_back(Ok(FrameWrite::Incomplete {
            accepted: 1,
            report_pending: true,
        }));
        backend.flush = false;
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        assert!(runtime.poll(MonoTime(0)).is_err());
        for t in 1..=MAX_RECOVERY_POLLS {
            runtime.poll(MonoTime(t as u64)).unwrap();
        }
        let report = runtime.termination_report().unwrap();
        assert!(
            report.recovery_budget_exhausted && report.submitted_remaining && report.report_pending
        );
        assert!(
            !report.output_recovery_confirmed
                && !report.acceptance_unknown
                && !report.recovery_failed
        );
        assert!(runtime.submitted().is_empty());
    }
    #[test]
    fn empty_submitted_with_pending_recovery_report_is_unconfirmed_at_budget() {
        let (inputs, mut backend, _) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Ok(FrameWrite::Incomplete {
                accepted: 1,
                report_pending: true,
            }),
        ]);
        backend.flush = false;
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        runtime.pause(MonoTime(1)).unwrap();
        for _ in 0..MAX_RECOVERY_POLLS {
            runtime.poll(MonoTime(1)).unwrap();
        }
        let report = runtime.termination_report().unwrap();
        assert!(report.report_pending && report.recovery_budget_exhausted);
        assert!(
            !report.submitted_remaining
                && !report.output_recovery_confirmed
                && !report.recovery_failed
        );
    }
    #[test]
    fn flushed_short_recovery_has_confirmed_evidence_only_after_report() {
        let (inputs, mut backend, _) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Ok(FrameWrite::Incomplete {
                accepted: 1,
                report_pending: true,
            }),
        ]);
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        runtime.pause(MonoTime(1)).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        assert!(runtime.submitted().is_empty());
        assert!(runtime.termination_report().is_none());
        runtime.poll(MonoTime(1)).unwrap();
        let report = runtime.termination_report().unwrap();
        assert!(
            report.output_recovery_confirmed
                && !report.report_pending
                && !report.recovery_budget_exhausted
        );
    }
    #[test]
    fn recovery_failure_is_retained_independently_from_owner_drop() {
        let (inputs, mut backend, _) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Err(WriteError::UnknownAcceptance(io::Error::other("synthetic"))),
        ]);
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        runtime.pause(MonoTime(1)).unwrap();
        assert!(runtime.poll(MonoTime(1)).is_err());
        let report = runtime.termination_report().unwrap();
        assert_eq!(report.reason, StopReason::Pause);
        assert!(report.recovery_failed && report.acceptance_unknown && report.submitted_remaining);
        assert!(report.output_owner_dropped && report.backend_transition_confirmed);
        assert!(!report.output_recovery_confirmed);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_successful_stop_result_does_not_hide_unfinished_report() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            worker::run_with_wait_report,
        };
        let (inputs, mut backend, _) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Ok(FrameWrite::Incomplete {
                accepted: 1,
                report_pending: true,
            }),
        ]);
        backend.flush = false;
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        let client = ProgressClient::new(
            io::Cursor::new(StopRequest { run_id: 7, id: 1 }.encode()),
            io::sink(),
            7,
        )
        .unwrap();
        let report =
            run_with_wait_report(runtime, client, true, |_, _, _| panic!("stop cannot wait"));
        assert!(matches!(report.outcome, Ok(StopReason::Pause)));
        let terminal = report.termination.unwrap();
        assert!(
            terminal.recovery_budget_exhausted
                && terminal.report_pending
                && !terminal.output_recovery_confirmed
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_preserves_primary_and_independent_termination_uncertainty() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            worker::{WorkerError, run_with_wait_report},
        };
        let (inputs, mut backend, _) = resources(vec![vec![frame(30, 1)]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Err(WriteError::UnknownAcceptance(io::Error::other("synthetic"))),
        ]);
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        let client = ProgressClient::new(
            io::Cursor::new(StopRequest { run_id: 7, id: 1 }.encode()),
            io::sink(),
            7,
        )
        .unwrap();
        let report =
            run_with_wait_report(runtime, client, true, |_, _, _| panic!("stop cannot wait"));
        assert!(matches!(
            report.outcome,
            Err(WorkerError::Runtime(RuntimeError::Output(_)))
        ));
        let terminal = report.termination.unwrap();
        assert!(
            terminal.recovery_failed
                && terminal.acceptance_unknown
                && terminal.output_owner_dropped
                && !terminal.output_recovery_confirmed
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn worker_finalization_follows_capture_and_output_close_and_keeps_original_error() {
        use crate::{
            progress::{ProgressClient, StopRequest},
            termination::{
                TerminalFrame, TerminalWriter,
                bounded::{DeliveryFailure, SendSchedule},
            },
            worker::{WorkerError, finalize_worker, run_with_wait_report},
        };
        struct Writer {
            bytes: Rc<RefCell<Vec<u8>>>,
            trace: Trace,
            fail: bool,
        }
        impl io::Write for Writer {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.trace.borrow_mut().push(Call::TerminalWrite);
                if self.fail {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                self.bytes.borrow_mut().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                panic!("no stdio flush")
            }
        }
        struct Schedule;
        impl SendSchedule<Writer> for Schedule {
            fn now(&mut self) -> io::Result<MonoTime> {
                Ok(MonoTime(0))
            }
            fn wait(
                &mut self,
                _: &TerminalWriter<Writer>,
                _: MonoTime,
            ) -> io::Result<(MonoTime, bool)> {
                panic!("single complete/failed write cannot wait")
            }
        }
        for fail in [false, true] {
            let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
            runtime.poll(MonoTime(0)).unwrap();
            let client = ProgressClient::new(
                io::Cursor::new(StopRequest { run_id: 8, id: 1 }.encode()),
                io::sink(),
                7,
            )
            .unwrap();
            let worker = run_with_wait_report(runtime, client, true, |_, _, _| {
                panic!("invalid run cannot wait")
            });
            let terminal = worker.termination.unwrap();
            let bytes = Rc::new(RefCell::new(Vec::new()));
            let mut writer = TerminalWriter::new(
                Writer {
                    bytes: bytes.clone(),
                    trace: trace.clone(),
                    fail,
                },
                7,
            )
            .unwrap();
            let reported = finalize_worker(worker, &mut writer, &mut Schedule);
            assert!(matches!(
                reported.worker.outcome,
                Err(WorkerError::Progress(_))
            ));
            assert_eq!(reported.worker.termination, Some(terminal));
            if fail {
                assert!(!reported.delivery.frame_sent);
                assert!(matches!(
                    reported.delivery.failure,
                    Some(DeliveryFailure::Write(_))
                ));
            } else {
                assert!(reported.delivery.frame_sent && reported.delivery.failure.is_none());
                assert_eq!(
                    TerminalFrame::decode(&bytes.borrow(), 7).unwrap().report,
                    terminal
                );
            }
            let calls = trace.borrow();
            let closed = calls.iter().position(|c| *c == Call::OutputClose).unwrap();
            let sent = calls
                .iter()
                .position(|c| *c == Call::TerminalWrite)
                .unwrap();
            assert!(closed < sent);
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn missing_worker_termination_skips_send_and_schedule_without_replacing_outcome() {
        use crate::{
            termination::{
                TerminalWriter,
                bounded::{DeliveryFailure, SendSchedule},
            },
            worker::{WorkerError, WorkerReport, finalize_worker},
        };
        struct Schedule;
        impl SendSchedule<Vec<u8>> for Schedule {
            fn now(&mut self) -> io::Result<MonoTime> {
                panic!("missing report must not read clock")
            }
            fn wait(
                &mut self,
                _: &TerminalWriter<Vec<u8>>,
                _: MonoTime,
            ) -> io::Result<(MonoTime, bool)> {
                panic!("missing report must not wait")
            }
        }
        let worker = WorkerReport {
            outcome: Err(WorkerError::System(io::ErrorKind::PermissionDenied.into())),
            termination: None,
        };
        let mut writer = TerminalWriter::new(Vec::new(), 7).unwrap();
        let reported = finalize_worker(worker, &mut writer, &mut Schedule);
        assert!(matches!(
            reported.worker.outcome,
            Err(WorkerError::System(_))
        ));
        assert!(matches!(
            reported.delivery.failure,
            Some(DeliveryFailure::MissingReport)
        ));
        assert_eq!(reported.delivery.attempts, 0);
        assert!(!reported.delivery.frame_sent);
    }
    fn instance(id: u64) -> DeviceInstanceId {
        DeviceInstanceId { id, generation: 1 }
    }
    #[test]
    fn removal_keeps_other_owner_and_never_reads_the_retired_slot() {
        let (mut runtime, trace) = make_runtime(
            vec![
                vec![frame(30, 1), Err(io::Error::other("retired slot read"))],
                vec![frame(30, 1), frame(30, 0)],
            ],
            config(1, "b"),
        );
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        let epoch = runtime.progress_epoch();
        let report = runtime.remove_device(instance(1), MonoTime(2)).unwrap();
        assert_eq!(
            report,
            RemovalReport {
                device: instance(1),
                remaining_devices: 1,
                phase: RuntimePhase::Active
            }
        );
        assert_eq!(runtime.progress_epoch(), epoch);
        assert_eq!(runtime.inputs.len(), 2);
        assert_eq!(runtime.inputs[0].capture.phase(), CapturePhase::Closed);
        assert_eq!(runtime.engine_state().bindings, 1);
        assert_eq!(submits(&trace).len(), 1); // shared b remains down
        runtime.poll(MonoTime(2)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        assert!(runtime.submitted().is_empty());
        assert_eq!(
            submits(&trace),
            vec![
                (vec![NativeKey { code: 48, value: 1 }], false),
                (vec![NativeKey { code: 48, value: 0 }], false)
            ]
        );
        assert!(trace.borrow().contains(&Call::InputClose(1)));
        assert!(!trace.borrow().contains(&Call::InputClose(2)));
        assert_eq!(
            runtime
                .captured_devices
                .iter()
                .map(|d| d.device)
                .collect::<Vec<_>>(),
            vec![instance(2)]
        );
    }
    #[test]
    fn removal_releases_old_config_binding_after_capture_close_without_extra_taps() {
        let mut cfg = (*config(1, "b")).clone();
        cfg.simple.clear();
        cfg.complex = vec![Mapping {
            id: "a".into(),
            from: "a".parse().unwrap(),
            to: vec!["b".parse::<KeyId>().unwrap().into()],
            allow_extra_modifiers: true,
            extras: keyweave_engine::MappingExtras {
                if_alone: vec!["c".parse::<KeyId>().unwrap().into()],
                after_key_up: vec!["d".parse::<KeyId>().unwrap().into()],
                ..Default::default()
            },
        }];
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)], vec![]], Arc::new(cfg));
        runtime.poll(MonoTime(0)).unwrap();
        runtime.activate(config(2, "e")).unwrap();
        runtime.remove_device(instance(1), MonoTime(1)).unwrap();
        assert!(runtime.submitted().contains(&"b".parse().unwrap())); // up is queued, not yet accepted
        assert_eq!(trace.borrow()[1], Call::InputClose(1));
        runtime.poll(MonoTime(1)).unwrap();
        assert_eq!(
            submits(&trace),
            vec![
                (vec![NativeKey { code: 48, value: 1 }], false),
                (vec![NativeKey { code: 48, value: 0 }], false)
            ]
        );
        assert_eq!(runtime.engine_state().bindings, 0);
    }
    #[test]
    fn known_removal_wins_over_own_held_deadline_but_keeps_other_timer() {
        let cfg = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "remove held".into(),
            simple: vec![],
            complex: [("a", "b", "c"), ("d", "f", "g")]
                .into_iter()
                .map(|(from, to, held)| Mapping {
                    id: from.into(),
                    from: from.parse().unwrap(),
                    to: vec![to.parse::<KeyId>().unwrap().into()],
                    allow_extra_modifiers: true,
                    extras: keyweave_engine::MappingExtras {
                        if_held_down: vec![held.parse::<KeyId>().unwrap().into()],
                        held_threshold_ns: 5,
                        ..Default::default()
                    },
                })
                .collect(),
        });
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)], vec![frame(32, 1)]], cfg);
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        runtime.remove_device(instance(1), MonoTime(5)).unwrap();
        runtime.poll(MonoTime(5)).unwrap();
        let edges: Vec<_> = submits(&trace).into_iter().flat_map(|(e, _)| e).collect();
        assert_eq!(
            edges,
            vec![
                NativeKey { code: 48, value: 1 },
                NativeKey { code: 33, value: 1 },
                NativeKey { code: 48, value: 0 },
                NativeKey { code: 33, value: 0 },
                NativeKey { code: 34, value: 1 }
            ]
        );
        assert_eq!(runtime.engine_state().bindings, 1);
    }
    #[test]
    fn removal_cancels_delayed_work_even_after_original_key_up() {
        let cfg = Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "remove pending".into(),
            simple: vec![],
            complex: vec![Mapping {
                id: "a".into(),
                from: "a".parse().unwrap(),
                to: vec!["b".parse::<KeyId>().unwrap().into()],
                allow_extra_modifiers: true,
                extras: keyweave_engine::MappingExtras {
                    delayed: Some(DelayedAction {
                        if_invoked: vec!["c".parse::<KeyId>().unwrap().into()],
                        if_canceled: vec!["d".parse::<KeyId>().unwrap().into()],
                        delay_ns: 5,
                    }),
                    ..Default::default()
                },
            }],
        });
        let (mut runtime, trace) =
            make_runtime(vec![vec![frame(30, 1), frame(30, 0)], vec![]], cfg);
        for t in 0..3 {
            runtime.poll(MonoTime(t)).unwrap();
        }
        assert!(runtime.engine.next_delayed().is_some());
        runtime.remove_device(instance(1), MonoTime(5)).unwrap();
        assert!(runtime.engine.next_delayed().is_none());
        runtime.poll(MonoTime(10)).unwrap();
        assert_eq!(
            submits(&trace),
            vec![
                (vec![NativeKey { code: 48, value: 1 }], false),
                (vec![NativeKey { code: 48, value: 0 }], false)
            ]
        );
    }
    #[test]
    fn discarded_buffered_source_events_do_not_rebind_other_slot_or_invent_up() {
        let cfg = device_config(
            1,
            "b",
            DeviceProperties {
                vendor_id: Some(2),
                ..Default::default()
            },
            false,
            false,
        );
        let (mut runtime, trace) = make_runtime(vec![vec![], vec![]], cfg);
        runtime
            .raw
            .extend([(0, Event::output(1, 30, 1)), (1, Event::output(1, 30, 1))]);
        runtime.logical = Some((
            0,
            InputEvent::Key {
                key: "a".parse().unwrap(),
                edge: KeyEdge::Down,
            },
        ));
        runtime.remove_device(instance(1), MonoTime(0)).unwrap();
        assert!(submits(&trace).is_empty());
        assert!(runtime.logical.is_none());
        assert_eq!(runtime.raw.front().unwrap().0, 1);
        runtime.poll(MonoTime(0)).unwrap();
        assert_eq!(
            submits(&trace),
            vec![(vec![NativeKey { code: 48, value: 1 }], false)]
        );
        assert_eq!(runtime.inputs[1].id, instance(2));
    }
    #[test]
    fn stale_unknown_and_duplicate_removal_cannot_advance_or_touch_live_capture() {
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)], vec![]], config(1, "b"));
        runtime.poll(MonoTime(10)).unwrap();
        let seq = runtime.seq;
        let count = trace.borrow().len();
        for device in [
            instance(99),
            DeviceInstanceId {
                id: 1,
                generation: 2,
            },
            DeviceInstanceId {
                id: 1,
                generation: 0,
            },
        ] {
            assert!(matches!(
                runtime.remove_device(device, MonoTime(u64::MAX)),
                Err(RuntimeError::DeviceNotCaptured)
            ));
            assert_eq!(runtime.now, MonoTime(10));
            assert_eq!(runtime.seq, seq);
            assert_eq!(trace.borrow().len(), count);
        }
        runtime.remove_device(instance(1), MonoTime(11)).unwrap();
        let seq = runtime.seq;
        assert!(matches!(
            runtime.remove_device(instance(1), MonoTime(12)),
            Err(RuntimeError::DeviceNotCaptured)
        ));
        assert_eq!(runtime.seq, seq);
        assert_eq!(runtime.now, MonoTime(11));
        assert_eq!(runtime.inputs[1].capture.phase(), CapturePhase::Captured);
    }
    #[test]
    fn last_device_removal_uses_finite_recovery_without_claiming_output_is_clear() {
        let (mut runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
        runtime.poll(MonoTime(0)).unwrap();
        let report = runtime.remove_device(instance(1), MonoTime(1)).unwrap();
        assert_eq!(report.remaining_devices, 0);
        assert_eq!(report.phase, RuntimePhase::Recovering);
        assert_eq!(runtime.stop_reason(), Some(StopReason::NoDevices));
        assert!(!runtime.submitted().is_empty());
        assert_eq!(trace.borrow()[1], Call::InputClose(1));
        runtime.poll(MonoTime(1)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert!(runtime.submitted().is_empty());
        assert!(matches!(
            runtime.remove_device(instance(1), MonoTime(2)),
            Err(RuntimeError::Stopped)
        ));
    }
    #[test]
    fn removal_sequence_failure_and_backward_clock_close_all_before_recovery() {
        for sequence in [true, false] {
            let (mut runtime, trace) =
                make_runtime(vec![vec![frame(30, 1)], vec![]], config(1, "b"));
            runtime.poll(MonoTime(10)).unwrap();
            if sequence {
                runtime.seq = u64::MAX;
            }
            let result =
                runtime.remove_device(instance(1), MonoTime(if sequence { 11 } else { 9 }));
            if sequence {
                assert!(matches!(result, Err(RuntimeError::IdentityExhausted)));
                assert_eq!(runtime.stop_reason(), Some(StopReason::ControlFault));
            } else {
                assert!(matches!(result, Err(RuntimeError::ClockBackwards)));
                assert_eq!(runtime.stop_reason(), Some(StopReason::ClockFault));
            }
            assert_eq!(
                &trace.borrow()[1..],
                &[Call::InputClose(1), Call::InputClose(2)]
            );
            runtime.poll(runtime.now).unwrap();
            assert!(runtime.submitted().is_empty());
        }
    }
    #[test]
    fn removal_output_short_write_stops_remaining_capture_before_recovery() {
        let (inputs, mut backend, trace) = resources(vec![vec![frame(30, 1)], vec![]]);
        backend.receipts.extend([
            Ok(FrameWrite::Complete { accepted: 1 }),
            Ok(FrameWrite::Incomplete {
                accepted: 1,
                report_pending: true,
            }),
        ]);
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        runtime.remove_device(instance(1), MonoTime(1)).unwrap();
        assert!(matches!(
            runtime.poll(MonoTime(1)),
            Err(RuntimeError::IncompleteOutput)
        ));
        assert_eq!(runtime.stop_reason(), Some(StopReason::OutputFault));
        assert!(runtime.submitted().is_empty());
        assert_eq!(runtime.phase(), RuntimePhase::Recovering); // empty ledger does not finish REPORT
        runtime.poll(MonoTime(1)).unwrap();
        assert!(runtime.submitted().is_empty());
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        let calls = trace.borrow();
        let remaining_close = calls
            .iter()
            .position(|c| *c == Call::InputClose(2))
            .unwrap();
        let flush = calls.iter().position(|c| *c == Call::Flush).unwrap();
        assert!(remaining_close < flush);
        assert!(!calls.iter().any(|c| matches!(c, Call::Submit(_, true)))); // up was already accepted
    }
    #[test]
    fn ambiguous_native_enodev_still_stops_every_capture() {
        let (mut runtime, trace) = make_runtime(
            vec![
                vec![frame(30, 1), Err(io::Error::from_raw_os_error(19))],
                vec![],
            ],
            config(1, "b"),
        );
        runtime.poll(MonoTime(0)).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        assert!(matches!(
            runtime.poll(MonoTime(2)),
            Err(RuntimeError::Capture(CaptureError::Read(_)))
        ));
        assert_eq!(runtime.stop_reason(), Some(StopReason::InputFault));
        assert!(
            trace.borrow().contains(&Call::InputClose(1))
                && trace.borrow().contains(&Call::InputClose(2))
        );
    }
    #[test]
    fn removal_of_last_emergency_interface_restores_remaining_physical_input() {
        let (mut inputs, backend, trace) = resources(vec![vec![frame(30, 1)]]);
        let output = node_for_removal_test(&backend);
        let proof = OutputNodeReady::verify(&backend.spec, &backend.path, &output).unwrap();
        let mut consumer = probe("/sys/devices/mock/input/consumer2");
        consumer.capabilities.keys = bits(768, &[113]);
        let mut input = CaptureDevice::new(Source {
            id: 2,
            probe: consumer,
            reads: VecDeque::new(),
            trace: trace.clone(),
        });
        assert_eq!(input.arm(&proof).unwrap(), ArmResult::Captured);
        inputs.push((instance(2), input));
        let mut runtime = Runtime::new(inputs, backend, config(1, "b")).unwrap();
        runtime.poll(MonoTime(0)).unwrap();
        let report = runtime.remove_device(instance(1), MonoTime(1)).unwrap();
        assert_eq!(report.remaining_devices, 0);
        assert_eq!(report.phase, RuntimePhase::Recovering);
        assert_eq!(
            runtime.stop_reason(),
            Some(StopReason::EmergencyUnavailable)
        );
        assert_eq!(
            &trace.borrow()[1..],
            &[Call::InputClose(1), Call::InputClose(2)]
        );
        runtime.poll(MonoTime(1)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
        assert!(runtime.submitted().is_empty());
    }
    fn node_for_removal_test(backend: &Backend) -> Probe {
        probe(backend.path.to_str().unwrap())
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn extension_runs_after_actual_poll_and_stop_or_fault_recovers_before_close() {
        use crate::{
            progress::ProgressClient,
            reactor::MonotonicClock,
            worker::{PollControl, WorkerError, run_with_poll_control},
        };
        struct Hook {
            gate: ConsumerGate,
            stop: bool,
        }
        impl PollControl<Source, Backend> for Hook {
            fn before_poll(
                &mut self,
                r: &Runtime<Source, Backend>,
                c: &mut MonotonicClock,
                n: MonoTime,
            ) -> Result<MonoTime, WorkerError> {
                self.gate.before_poll(r, c, n)
            }
            fn after_poll<R: io::Read, W: io::Write>(
                &mut self,
                runtime: &mut Runtime<Source, Backend>,
                _: &mut ProgressClient<R, W>,
                _: &mut MonotonicClock,
            ) -> Result<bool, WorkerError> {
                self.gate.trace.borrow_mut().push(Call::ExtensionTick);
                assert_eq!(runtime.progress_epoch(), 1);
                runtime.activate(config(2, "c")).unwrap();
                if self.stop {
                    Ok(true)
                } else {
                    Err(WorkerError::Extension(io::Error::other(
                        "owned test extension fault",
                    )))
                }
            }
            fn next_wakeup(&self) -> Option<MonoTime> {
                None
            }
            fn close(&mut self) {
                self.gate.trace.borrow_mut().push(Call::ExtensionClose);
            }
        }
        for stop in [true, false] {
            let (runtime, trace) = make_runtime(vec![vec![frame(30, 1)]], config(1, "b"));
            let hook = Hook {
                gate: ConsumerGate {
                    remaining: 1,
                    trace: trace.clone(),
                },
                stop,
            };
            let report = run_with_poll_control(
                runtime,
                ProgressClient::new(BlockedReader, io::sink(), 7).unwrap(),
                true,
                MonotonicClock::new(),
                hook,
                |_, _, _, _| panic!("extension stop/fault must not wait"),
            );
            if stop {
                assert!(matches!(report.outcome, Ok(StopReason::Pause)));
            } else {
                assert!(matches!(report.outcome, Err(WorkerError::Extension(_))));
            }
            assert_eq!(
                *trace.borrow(),
                vec![
                    Call::ConsumerCheck,
                    Call::Submit(vec![NativeKey { code: 48, value: 1 }], false),
                    Call::ExtensionTick,
                    Call::InputClose(1),
                    Call::Submit(vec![NativeKey { code: 48, value: 0 }], true),
                    Call::OutputClose,
                    Call::ExtensionClose,
                    Call::ConsumerClose
                ]
            );
            assert!(report.termination.unwrap().output_recovery_confirmed);
        }
    }
}
