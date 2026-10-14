use super::*;
use crate::{
    events::{FrameWrite, NativeKey},
    keys,
    output::WriteError,
    probe::{Bits, Capabilities, DeviceId},
    read::ReadBatch,
};
use keyweave_types::ConfigGeneration;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};
const OUTPUT: &str = "/sys/devices/virtual/input/input999";
type Trace = Rc<RefCell<Vec<String>>>;
fn record(trace: &Trace, event: impl Into<String>) {
    trace.borrow_mut().push(event.into());
}
fn bits(codes: &[u16], count: usize) -> Bits {
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
        input_syspath: path.into(),
        capabilities: Capabilities {
            relative: crate::probe::Bits::empty(16).unwrap_or_else(|error| panic!("{error}")),

            events: bits(&[0, 1, 20], 32),
            keys: bits(&keys::native_codes().collect::<Vec<_>>(), 768),
            misc: bits(&[], 8),
            leds: bits(&[], 16),
            properties: bits(&[], 32),
        },
    }
}
struct Source {
    probe: Probe,
    trace: Trace,
    held: Rc<Cell<bool>>,
    number: usize,
}
impl CaptureSource for Source {
    fn probe(&self) -> &Probe {
        &self.probe
    }
    fn pressed_keys(&mut self) -> io::Result<Bits> {
        record(&self.trace, format!("query{}", self.number));
        Ok(bits(if self.held.get() { &[30] } else { &[] }, 768))
    }
    fn set_grab(&mut self, grabbed: bool) -> io::Result<()> {
        record(&self.trace, format!("grab{}:{grabbed}", self.number));
        Ok(())
    }
    fn read_batch(&mut self) -> io::Result<ReadBatch> {
        record(&self.trace, "read");
        Ok(ReadBatch::WouldBlock)
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        record(&self.trace, format!("close{}", self.number));
    }
}
struct Output {
    spec: DeviceSpec,
    trace: Trace,
}
impl FrameBackend for Output {
    fn spec(&self) -> &DeviceSpec {
        &self.spec
    }
    fn input_syspath(&self) -> Option<&Path> {
        Some(Path::new(OUTPUT))
    }
    fn submit(
        &mut self,
        edges: &[NativeKey],
        _: bool,
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        record(&self.trace, "write");
        Ok(Ok(FrameWrite::Complete {
            accepted: edges.len(),
        }))
    }
    fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        Ok(Ok(true))
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        record(&self.trace, "output-close");
    }
}
#[derive(Clone, Copy)]
enum Observation {
    Ready,
    Missing,
    Wrong,
    Stale,
    Error,
}
struct Mock {
    trace: Trace,
    clock: Rc<Cell<u64>>,
    times: VecDeque<u64>,
    held: Rc<Cell<bool>>,
    missing_node: bool,
    bad_node: bool,
    fail: Option<&'static str>,
    opened: usize,
    observations: VecDeque<Observation>,
}
impl Mock {
    fn new() -> Self {
        Self {
            trace: Default::default(),
            clock: Rc::new(Cell::new(1)),
            times: VecDeque::new(),
            held: Default::default(),
            missing_node: false,
            bad_node: false,
            fail: None,
            opened: 0,
            observations: VecDeque::new(),
        }
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        record(&self.trace, "provider-close");
    }
}
impl Platform for Mock {
    type Source = Source;
    type Output = Output;
    fn verify(&mut self) -> io::Result<()> {
        if self.fail == Some("guard") {
            return Err(io::Error::other("guard"));
        }
        Ok(())
    }
    fn now(&mut self) -> io::Result<MonoTime> {
        Ok(MonoTime(self.times.pop_front().unwrap_or(self.clock.get())))
    }
    fn create(&mut self, spec: DeviceSpec) -> io::Result<Output> {
        record(&self.trace, "create");
        if self.fail == Some("create") {
            return Err(io::Error::other("create"));
        }
        Ok(Output {
            spec,
            trace: self.trace.clone(),
        })
    }
    fn discover(&mut self, _: &Output) -> io::Result<Option<Probe>> {
        record(&self.trace, "discover");
        if self.fail == Some("discover") {
            return Err(io::Error::other("discover"));
        }
        if self.missing_node {
            return Ok(None);
        }
        Ok(Some(probe(if self.bad_node {
            "/sys/devices/virtual/input/input998"
        } else {
            OUTPUT
        })))
    }
    fn open(&mut self, _: &Path) -> io::Result<CaptureDevice<Source>> {
        self.opened += 1;
        record(&self.trace, format!("open{}", self.opened));
        if self.fail == Some("open2") && self.opened == 2 {
            return Err(io::Error::other("open"));
        }
        Ok(CaptureDevice::new(Source {
            probe: probe(&format!("/sys/devices/mock/input/input{}", self.opened)),
            trace: self.trace.clone(),
            held: self.held.clone(),
            number: self.opened,
        }))
    }
    fn observe(&mut self, _: &Output) -> io::Result<Option<ConsoleReady>> {
        record(&self.trace, "observe");
        let now = self.clock.get();
        match self.observations.pop_front().unwrap_or(Observation::Ready) {
            Observation::Missing => Ok(None),
            Observation::Error => Err(io::Error::other("observe")),
            Observation::Ready => Ok(Some(ConsoleReady::for_test(
                OUTPUT.into(),
                MonoTime(now),
                MonoTime(now + 250_000_000),
            ))),
            Observation::Wrong => Ok(Some(ConsoleReady::for_test(
                "/sys/devices/virtual/input/input998".into(),
                MonoTime(now),
                MonoTime(now + 250_000_000),
            ))),
            Observation::Stale => Ok(Some(ConsoleReady::for_test(
                OUTPUT.into(),
                MonoTime(now),
                MonoTime(now),
            ))),
        }
    }
}
fn config() -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "bootstrap".into(),
        simple: vec![],
        complex: vec![],
    })
}
fn spec() -> DeviceSpec {
    DeviceSpec::new(
        "bootstrap",
        &keys::native_codes().collect::<Vec<_>>(),
        RepeatPolicy::Kernel,
        false,
    )
    .unwrap()
}
fn inputs() -> Vec<(DeviceInstanceId, PathBuf)> {
    (1..=2)
        .map(|id| {
            (
                DeviceInstanceId { id, generation: 1 },
                format!("/dev/input/event{id}").into(),
            )
        })
        .collect()
}
fn bootstrap(mock: Mock) -> Bootstrap<Mock> {
    Bootstrap::new(Plan::new(inputs(), config(), spec()).unwrap(), mock)
}
fn drive(boot: &mut Bootstrap<Mock>) -> Result<(), BootstrapError> {
    for _ in 0..32 {
        if boot.step()? == BootstrapStep::Ready {
            return Ok(());
        }
    }
    panic!("fixture failed to converge")
}
fn inputs_closed_before_output(trace: &Trace) {
    let trace = trace.borrow();
    let output = trace.iter().position(|v| v == "output-close").unwrap();
    for (index, event) in trace.iter().enumerate() {
        if event.starts_with("close") {
            assert!(index < output);
        }
    }
    let provider = trace.iter().position(|v| v == "provider-close").unwrap();
    assert!(output < provider);
    assert!(!trace.iter().any(|v| v == "read" || v == "write"));
}
#[test]
fn admission_observes_before_and_after_grab_and_hands_off_without_dispatch() {
    let mock = Mock::new();
    let trace = mock.trace.clone();
    let mut boot = bootstrap(mock);
    drive(&mut boot).unwrap();
    let Handoff {
        runtime,
        platform,
        ready,
    } = boot.finish().unwrap();
    assert_eq!(runtime.progress_epoch(), 0);
    assert!(ready.fresh_for(Path::new(OUTPUT), MonoTime(1)));
    let events = trace.borrow();
    let first_grab = events.iter().position(|v| v == "grab1:true").unwrap();
    let last_grab = events.iter().rposition(|v| v.ends_with(":true")).unwrap();
    assert!(events.iter().position(|v| v == "open2").unwrap() < first_grab);
    assert!(events.iter().position(|v| v == "observe").unwrap() < first_grab);
    assert!(events.iter().rposition(|v| v == "observe").unwrap() > last_grab);
    assert!(!events.iter().any(|v| v == "read" || v == "write"));
    drop(events);
    drop(runtime);
    drop(platform);
    inputs_closed_before_output(&trace);
}
#[test]
fn pending_discovery_and_held_keys_never_bypass_consumer_or_neutral_checks() {
    let mut mock = Mock::new();
    mock.missing_node = true;
    mock.held.set(true);
    let trace = mock.trace.clone();
    let held = mock.held.clone();
    let mut boot = bootstrap(mock);
    for _ in 0..3 {
        assert_eq!(boot.step().unwrap(), BootstrapStep::Pending);
    }
    assert!(!trace.borrow().iter().any(|v| v.starts_with("open")));
    boot.platform.as_mut().unwrap().missing_node = false;
    for _ in 0..6 {
        assert_eq!(boot.step().unwrap(), BootstrapStep::Pending);
    }
    assert!(!trace.borrow().iter().any(|v| v.starts_with("grab")));
    held.set(false);
    drive(&mut boot).unwrap();
    drop(boot);
    inputs_closed_before_output(&trace);
}
#[test]
fn missing_wrong_and_stale_evidence_do_no_neutral_queries_until_ready() {
    let mut mock = Mock::new();
    mock.observations = [Observation::Missing, Observation::Wrong, Observation::Stale].into();
    let trace = mock.trace.clone();
    let mut boot = bootstrap(mock);
    for _ in 0..8 {
        assert_eq!(boot.step().unwrap(), BootstrapStep::Pending);
    }
    assert!(
        !trace
            .borrow()
            .iter()
            .any(|v| v.starts_with("query") || v.starts_with("grab"))
    );
    drive(&mut boot).unwrap();
    drop(boot);
    inputs_closed_before_output(&trace);
}
#[test]
fn lost_or_failed_post_grab_evidence_closes_inputs_before_output() {
    for observation in [
        Observation::Missing,
        Observation::Wrong,
        Observation::Stale,
        Observation::Error,
    ] {
        let mut mock = Mock::new();
        mock.observations = [Observation::Ready, observation].into();
        let trace = mock.trace.clone();
        let mut boot = bootstrap(mock);
        assert!(drive(&mut boot).is_err());
        assert!(trace.borrow().iter().any(|v| v == "grab1:true"));
        assert!(trace.borrow().iter().any(|v| v == "close2"));
        inputs_closed_before_output(&trace);
        let count = trace.borrow().len();
        assert!(boot.step().is_err());
        assert_eq!(trace.borrow().len(), count);
    }
}
#[test]
fn partial_open_and_node_mismatch_cleanup_do_not_grab() {
    for failure in ["discover", "open2", "bad-node"] {
        let mut mock = Mock::new();
        mock.fail = Some(failure);
        mock.bad_node = failure == "bad-node";
        let trace = mock.trace.clone();
        let mut boot = bootstrap(mock);
        assert!(drive(&mut boot).is_err());
        inputs_closed_before_output(&trace);
        assert!(!trace.borrow().iter().any(|v| v.starts_with("grab")));
    }
    for failure in ["guard", "create"] {
        let mut mock = Mock::new();
        mock.fail = Some(failure);
        let trace = mock.trace.clone();
        assert!(bootstrap(mock).step().is_err());
        assert!(
            !trace
                .borrow()
                .iter()
                .any(|v| v.starts_with("open") || v.starts_with("grab"))
        );
        if failure == "guard" {
            assert_eq!(*trace.borrow(), vec!["provider-close"]);
        }
    }
}
#[test]
fn deadline_clock_faults_and_attempt_budget_are_finite() {
    for (times, error) in [
        ([0, BOOTSTRAP_TIMEOUT_NS], "deadline"),
        ([2, 1], "clock"),
        ([u64::MAX, u64::MAX], "overflow"),
    ] {
        let mut mock = Mock::new();
        mock.times = times.into();
        let trace = mock.trace.clone();
        let mut boot = bootstrap(mock);
        let result = boot.step();
        assert!(match error {
            "clock" => matches!(result, Err(BootstrapError::Clock)),
            _ => matches!(result, Err(BootstrapError::Deadline)),
        });
        if error != "overflow" {
            inputs_closed_before_output(&trace);
        } else {
            assert_eq!(*trace.borrow(), vec!["provider-close"]);
        }
    }
    let mut mock = Mock::new();
    mock.missing_node = true;
    let trace = mock.trace.clone();
    let mut boot = bootstrap(mock);
    for _ in 0..MAX_BOOTSTRAP_STEPS {
        assert_eq!(boot.step().unwrap(), BootstrapStep::Pending);
    }
    assert!(matches!(boot.step(), Err(BootstrapError::Attempts)));
    inputs_closed_before_output(&trace);
}
#[test]
fn expired_handoff_and_drop_after_grab_never_dispatch() {
    let mock = Mock::new();
    let clock = mock.clock.clone();
    let trace = mock.trace.clone();
    let mut boot = bootstrap(mock);
    drive(&mut boot).unwrap();
    clock.set(250_000_001);
    assert!(matches!(boot.finish(), Err(BootstrapError::ConsumerLost)));
    inputs_closed_before_output(&trace);
    let mock = Mock::new();
    let trace = mock.trace.clone();
    let mut boot = bootstrap(mock);
    for _ in 0..6 {
        assert_eq!(boot.step().unwrap(), BootstrapStep::Pending);
    }
    assert!(trace.borrow().iter().any(|v| v == "grab2:true"));
    drop(boot);
    inputs_closed_before_output(&trace);
}
#[test]
fn cold_plan_rejects_ambiguous_or_invalid_identities() {
    for bad in [
        vec![],
        vec![(
            DeviceInstanceId {
                id: 0,
                generation: 1,
            },
            "/dev/input/event1".into(),
        )],
        vec![(
            DeviceInstanceId {
                id: 1,
                generation: 0,
            },
            "/dev/input/event1".into(),
        )],
        vec![(
            DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            "relative".into(),
        )],
        vec![inputs()[0].clone(), inputs()[0].clone()],
    ] {
        assert!(matches!(
            Plan::new(bad, config(), spec()),
            Err(BootstrapError::Inputs)
        ));
    }
    let mut same_id = inputs();
    same_id[1].0.id = 1;
    assert!(matches!(
        Plan::new(same_id, config(), spec()),
        Err(BootstrapError::Inputs)
    ));
    let mut same_path = inputs();
    same_path[1].1 = same_path[0].1.clone();
    assert!(matches!(
        Plan::new(same_path, config(), spec()),
        Err(BootstrapError::Inputs)
    ));
}
#[test]
fn post_observation_clock_and_late_ready_fault_close_without_dispatch() {
    let mock = Mock::new();
    let trace = mock.trace.clone();
    let mut boot = bootstrap(mock);
    for _ in 0..5 {
        assert_eq!(boot.step().unwrap(), BootstrapStep::Pending);
    }
    boot.platform.as_mut().unwrap().times = [2, 1].into();
    assert!(matches!(boot.step(), Err(BootstrapError::Clock)));
    assert!(!trace.borrow().iter().any(|v| v.starts_with("grab")));
    inputs_closed_before_output(&trace);
    let mock = Mock::new();
    let trace = mock.trace.clone();
    let mut boot = bootstrap(mock);
    for _ in 0..6 {
        assert_eq!(boot.step().unwrap(), BootstrapStep::Pending);
    }
    boot.platform.as_mut().unwrap().times = [1, 1, 250_000_001].into();
    assert!(matches!(boot.step(), Err(BootstrapError::ConsumerLost)));
    inputs_closed_before_output(&trace);
}
struct StartupReader {
    bytes: Vec<u8>,
    delay: usize,
}
impl io::Read for StartupReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.delay != 0 {
            self.delay -= 1;
            return Err(io::ErrorKind::WouldBlock.into());
        }
        if self.bytes.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = buffer.len().min(self.bytes.len());
        buffer[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes.drain(..count);
        Ok(count)
    }
}
struct SharedWriter(Rc<RefCell<Vec<u8>>>);
impl io::Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn startup_stop_precedes_creation_or_cancels_captured_owners_without_dispatch() {
    for delay in [0, 6] {
        let mock = Mock::new();
        let trace = mock.trace.clone();
        let mut client = crate::progress::ProgressClient::new(
            StartupReader {
                bytes: crate::progress::StopRequest { run_id: 7, id: 1 }
                    .encode()
                    .to_vec(),
                delay,
            },
            io::sink(),
            7,
        )
        .unwrap();
        assert!(
            drive_controlled(bootstrap(mock), &mut client, |_, _| Ok(()))
                .unwrap()
                .is_none()
        );
        if delay == 0 {
            assert_eq!(*trace.borrow(), vec!["provider-close"]);
        } else {
            assert!(trace.borrow().iter().any(|v| v == "grab2:true"));
            inputs_closed_before_output(&trace);
        }
    }
}
#[test]
fn startup_challenge_survives_handoff_without_metadata_heartbeat() {
    let mock = Mock::new();
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut client = crate::progress::ProgressClient::new(
        StartupReader {
            bytes: crate::progress::Challenge { run_id: 7, id: 1 }
                .encode()
                .to_vec(),
            delay: 0,
        },
        SharedWriter(bytes.clone()),
        7,
    )
    .unwrap();
    let Handoff {
        mut runtime,
        platform,
        ..
    } = drive_controlled(bootstrap(mock), &mut client, |_, _| Ok(()))
        .unwrap()
        .unwrap();
    assert!(bytes.borrow().is_empty());
    assert_eq!(runtime.progress_epoch(), 0);
    runtime.poll(MonoTime(1)).unwrap();
    assert!(client.publish(&runtime).unwrap());
    let receipt = crate::progress::Receipt::decode(&bytes.borrow()).unwrap();
    assert_eq!(receipt.challenge.run_id, 7);
    assert_eq!(receipt.challenge.id, 1);
    assert_eq!(receipt.epoch, 1);
    drop(runtime);
    drop(platform);
}
#[test]
fn startup_protocol_and_wait_faults_close_without_retries() {
    let mock = Mock::new();
    let trace = mock.trace.clone();
    let mut client = crate::progress::ProgressClient::new(
        StartupReader {
            bytes: vec![0; 24],
            delay: 0,
        },
        io::sink(),
        7,
    )
    .unwrap();
    assert!(matches!(
        drive_controlled(bootstrap(mock), &mut client, |_, _| panic!(
            "malformed request cannot wait"
        )),
        Err(BootstrapError::Progress(_))
    ));
    assert_eq!(*trace.borrow(), vec!["provider-close"]);
    let mock = Mock::new();
    let trace = mock.trace.clone();
    let mut waits = 0;
    let mut client = crate::progress::ProgressClient::new(
        StartupReader {
            bytes: vec![],
            delay: 0,
        },
        io::sink(),
        7,
    )
    .unwrap();
    assert!(matches!(
        drive_controlled(bootstrap(mock), &mut client, |_, _| {
            waits += 1;
            if waits == 6 {
                Err(BootstrapError::Endpoint)
            } else {
                Ok(())
            }
        }),
        Err(BootstrapError::Endpoint)
    ));
    assert_eq!(waits, 6);
    assert!(trace.borrow().iter().any(|v| v == "grab2:true"));
    inputs_closed_before_output(&trace);
}
#[test]
fn staged_plan_propagates_mode_through_bootstrap_group_and_handoff_without_dispatch() {
    let mut cfg = (*config()).clone();
    cfg.complex.push(keyweave_engine::Mapping {
        id: "timed".into(),
        from: "a".parse().unwrap(),
        to: vec![
            keyweave_engine::Action {
                hold_down_ns: 50,
                .."b".parse::<keyweave_types::KeyId>().unwrap().into()
            },
            "c".parse::<keyweave_types::KeyId>().unwrap().into(),
        ],
        allow_extra_modifiers: true,
        extras: Default::default(),
    });
    let cfg = Arc::new(cfg);
    assert!(matches!(
        Plan::new(inputs(), cfg.clone(), spec()),
        Err(BootstrapError::Engine(EngineError::UnsupportedActionTiming))
    ));
    let mock = Mock::new();
    let trace = mock.trace.clone();
    let plan = Plan::new_with_mode(
        inputs(),
        cfg,
        spec(),
        crate::runtime::ExecutionMode::StagedKeyboard,
    )
    .unwrap();
    let mut boot = Bootstrap::new(plan, mock);
    drive(&mut boot).unwrap();
    let Handoff {
        runtime,
        platform,
        ready: _,
    } = boot.finish().unwrap();
    assert_eq!(
        runtime.execution_mode(),
        crate::runtime::ExecutionMode::StagedKeyboard
    );
    assert_eq!(runtime.progress_epoch(), 0);
    assert!(
        !trace
            .borrow()
            .iter()
            .any(|event| event == "read" || event == "write")
    );
    drop(runtime);
    drop(platform);
    inputs_closed_before_output(&trace);
}
