//! Explicit-clock, bounded IO coordinator. Does not open devices or spawn threads.
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
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};
pub const MAX_DEVICES: usize = 64;
pub const MAX_WORK_PER_POLL: usize = 64;
pub const MAX_RECOVERY_POLLS: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimePhase {
    Active,
    Recovering,
    Stopped,
}
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
    engine: Engine,
    raw: VecDeque<(usize, Event)>,
    logical: Option<(usize, InputEvent)>,
    cursor: usize,
    seq: u64,
    now: MonoTime,
    progress_epoch: u64,
    phase: RuntimePhase,
    reason: Option<StopReason>,
    recovery_polls: usize,
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
impl<S: CaptureSource, B: FrameBackend> Runtime<S, B> {
    /// Takes ownership of already armed inputs. Caller must finish bootstrap
    /// readiness and neutral policy before constructing this coordinator.
    pub fn new(
        mut inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        backend: B,
        config: Arc<ConfigSnapshot>,
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
            Engine::new(config).map_err(RuntimeError::Engine)
        })();
        let engine = match prepared {
            Ok(engine) => engine,
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
        })
    }
    pub fn phase(&self) -> RuntimePhase {
        self.phase
    }
    pub fn stop_reason(&self) -> Option<StopReason> {
        self.reason
    }
    pub fn progress_epoch(&self) -> u64 {
        self.progress_epoch
    }
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
                .map(|(at, _)| at)
                .into_iter()
                .chain(self.output.ledger().next_deadline())
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
        let index = self.cursor;
        self.cursor = (self.cursor + 1) % self.inputs.len();
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
        let mut work = 0;
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
            } else if let Some((_, token)) = self
                .engine
                .next_deadline()
                .filter(|(at, _)| *at <= self.now)
            {
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
        let _ = self.engine.reset(self.now); // reset intents were never an acceptance ledger
        self.reason = Some(reason);
        self.phase = RuntimePhase::Recovering;
        if (self.output.phase() == Phase::Normal && self.output.begin_recovery(self.now).is_err())
            || matches!(self.output.phase(), Phase::Quarantined | Phase::Detached)
        {
            let _ = self.output.destroy_backend();
            self.phase = RuntimePhase::Stopped;
        }
    }
    fn recover_poll(&mut self) -> Result<usize, RuntimeError> {
        self.recovery_polls += 1;
        let result = self.output.poll(self.now);
        if result.is_err()
            || (self.output.phase() != Phase::ReportPending
                && self.output.ledger().submitted().is_empty())
            || self.recovery_polls >= MAX_RECOVERY_POLLS
        {
            self.output.destroy_backend()?;
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
                    RuntimeError::Emergency(_) => StopReason::ControlFault,
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
        use crate::reactor::{self, Interest, WaitRequest};
        let mut requests = Vec::with_capacity(reactor::MAX_WAIT_FDS);
        // Control slots stay at 0/1; all subsequent slots belong to devices.
        requests.extend(client.wait_requests());
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
            self.next_wakeup(),
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
        InputClose(u64),
        Submit(Vec<NativeKey>, bool),
        Flush,
        OutputClose,
    }
    type Trace = Rc<RefCell<Vec<Call>>>;
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
        assert_eq!(runtime.seq, 4); // down, up, CancelDelayed, then retried e down
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
        assert!(runtime.next_wakeup().is_none());
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
        assert!(runtime.next_wakeup().is_none());
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
        assert_eq!(runtime.engine.next_deadline().unwrap().0, MonoTime(hold));
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
        assert!(runtime.next_wakeup().is_none());
        runtime.poll(MonoTime(hold)).unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Active);
        let (mut runtime, trace) = make_runtime(
            vec![vec![emergency_frame(&[29, 56, 42, 88], 1)]],
            Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))),
        );
        assert!(matches!(
            runtime.poll(MonoTime(u64::MAX)),
            Err(RuntimeError::Emergency(EmergencyError::TimeOverflow))
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
}
