//! All-device capture admission, without OS discovery or consumer-ready claims.
use crate::{
    capture::{
        ArmResult, CaptureDevice, CaptureError, CapturePhase, CaptureSource, OutputNodeReady,
    },
    emergency::EmergencyChord,
    output::RepeatPolicy,
    pump::FrameBackend,
    runtime::{ExecutionMode, MAX_DEVICES, Runtime, RuntimeError, validate_config},
};
use keyweave_engine::{ConfigSnapshot, EngineError};
use keyweave_types::DeviceInstanceId;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupPhase {
    Standby,
    Armed,
    Closed,
}
#[derive(Debug, thiserror::Error)]
pub enum ActivationError {
    #[error("invalid input count, identity, path or capture phase")]
    Inputs,
    #[error("invalid capture group phase")]
    Phase,
    #[cfg(target_os = "linux")]
    #[error("console observation is stale or belongs to another output")]
    ConsoleNotReady,
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("capture admission failed for {device:?}: {source}")]
    Capture {
        device: DeviceInstanceId,
        #[source]
        source: CaptureError,
    },
    #[error("capture rollback failed for {count} device(s)", count = .failures.len())]
    Rollback {
        #[source]
        primary: Option<Box<ActivationError>>,
        failures: Vec<(DeviceInstanceId, CaptureError)>,
    },
}
pub struct CaptureGroup<S: CaptureSource> {
    inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
    config: Arc<ConfigSnapshot>,
    phase: GroupPhase,
    mode: ExecutionMode,
    relative: bool,
}
impl<S: CaptureSource> CaptureGroup<S> {
    pub fn new(
        inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, ActivationError> {
        Self::new_with_mode(inputs, config, ExecutionMode::Legacy)
    }
    pub fn new_with_mode(
        inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        config: Arc<ConfigSnapshot>,
        mode: ExecutionMode,
    ) -> Result<Self, ActivationError> {
        Self::new_inner(inputs, config, mode, false)
    }
    /// Explicit mixed admission. Consumer readiness and supervision remain caller responsibilities.
    pub fn new_staged_relative(
        inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, ActivationError> {
        Self::new_inner(inputs, config, ExecutionMode::StagedKeyboard, true)
    }
    fn new_inner(
        inputs: Vec<(DeviceInstanceId, CaptureDevice<S>)>,
        config: Arc<ConfigSnapshot>,
        mode: ExecutionMode,
        relative: bool,
    ) -> Result<Self, ActivationError> {
        let group = Self {
            inputs,
            config,
            phase: GroupPhase::Standby,
            mode,
            relative,
        };
        group.inventory()?;
        // Reject unsupported timing/expansion before any state query or grab.
        match mode.validate_snapshot(group.config.clone()) {
            Err(RuntimeError::Engine(error)) => return Err(ActivationError::Engine(error)),
            Err(error) => return Err(ActivationError::Runtime(error)),
            Ok(()) => {}
        }
        Ok(group)
    }
    fn inventory(&self) -> Result<(), ActivationError> {
        if self.inputs.is_empty() || self.inputs.len() > MAX_DEVICES {
            return Err(ActivationError::Inputs);
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for (id, input) in &self.inputs {
            if (input.relative_allowed() && !self.relative)
                || input.phase() != CapturePhase::Standby
                || !ids.insert(id.id)
                || input.probe().is_none_or(|probe| {
                    !probe.input_syspath.is_absolute() || !paths.insert(probe.input_syspath.clone())
                })
            {
                return Err(ActivationError::Inputs);
            }
        }
        Ok(())
    }
    pub fn phase(&self) -> GroupPhase {
        self.phase
    }
    fn prepare(&self, output: &OutputNodeReady<'_>) -> Result<(), ActivationError> {
        self.inventory()?;
        if (self.relative && output.spec().relative_axes().is_none())
            || (!self.relative && output.spec().pointing_buttons().is_some())
        {
            return Err(RuntimeError::StagedPointer.into());
        }
        validate_config(&self.config, output.spec())?;
        if output.spec().repeat() != RepeatPolicy::Kernel {
            return Err(RuntimeError::RepeatPolicy.into());
        }
        for (device, input) in &self.inputs {
            input
                .validate_output(output)
                .map_err(|source| ActivationError::Capture {
                    device: *device,
                    source,
                })?;
        }
        if !self.inputs.iter().any(|(_, input)| {
            input
                .probe()
                .is_some_and(|probe| EmergencyChord::supported(&probe.capabilities.keys))
        }) {
            return Err(RuntimeError::EmergencyUnavailable.into());
        }
        Ok(())
    }
    /// Caller must establish consumer readiness and supervision first. The
    /// output proof only identifies the virtual node; neutral checks are samples,
    /// not an atomic cross-device snapshot or a lossless switching guarantee.
    pub fn try_arm(&mut self, output: &OutputNodeReady<'_>) -> Result<ArmResult, ActivationError> {
        if self.phase != GroupPhase::Standby {
            return Err(ActivationError::Phase);
        }
        let result = (|| {
            self.prepare(output)?;
            for (device, input) in &mut self.inputs {
                if !input
                    .check_neutral()
                    .map_err(|source| ActivationError::Capture {
                        device: *device,
                        source,
                    })?
                {
                    return Ok(ArmResult::WaitingNeutral);
                }
            }
            for (device, input) in &mut self.inputs {
                if input
                    .arm(output)
                    .map_err(|source| ActivationError::Capture {
                        device: *device,
                        source,
                    })?
                    == ArmResult::WaitingNeutral
                {
                    return Ok(ArmResult::WaitingNeutral);
                }
            }
            for (device, input) in &mut self.inputs {
                if !input
                    .check_neutral()
                    .map_err(|source| ActivationError::Capture {
                        device: *device,
                        source,
                    })?
                {
                    return Ok(ArmResult::WaitingNeutral);
                }
            }
            Ok(ArmResult::Captured)
        })();
        if matches!(result, Ok(ArmResult::Captured)) {
            self.phase = GroupPhase::Armed;
            return result;
        }
        let mut failures = Vec::new();
        for (id, input) in &mut self.inputs {
            if input.phase() == CapturePhase::Captured
                && let Err(error) = input.release()
            {
                failures.push((*id, error));
            }
        }
        if !failures.is_empty() {
            self.close();
            return Err(ActivationError::Rollback {
                primary: result.err().map(Box::new),
                failures,
            });
        }
        if result.is_err() {
            self.close();
        }
        result
    }
    /// Initial text-console gate, checked before any neutral query or grab.
    /// The caller supplies current monotonic time and independent supervision.
    /// This is not an active lease: reobserve after admission, before Runtime
    /// dispatch, and throughout the active loop; close capture on lost evidence.
    #[cfg(target_os = "linux")]
    pub fn try_arm_console(
        &mut self,
        output: &OutputNodeReady<'_>,
        ready: &crate::console::ConsoleReady,
        now: keyweave_types::MonoTime,
    ) -> Result<ArmResult, ActivationError> {
        if self.phase != GroupPhase::Standby {
            return Err(ActivationError::Phase);
        }
        if self.relative || !ready.fresh_for(output.input_syspath(), now) {
            return Err(ActivationError::ConsoleNotReady);
        }
        self.try_arm(output)
    }
    pub fn close(&mut self) {
        for (_, input) in &mut self.inputs {
            input.close();
        }
        self.phase = GroupPhase::Closed;
    }
    pub fn into_runtime<B: FrameBackend>(
        mut self,
        backend: B,
    ) -> Result<Runtime<S, B>, ActivationError> {
        if self.phase != GroupPhase::Armed {
            self.close();
            drop(backend);
            return Err(ActivationError::Phase);
        }
        let inputs = std::mem::take(&mut self.inputs);
        if self.relative {
            Runtime::new_staged_relative(inputs, backend, self.config.clone())
        } else {
            Runtime::new_with_mode(inputs, backend, self.config.clone(), self.mode)
        }
        .map_err(ActivationError::Runtime)
    }
}
impl<S: CaptureSource> Drop for CaptureGroup<S> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        events::{Event, FrameWrite, NativeKey},
        keys,
        output::{DeviceSpec, WriteError},
        probe::{Bits, Capabilities, DeviceId, Probe},
        read::ReadBatch,
    };
    use keyweave_engine::Mapping;
    use keyweave_types::{ConfigGeneration, KeyId, MonoTime};
    use std::{
        cell::RefCell,
        collections::VecDeque,
        io,
        path::{Path, PathBuf},
        rc::Rc,
    };
    const OUTPUT: &str = "/sys/devices/virtual/input/input999";
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        Query(u64),
        Grab(u64, bool),
        Close(u64),
        Read(u64),
        Submit(Vec<NativeKey>, bool),
        OutputClose,
    }
    type Trace = Rc<RefCell<Vec<Call>>>;
    struct Source {
        id: u64,
        probe: Probe,
        queries: VecDeque<io::Result<Bits>>,
        grab_fault: bool,
        release_fault: bool,
        reads: VecDeque<ReadBatch>,
        trace: Trace,
    }
    impl CaptureSource for Source {
        fn probe(&self) -> &Probe {
            &self.probe
        }
        fn pressed_keys(&mut self) -> io::Result<Bits> {
            self.trace.borrow_mut().push(Call::Query(self.id));
            self.queries
                .pop_front()
                .unwrap_or_else(|| Ok(bits(768, &[])))
        }
        fn set_grab(&mut self, grabbed: bool) -> io::Result<()> {
            self.trace.borrow_mut().push(Call::Grab(self.id, grabbed));
            if (grabbed && self.grab_fault) || (!grabbed && self.release_fault) {
                Err(io::Error::from_raw_os_error(if grabbed { 1 } else { 5 }))
            } else {
                Ok(())
            }
        }
        fn read_batch(&mut self) -> io::Result<ReadBatch> {
            self.trace.borrow_mut().push(Call::Read(self.id));
            Ok(self.reads.pop_front().unwrap_or(ReadBatch::WouldBlock))
        }
    }
    impl Drop for Source {
        fn drop(&mut self) {
            self.trace.borrow_mut().push(Call::Close(self.id));
        }
    }
    struct Backend {
        spec: DeviceSpec,
        path: PathBuf,
        trace: Trace,
    }
    impl FrameBackend for Backend {
        fn relative_axes(&self) -> Option<&crate::relative::RelativeAxes> {
            self.spec.relative_axes()
        }
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
            Ok(Ok(FrameWrite::Complete {
                accepted: edges.len(),
            }))
        }
        fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
            Ok(Ok(true))
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
                relative: crate::probe::Bits::empty(16).unwrap_or_else(|error| panic!("{error}")),

                events: bits(32, &[0, 1, 20]),
                keys: bits(768, &keys::native_codes().collect::<Vec<_>>()),
                misc: bits(8, &[]),
                leds: bits(16, &[]),
                properties: bits(32, &[]),
            },
        }
    }
    fn source(id: u64, trace: &Trace) -> Source {
        Source {
            id,
            probe: probe(&format!("/sys/devices/mock/input/input{id}")),
            queries: VecDeque::new(),
            grab_fault: false,
            release_fault: false,
            reads: VecDeque::new(),
            trace: trace.clone(),
        }
    }
    fn inputs(sources: Vec<Source>) -> Vec<(DeviceInstanceId, CaptureDevice<Source>)> {
        sources
            .into_iter()
            .map(|source| {
                (
                    DeviceInstanceId {
                        id: source.id,
                        generation: 1,
                    },
                    CaptureDevice::new(source),
                )
            })
            .collect()
    }
    fn config() -> Arc<ConfigSnapshot> {
        Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "group".into(),
            simple: vec![Mapping {
                id: "a-b".into(),
                from: "a".parse().unwrap(),
                to: vec!["b".parse::<KeyId>().unwrap().into()],
                allow_extra_modifiers: true,
                extras: Default::default(),
            }],
            complex: vec![],
        })
    }
    fn spec() -> DeviceSpec {
        DeviceSpec::new(
            "group",
            &keys::native_codes().collect::<Vec<_>>(),
            RepeatPolicy::Kernel,
            false,
        )
        .unwrap()
    }
    fn backend(trace: &Trace) -> Backend {
        Backend {
            spec: spec(),
            path: crate::mock_syspath(OUTPUT),
            trace: trace.clone(),
        }
    }
    fn proof<'a>(spec: &'a DeviceSpec, node: &'a Probe) -> OutputNodeReady<'a> {
        OutputNodeReady::verify(spec, &node.input_syspath, node).unwrap()
    }
    fn held() -> io::Result<Bits> {
        Ok(bits(768, &[30]))
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn staged_capture_group_pins_mode_through_runtime_transfer() {
        let trace = Trace::default();
        let mut cfg = (*config()).clone();
        cfg.simple[0].to = vec![
            keyweave_engine::Action {
                hold_down_ns: 50,
                .."b".parse::<KeyId>().unwrap().into()
            },
            "c".parse::<KeyId>().unwrap().into(),
        ];
        let mut group = CaptureGroup::new_with_mode(
            inputs(vec![source(1, &trace)]),
            Arc::new(cfg),
            ExecutionMode::StagedKeyboard,
        )
        .unwrap();
        assert!(trace.borrow().is_empty());
        let backend = backend(&trace);
        let node = probe(OUTPUT);
        group.try_arm(&proof(&backend.spec, &node)).unwrap();
        let runtime = group.into_runtime(backend).unwrap();
        assert_eq!(runtime.execution_mode(), ExecutionMode::StagedKeyboard);
        assert_eq!(runtime.progress_epoch(), 0);
        assert!(
            !trace
                .borrow()
                .iter()
                .any(|call| matches!(call, Call::Submit(_, _)))
        );
    }
    #[test]
    fn console_rejection_precedes_all_queries_and_grabs_and_keeps_standby() {
        let trace = Trace::default();
        let mut group = CaptureGroup::new(inputs(vec![source(1, &trace)]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        let proof = proof(&spec, &node);
        for (path, observed, until, now) in [
            (node.input_syspath.clone(), 10, 20, 9),
            (node.input_syspath.clone(), 10, 20, 20),
            (crate::mock_syspath("other-output"), 10, 20, 11),
        ] {
            let ready =
                crate::console::ConsoleReady::for_test(path, MonoTime(observed), MonoTime(until));
            assert!(matches!(
                group.try_arm_console(&proof, &ready, MonoTime(now)),
                Err(ActivationError::ConsoleNotReady)
            ));
            assert_eq!(group.phase(), GroupPhase::Standby);
            assert!(trace.borrow().is_empty());
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn fresh_console_observation_enters_existing_neutral_transaction() {
        let trace = Trace::default();
        let mut group = CaptureGroup::new(inputs(vec![source(1, &trace)]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        let ready = crate::console::ConsoleReady::for_test(
            node.input_syspath.clone(),
            MonoTime(10),
            MonoTime(20),
        );
        let proof = proof(&spec, &node);
        assert_eq!(
            group.try_arm_console(&proof, &ready, MonoTime(11)).unwrap(),
            ArmResult::Captured
        );
        assert_eq!(group.phase(), GroupPhase::Armed);
        assert!(trace.borrow().contains(&Call::Grab(1, true)));
        let count = trace.borrow().len();
        assert!(matches!(
            group.try_arm_console(&proof, &ready, MonoTime(11)),
            Err(ActivationError::Phase)
        ));
        assert_eq!(trace.borrow().len(), count);
    }
    fn neutral() -> io::Result<Bits> {
        Ok(bits(768, &[]))
    }
    #[test]
    fn group_checks_all_devices_before_first_grab_and_transfers_to_runtime() {
        let trace = Trace::default();
        let mut first = source(1, &trace);
        first.reads = [1, 0]
            .into_iter()
            .map(|value| {
                ReadBatch::Events(vec![Event::output(1, 30, value), Event::output(0, 0, 0)])
            })
            .collect();
        let mut group =
            CaptureGroup::new(inputs(vec![first, source(2, &trace)]), config()).unwrap();
        let backend = backend(&trace);
        let node = probe(OUTPUT);
        assert_eq!(
            group.try_arm(&proof(&backend.spec, &node)).unwrap(),
            ArmResult::Captured
        );
        assert_eq!(group.phase(), GroupPhase::Armed);
        assert_eq!(
            *trace.borrow(),
            vec![
                Call::Query(1),
                Call::Query(2),
                Call::Query(1),
                Call::Grab(1, true),
                Call::Query(1),
                Call::Query(2),
                Call::Grab(2, true),
                Call::Query(2),
                Call::Query(1),
                Call::Query(2)
            ]
        );
        let count = trace.borrow().len();
        assert!(matches!(
            group.try_arm(&proof(&backend.spec, &node)),
            Err(ActivationError::Phase)
        ));
        assert_eq!(trace.borrow().len(), count);
        let mut runtime = group.into_runtime(backend).unwrap();
        runtime.poll(MonoTime(1)).unwrap();
        runtime.poll(MonoTime(2)).unwrap();
        runtime.poll(MonoTime(3)).unwrap();
        assert!(
            trace
                .borrow()
                .contains(&Call::Submit(vec![NativeKey { code: 48, value: 1 }], false))
        );
        assert!(
            trace
                .borrow()
                .contains(&Call::Submit(vec![NativeKey { code: 48, value: 0 }], false))
        );
        drop(runtime);
        assert_eq!(
            &trace.borrow()[trace.borrow().len() - 3..],
            &[Call::Close(1), Call::Close(2), Call::OutputClose]
        );
    }
    #[test]
    fn held_global_precheck_never_grabs_and_can_retry_after_neutral() {
        let trace = Trace::default();
        let mut second = source(2, &trace);
        second.queries.push_back(held());
        let mut group =
            CaptureGroup::new(inputs(vec![source(1, &trace), second]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        let proof = proof(&spec, &node);
        assert_eq!(group.try_arm(&proof).unwrap(), ArmResult::WaitingNeutral);
        assert_eq!(*trace.borrow(), vec![Call::Query(1), Call::Query(2)]);
        assert_eq!(group.phase(), GroupPhase::Standby);
        assert_eq!(group.try_arm(&proof).unwrap(), ArmResult::Captured);
    }
    #[test]
    fn later_device_becoming_held_rolls_back_previously_captured_prefix() {
        let trace = Trace::default();
        let mut second = source(2, &trace);
        second.queries = [neutral(), held()].into();
        let mut group =
            CaptureGroup::new(inputs(vec![source(1, &trace), second]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        assert_eq!(
            group.try_arm(&proof(&spec, &node)).unwrap(),
            ArmResult::WaitingNeutral
        );
        assert_eq!(group.phase(), GroupPhase::Standby);
        assert_eq!(trace.borrow().last(), Some(&Call::Grab(1, false)));
        assert!(!trace.borrow().contains(&Call::Grab(2, true)));
        assert!(
            group
                .inputs
                .iter()
                .all(|(_, input)| input.phase() == CapturePhase::Standby)
        );
    }
    #[test]
    fn final_neutral_failure_releases_every_capture_and_retry_is_supported() {
        let trace = Trace::default();
        let mut first = source(1, &trace);
        first.queries = [neutral(), neutral(), neutral(), held()].into();
        let mut group =
            CaptureGroup::new(inputs(vec![first, source(2, &trace)]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        let proof = proof(&spec, &node);
        assert_eq!(group.try_arm(&proof).unwrap(), ArmResult::WaitingNeutral);
        assert_eq!(
            &trace.borrow()[trace.borrow().len() - 2..],
            &[Call::Grab(1, false), Call::Grab(2, false)]
        );
        assert_eq!(group.try_arm(&proof).unwrap(), ArmResult::Captured);
    }
    #[test]
    fn late_capability_error_is_found_before_any_query_or_grab() {
        let trace = Trace::default();
        let mut second = source(2, &trace);
        second.probe.input_syspath = crate::mock_syspath(OUTPUT);
        let mut group =
            CaptureGroup::new(inputs(vec![source(1, &trace), second]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        assert!(matches!(
            group.try_arm(&proof(&spec, &node)),
            Err(ActivationError::Capture {
                device: DeviceInstanceId { id: 2, .. },
                source: CaptureError::OwnOutput
            })
        ));
        assert_eq!(group.phase(), GroupPhase::Closed);
        assert_eq!(*trace.borrow(), vec![Call::Close(1), Call::Close(2)]);
    }
    #[test]
    fn grab_error_and_rollback_error_both_survive_and_all_inputs_close() {
        let trace = Trace::default();
        let mut first = source(1, &trace);
        first.release_fault = true;
        let mut second = source(2, &trace);
        second.grab_fault = true;
        let mut group = CaptureGroup::new(inputs(vec![first, second]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        let error = group.try_arm(&proof(&spec, &node)).unwrap_err();
        let ActivationError::Rollback {
            primary: Some(primary),
            failures,
        } = error
        else {
            panic!("missing primary/rollback failure")
        };
        assert!(
            matches!(*primary, ActivationError::Capture { device: DeviceInstanceId {id:2,..}, source: CaptureError::Grab(ref error) } if error.raw_os_error()==Some(1))
        );
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0.id, 1);
        assert!(
            matches!(failures[0].1, CaptureError::Rollback {ref error,..} if error.raw_os_error()==Some(5))
        );
        assert_eq!(group.phase(), GroupPhase::Closed);
        assert_eq!(
            &trace.borrow()[trace.borrow().len() - 2..],
            &[Call::Close(1), Call::Close(2)]
        );
        assert!(matches!(
            group.try_arm(&proof(&spec, &node)),
            Err(ActivationError::Phase)
        ));
    }
    #[test]
    fn query_fault_after_all_grabs_releases_all_then_closes_all() {
        let trace = Trace::default();
        let mut second = source(2, &trace);
        second.queries = [
            neutral(),
            neutral(),
            neutral(),
            Err(io::Error::from_raw_os_error(5)),
        ]
        .into();
        let mut group =
            CaptureGroup::new(inputs(vec![source(1, &trace), second]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        assert!(matches!(
            group.try_arm(&proof(&spec, &node)),
            Err(ActivationError::Capture {
                device: DeviceInstanceId { id: 2, .. },
                source: CaptureError::Query(_)
            })
        ));
        assert_eq!(
            &trace.borrow()[trace.borrow().len() - 4..],
            &[
                Call::Grab(1, false),
                Call::Grab(2, false),
                Call::Close(1),
                Call::Close(2)
            ]
        );
    }
    #[test]
    fn duplicate_identity_path_and_capacity_reject_and_close_without_io() {
        for kind in 0..4 {
            let trace = Trace::default();
            let sources = match kind {
                0 => vec![source(1, &trace), source(1, &trace)],
                1 => {
                    let mut second = source(2, &trace);
                    second.probe.input_syspath =
                        crate::mock_syspath("/sys/devices/mock/input/input1");
                    vec![source(1, &trace), second]
                }
                2 => (1..=MAX_DEVICES as u64 + 1)
                    .map(|id| source(id, &trace))
                    .collect(),
                _ => vec![],
            };
            let count = sources.len();
            assert!(matches!(
                CaptureGroup::new(inputs(sources), config()),
                Err(ActivationError::Inputs)
            ));
            assert_eq!(trace.borrow().len(), count);
            assert!(
                trace
                    .borrow()
                    .iter()
                    .all(|call| matches!(call, Call::Close(_)))
            );
        }
    }
    #[test]
    fn configuration_timing_and_output_coverage_reject_before_grab() {
        let trace = Trace::default();
        let mut invalid = (*config()).clone();
        invalid.simple[0].to[0].hold_down_ns = 1;
        assert!(matches!(
            CaptureGroup::new(inputs(vec![source(1, &trace)]), Arc::new(invalid)),
            Err(ActivationError::Engine(_))
        ));
        assert_eq!(*trace.borrow(), vec![Call::Close(1)]);
        trace.borrow_mut().clear();
        let spec = DeviceSpec::new("limited", &[30], RepeatPolicy::Kernel, false).unwrap();
        let mut node = probe(OUTPUT);
        node.capabilities.keys = bits(768, &[30]);
        let mut group = CaptureGroup::new(inputs(vec![source(1, &trace)]), config()).unwrap();
        assert!(matches!(
            group.try_arm(&proof(&spec, &node)),
            Err(ActivationError::Runtime(RuntimeError::ConfigurationKey(_)))
        ));
        assert_eq!(*trace.borrow(), vec![Call::Close(1)]);
    }
    #[test]
    fn runtime_transfer_rejects_unarmed_or_different_backend_and_closes_input_first() {
        for armed in [false, true] {
            let trace = Trace::default();
            let mut group =
                CaptureGroup::new(inputs(vec![source(1, &trace), source(2, &trace)]), config())
                    .unwrap();
            let mut backend = backend(&trace);
            if armed {
                let node = probe(OUTPUT);
                group.try_arm(&proof(&backend.spec, &node)).unwrap();
                backend.path = crate::mock_syspath("/sys/devices/virtual/input/input998");
            }
            assert!(group.into_runtime(backend).is_err());
            assert_eq!(
                &trace.borrow()[trace.borrow().len() - 3..],
                &[Call::Close(1), Call::Close(2), Call::OutputClose]
            );
        }
    }
    #[test]
    fn waiting_neutral_with_multiple_rollback_errors_closes_every_failed_source() {
        let trace = Trace::default();
        let mut first = source(1, &trace);
        first.release_fault = true;
        first.queries = [neutral(), neutral(), neutral(), held()].into();
        let mut second = source(2, &trace);
        second.release_fault = true;
        let mut group = CaptureGroup::new(inputs(vec![first, second]), config()).unwrap();
        let spec = spec();
        let node = probe(OUTPUT);
        let error = group.try_arm(&proof(&spec, &node)).unwrap_err();
        let ActivationError::Rollback {
            primary: None,
            failures,
        } = error
        else {
            panic!("unexpected error chain")
        };
        assert_eq!(
            failures.iter().map(|(id, _)| id.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            &trace.borrow()[trace.borrow().len() - 4..],
            &[
                Call::Grab(1, false),
                Call::Close(1),
                Call::Grab(2, false),
                Call::Close(2)
            ]
        );
        assert_eq!(group.phase(), GroupPhase::Closed);
    }
    #[test]
    fn unreachable_emergency_and_external_repeat_fail_before_neutral_queries() {
        for external in [false, true] {
            let trace = Trace::default();
            let mut input = source(1, &trace);
            if !external {
                input.probe.capabilities.keys = bits(768, &[30, 48]);
            }
            let mut group = CaptureGroup::new(inputs(vec![input]), config()).unwrap();
            let mut node = probe(OUTPUT);
            let spec = DeviceSpec::new(
                "group",
                &keys::native_codes().collect::<Vec<_>>(),
                if external {
                    RepeatPolicy::External
                } else {
                    RepeatPolicy::Kernel
                },
                false,
            )
            .unwrap();
            if external {
                node.capabilities.events = bits(32, &[0, 1]);
            }
            let error = group.try_arm(&proof(&spec, &node)).unwrap_err();
            assert!(matches!(
                (external, error),
                (true, ActivationError::Runtime(RuntimeError::RepeatPolicy))
                    | (
                        false,
                        ActivationError::Runtime(RuntimeError::EmergencyUnavailable)
                    )
            ));
            assert_eq!(*trace.borrow(), vec![Call::Close(1)]);
        }
    }
    fn mixed_spec() -> DeviceSpec {
        spec()
            .with_relative_axes(crate::relative::RelativeAxes::new(&[0, 1]).unwrap())
            .with_pointing_buttons(keys::PointingButtons::new(&[0x110]).unwrap())
            .unwrap()
    }
    fn mixed_node(spec: &DeviceSpec) -> Probe {
        let mut node = probe(OUTPUT);
        node.capabilities.keys = bits(768, &spec.codes().collect::<Vec<_>>());
        node.capabilities.events = bits(32, &[0, 1, 2, 20]);
        node.capabilities.relative = bits(16, &[0, 1]);
        node.capabilities.properties = bits(32, &[0]);
        node
    }
    fn mouse(id: u64, trace: &Trace) -> Source {
        let mut source = source(id, trace);
        source.probe.capabilities.keys = bits(768, &[0x110]);
        source.probe.capabilities.events = bits(32, &[0, 1, 2]);
        source.probe.capabilities.relative = bits(16, &[0, 1]);
        source.probe.capabilities.properties = bits(32, &[0]);
        source
    }
    fn mixed_inputs(trace: &Trace) -> Vec<(DeviceInstanceId, CaptureDevice<Source>)> {
        vec![
            (
                DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                CaptureDevice::new(source(1, trace)),
            ),
            (
                DeviceInstanceId {
                    id: 2,
                    generation: 1,
                },
                CaptureDevice::new_with_relative(mouse(2, trace)),
            ),
        ]
    }
    #[test]
    fn mixed_group_arms_keyboard_and_mouse_then_hands_to_relative_runtime() {
        let trace = Trace::default();
        let spec = mixed_spec();
        let node = mixed_node(&spec);
        let mut group = CaptureGroup::new_staged_relative(mixed_inputs(&trace), config()).unwrap();
        assert_eq!(
            group.try_arm(&proof(&spec, &node)).unwrap(),
            ArmResult::Captured
        );
        assert_eq!(
            trace
                .borrow()
                .iter()
                .filter(|call| matches!(call, Call::Grab(_, true)))
                .count(),
            2
        );
        let backend = Backend {
            spec,
            path: crate::mock_syspath(OUTPUT),
            trace: trace.clone(),
        };
        let runtime = group.into_runtime(backend).unwrap();
        assert!(runtime.relative_enabled());
        drop(runtime);
        let calls = trace.borrow();
        let output_close = calls
            .iter()
            .position(|call| *call == Call::OutputClose)
            .unwrap();
        assert!(
            calls
                .iter()
                .enumerate()
                .filter(|(_, call)| matches!(call, Call::Close(_)))
                .all(|(index, _)| index < output_close)
        );
    }
    #[test]
    fn default_group_cannot_select_relative_capture() {
        let trace = Trace::default();
        assert!(matches!(
            CaptureGroup::new(mixed_inputs(&trace), config()),
            Err(ActivationError::Inputs)
        ));
        assert!(
            !trace
                .borrow()
                .iter()
                .any(|call| matches!(call, Call::Grab(_, _) | Call::Query(_)))
        );
    }
    #[test]
    fn mouse_only_group_rejects_before_state_query_or_grab() {
        let trace = Trace::default();
        let spec = mixed_spec();
        let node = mixed_node(&spec);
        let inputs = vec![(
            DeviceInstanceId {
                id: 2,
                generation: 1,
            },
            CaptureDevice::new_with_relative(mouse(2, &trace)),
        )];
        let mut group = CaptureGroup::new_staged_relative(inputs, config()).unwrap();
        assert!(matches!(
            group.try_arm(&proof(&spec, &node)),
            Err(ActivationError::Runtime(RuntimeError::EmergencyUnavailable))
        ));
        assert!(
            !trace
                .borrow()
                .iter()
                .any(|call| matches!(call, Call::Grab(_, _) | Call::Query(_)))
        );
    }
    #[test]
    fn mixed_missing_axis_is_rejected_before_grab() {
        let trace = Trace::default();
        let spec = spec().with_relative_axes(crate::relative::RelativeAxes::new(&[0]).unwrap());
        let mut node = probe(OUTPUT);
        node.capabilities.events = bits(32, &[0, 1, 2, 20]);
        node.capabilities.relative = bits(16, &[0]);
        let mut mouse = mouse(2, &trace);
        mouse.probe.capabilities.keys = bits(768, &[30]);
        mouse.probe.capabilities.properties = bits(32, &[]);
        let inputs = vec![
            (
                DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                CaptureDevice::new(source(1, &trace)),
            ),
            (
                DeviceInstanceId {
                    id: 2,
                    generation: 1,
                },
                CaptureDevice::new_with_relative(mouse),
            ),
        ];
        let mut group = CaptureGroup::new_staged_relative(inputs, config()).unwrap();
        assert!(matches!(
            group.try_arm(&proof(&spec, &node)),
            Err(ActivationError::Capture {
                source: CaptureError::MissingOutputAxis(1),
                ..
            })
        ));
        assert!(
            !trace
                .borrow()
                .iter()
                .any(|call| matches!(call, Call::Grab(_, _) | Call::Query(_)))
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn console_observation_cannot_admit_mixed_group() {
        let trace = Trace::default();
        let spec = mixed_spec();
        let node = mixed_node(&spec);
        let mut group = CaptureGroup::new_staged_relative(mixed_inputs(&trace), config()).unwrap();
        let ready = crate::console::ConsoleReady::for_test(
            crate::mock_syspath(OUTPUT),
            MonoTime(0),
            MonoTime(100),
        );
        assert!(matches!(
            group.try_arm_console(&proof(&spec, &node), &ready, MonoTime(1)),
            Err(ActivationError::ConsoleNotReady)
        ));
        assert!(
            !trace
                .borrow()
                .iter()
                .any(|call| matches!(call, Call::Grab(_, _) | Call::Query(_)))
        );
    }
    #[test]
    fn mouse_held_during_arm_rolls_back_keyboard_prefix_and_can_retry() {
        let trace = Trace::default();
        let spec = mixed_spec();
        let node = mixed_node(&spec);
        let mut mouse = mouse(2, &trace);
        mouse.queries = [Ok(bits(768, &[])), Ok(bits(768, &[0x110]))].into();
        let inputs = vec![
            (
                DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                CaptureDevice::new(source(1, &trace)),
            ),
            (
                DeviceInstanceId {
                    id: 2,
                    generation: 1,
                },
                CaptureDevice::new_with_relative(mouse),
            ),
        ];
        let mut group = CaptureGroup::new_staged_relative(inputs, config()).unwrap();
        assert_eq!(
            group.try_arm(&proof(&spec, &node)).unwrap(),
            ArmResult::WaitingNeutral
        );
        assert_eq!(group.phase(), GroupPhase::Standby);
        let grabs: Vec<_> = trace
            .borrow()
            .iter()
            .filter(|call| matches!(call, Call::Grab(_, _)))
            .cloned()
            .collect();
        assert_eq!(grabs, vec![Call::Grab(1, true), Call::Grab(1, false)]);
        assert_eq!(
            group.try_arm(&proof(&spec, &node)).unwrap(),
            ArmResult::Captured
        );
    }
    #[test]
    fn mixed_scope_requires_declared_output_axes_before_keyboard_capture() {
        let trace = Trace::default();
        let spec = spec();
        let node = probe(OUTPUT);
        let mut group =
            CaptureGroup::new_staged_relative(inputs(vec![source(1, &trace)]), config()).unwrap();
        assert!(matches!(
            group.try_arm(&proof(&spec, &node)),
            Err(ActivationError::Runtime(RuntimeError::StagedPointer))
        ));
        assert!(
            !trace
                .borrow()
                .iter()
                .any(|call| matches!(call, Call::Grab(_, _) | Call::Query(_)))
        );
    }
}
