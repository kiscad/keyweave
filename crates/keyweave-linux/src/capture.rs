//! Single-device capture admission. No event loop, consumer barrier or watchdog.
use crate::{
    events::KEY_MAX,
    output::DeviceSpec,
    probe::{AdmissionIssue, Bits, Probe},
    read::ReadBatch,
};
use std::{io, path::Path};

/// Short-lived evidence that the owned output has a matching evdev node.
/// Does not prove that desktop consumers have opened it. Private construction.
pub struct OutputNodeReady<'a> {
    spec: &'a DeviceSpec,
    input_syspath: &'a Path,
}
impl<'a> OutputNodeReady<'a> {
    #[cfg(target_os = "linux")]
    pub(crate) fn input_syspath(&self) -> &Path {
        self.input_syspath
    }
    pub(crate) fn spec(&self) -> &DeviceSpec {
        self.spec
    }
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn verify(
        spec: &'a DeviceSpec,
        input_syspath: &'a Path,
        node: &Probe,
    ) -> Result<Self, CaptureError> {
        let expected: Vec<_> = spec.codes().collect();
        let events: Vec<_> = [0, 1]
            .into_iter()
            .chain(spec.relative_axes().map(|_| crate::relative::EV_REL))
            .chain(spec.leds().then_some(17))
            .chain((spec.repeat() == crate::output::RepeatPolicy::Kernel).then_some(20))
            .collect();
        let leds: Vec<_> = if spec.leds() { vec![0, 1, 2] } else { vec![] };
        if node.input_syspath != input_syspath
            || !input_syspath.is_absolute()
            || !complete_capabilities(node)
            || node.id.bus != 6
            || node.id.vendor != 0
            || node.id.product != 0
            || node.id.version != 1
            || node.capabilities.keys.codes().collect::<Vec<_>>() != expected
            || node.capabilities.events.codes().collect::<Vec<_>>() != events
            || node.capabilities.leds.codes().collect::<Vec<_>>() != leds
            || node.capabilities.relative.codes().collect::<Vec<_>>()
                != spec
                    .relative_axes()
                    .map_or_else(Vec::new, |axes| axes.codes().collect())
            || !node.capabilities.misc.is_empty()
            || !node.capabilities.properties.is_empty()
        {
            return Err(CaptureError::OutputNodeMismatch);
        }
        Ok(Self {
            spec,
            input_syspath,
        })
    }
}
/// Owns one client. Production construction is through CaptureDevice::open;
/// its FD cannot escape. Drop must close the client and release any grab.
pub trait CaptureSource {
    fn probe(&self) -> &Probe;
    /// Reconciliation query may flush queued keys; never use during active reads.
    fn pressed_keys(&mut self) -> io::Result<Bits>;
    fn set_grab(&mut self, grabbed: bool) -> io::Result<()>;
    fn read_batch(&mut self) -> io::Result<ReadBatch>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturePhase {
    Standby,
    Captured,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmResult {
    WaitingNeutral,
    Captured,
}
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("output is not ready for capture")]
    OutputNotReady,
    #[error("output evdev node does not match the owned uinput device")]
    OutputNodeMismatch,
    #[error("cannot capture our own virtual output")]
    OwnOutput,
    #[error("capability masks are incomplete")]
    InvalidCapabilities,
    #[error("unsupported keyboard: {0}")]
    Capability(AdmissionIssue),
    #[error("output cannot pass through native key {0}")]
    MissingOutputKey(u16),
    #[error("state mask is incomplete or contains unadvertised keys")]
    InvalidState,
    #[error("input state query failed: {0}")]
    Query(#[source] io::Error),
    #[error("input read failed; source closed: {0}")]
    Read(#[source] io::Error),
    #[error("input grab failed: {0}")]
    Grab(#[source] io::Error),
    #[error("capture rollback failed; input closed: {error}")]
    Rollback {
        primary: Option<Box<CaptureError>>,
        #[source]
        error: io::Error,
    },
    #[error("capture source is closed")]
    Closed,
    #[error("capture state does not permit this operation")]
    InvalidPhase,
}
fn complete_capabilities(node: &Probe) -> bool {
    let caps = &node.capabilities;
    [
        caps.events.count(),
        caps.keys.count(),
        caps.relative.count(),
        caps.misc.count(),
        caps.leds.count(),
        caps.properties.count(),
    ] == [32, usize::from(KEY_MAX) + 1, 16, 8, 16, 32]
}
pub struct CaptureDevice<S: CaptureSource> {
    source: Option<S>,
    phase: CapturePhase,
    output_identity: Option<std::path::PathBuf>,
}
impl<S: CaptureSource> CaptureDevice<S> {
    pub fn new(source: S) -> Self {
        Self {
            source: Some(source),
            phase: CapturePhase::Standby,
            output_identity: None,
        }
    }
    /// Identity of the output against which this active capture was admitted.
    pub fn output_syspath(&self) -> Option<&Path> {
        self.output_identity.as_deref()
    }
    pub fn phase(&self) -> CapturePhase {
        self.phase
    }
    pub fn probe(&self) -> Option<&Probe> {
        self.source.as_ref().map(CaptureSource::probe)
    }
    fn source(&mut self) -> Result<&mut S, CaptureError> {
        self.source.as_mut().ok_or(CaptureError::Closed)
    }
    pub(crate) fn check_neutral(&mut self) -> Result<bool, CaptureError> {
        let bits = self.source()?.pressed_keys().map_err(CaptureError::Query)?;
        if bits.count() != usize::from(KEY_MAX) + 1
            || bits.codes().any(|code| {
                !self
                    .source
                    .as_ref()
                    .is_some_and(|source| source.probe().capabilities.keys.contains(code))
            })
        {
            return Err(CaptureError::InvalidState);
        }
        Ok(bits.is_empty())
    }
    pub(crate) fn validate_output(&self, output: &OutputNodeReady<'_>) -> Result<(), CaptureError> {
        if self.phase == CapturePhase::Closed {
            return Err(CaptureError::Closed);
        }
        if self.phase != CapturePhase::Standby {
            return Err(CaptureError::InvalidPhase);
        }
        let node = self.source.as_ref().ok_or(CaptureError::Closed)?.probe();
        if node.input_syspath == output.input_syspath {
            return Err(CaptureError::OwnOutput);
        }
        if !node.input_syspath.is_absolute() || !complete_capabilities(node) {
            return Err(CaptureError::InvalidCapabilities);
        }
        if let Some(issue) = node.capabilities.keyboard_issues().into_iter().next() {
            return Err(CaptureError::Capability(issue));
        }
        let codes: std::collections::BTreeSet<_> = output.spec.codes().collect();
        if let Some(code) = node
            .capabilities
            .keys
            .codes()
            .find(|code| !codes.contains(code))
        {
            return Err(CaptureError::MissingOutputKey(code));
        }
        Ok(())
    }
    /// Runtime must also establish consumer readiness, config output coverage and
    /// progress supervision. This operation queries then grabs then queries again.
    pub fn arm(&mut self, output: &OutputNodeReady<'_>) -> Result<ArmResult, CaptureError> {
        self.validate_output(output)?;
        if !self.check_neutral()? {
            return Ok(ArmResult::WaitingNeutral);
        }
        self.source()?.set_grab(true).map_err(CaptureError::Grab)?;
        self.phase = CapturePhase::Captured; // Guard armed BEFORE the fallible query.
        match self.check_neutral() {
            Ok(true) => {
                self.output_identity = Some(output.input_syspath.into());
                Ok(ArmResult::Captured)
            }
            Ok(false) => self.rollback(Ok(ArmResult::WaitingNeutral)),
            Err(error) => self.rollback(Err(error)),
        }
    }
    fn rollback<T>(&mut self, outcome: Result<T, CaptureError>) -> Result<T, CaptureError> {
        self.output_identity = None;
        match self.source()?.set_grab(false) {
            Ok(()) => {
                self.phase = CapturePhase::Standby;
                outcome
            }
            Err(error) => {
                drop(self.source.take());
                self.phase = CapturePhase::Closed;
                Err(CaptureError::Rollback {
                    primary: outcome.err().map(Box::new),
                    error,
                })
            }
        }
    }
    pub fn release(&mut self) -> Result<(), CaptureError> {
        match self.phase {
            CapturePhase::Closed => Err(CaptureError::Closed),
            CapturePhase::Standby => Ok(()),
            CapturePhase::Captured => self.rollback(Ok(())),
        }
    }
    pub fn close(&mut self) {
        self.output_identity = None;
        drop(self.source.take());
        self.phase = CapturePhase::Closed;
    }
    pub fn read_batch(&mut self) -> Result<ReadBatch, CaptureError> {
        if self.phase != CapturePhase::Captured {
            return Err(CaptureError::InvalidPhase);
        }
        match self.source()?.read_batch() {
            Ok(ReadBatch::Closed) => {
                self.close();
                Ok(ReadBatch::Closed)
            }
            Ok(ReadBatch::Events(events)) if events.len() > crate::events::MAX_NATIVE_BATCH => {
                self.close();
                Err(CaptureError::Read(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "oversized native batch",
                )))
            }
            Ok(batch) => Ok(batch),
            Err(error) => {
                self.close();
                Err(CaptureError::Read(error))
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub struct EvdevSource {
    device: crate::device::EvdevDevice,
    grabbed: bool,
}
#[cfg(target_os = "linux")]
impl CaptureDevice<EvdevSource> {
    pub(crate) fn wait_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        self.source.as_ref().map(|source| source.device.wait_fd())
    }
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self::new(EvdevSource {
            device: crate::device::EvdevDevice::open(path)?,
            grabbed: false,
        }))
    }
}
#[cfg(target_os = "linux")]
impl CaptureSource for EvdevSource {
    fn probe(&self) -> &Probe {
        self.device.probe()
    }
    fn pressed_keys(&mut self) -> io::Result<Bits> {
        self.device.pressed_keys()
    }
    fn set_grab(&mut self, grabbed: bool) -> io::Result<()> {
        self.device.set_grab(grabbed)?;
        self.grabbed = grabbed;
        Ok(())
    }
    fn read_batch(&mut self) -> io::Result<ReadBatch> {
        self.device.read_batch()
    }
}
#[cfg(target_os = "linux")]
impl Drop for EvdevSource {
    fn drop(&mut self) {
        if self.grabbed {
            let _ = self.device.set_grab(false);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        output::RepeatPolicy,
        probe::{Capabilities, DeviceId},
    };
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        Query,
        Grab(bool),
        Read,
        Close,
    }
    struct Source {
        probe: Probe,
        queries: VecDeque<io::Result<Bits>>,
        grabs: VecDeque<io::Result<()>>,
        reads: VecDeque<io::Result<ReadBatch>>,
        calls: Rc<RefCell<Vec<Call>>>,
    }
    impl CaptureSource for Source {
        fn probe(&self) -> &Probe {
            &self.probe
        }
        fn pressed_keys(&mut self) -> io::Result<Bits> {
            self.calls.borrow_mut().push(Call::Query);
            self.queries.pop_front().unwrap()
        }
        fn set_grab(&mut self, grabbed: bool) -> io::Result<()> {
            self.calls.borrow_mut().push(Call::Grab(grabbed));
            self.grabs.pop_front().unwrap()
        }
        fn read_batch(&mut self) -> io::Result<ReadBatch> {
            self.calls.borrow_mut().push(Call::Read);
            self.reads.pop_front().unwrap()
        }
    }
    impl Drop for Source {
        fn drop(&mut self) {
            self.calls.borrow_mut().push(Call::Close);
        }
    }
    fn bits(count: usize, codes: &[u16]) -> Bits {
        let mut bits = Bits::empty(count).unwrap();
        for code in codes {
            bits.set(*code).unwrap();
        }
        bits
    }
    fn neutral() -> io::Result<Bits> {
        Ok(bits(768, &[]))
    }
    fn held() -> io::Result<Bits> {
        Ok(bits(768, &[30]))
    }
    fn fault() -> io::Error {
        io::Error::other("injected fault")
    }
    fn node(path: &str, codes: &[u16]) -> Probe {
        Probe {
            id: DeviceId {
                bus: 6,
                vendor: 0,
                product: 0,
                version: 1,
            },
            name: Some(b"KeyWeave".to_vec()),
            input_syspath: crate::mock_syspath(path),
            capabilities: Capabilities {
                relative: crate::probe::Bits::empty(16).unwrap_or_else(|error| panic!("{error}")),

                events: bits(32, &[0, 1, 20]),
                keys: bits(768, codes),
                misc: bits(8, &[]),
                leds: bits(16, &[]),
                properties: bits(32, &[]),
            },
        }
    }
    fn spec(codes: &[u16]) -> DeviceSpec {
        DeviceSpec::new("KeyWeave", codes, RepeatPolicy::Kernel, false).unwrap()
    }
    fn ready<'a>(spec: &'a DeviceSpec, node: &'a Probe) -> OutputNodeReady<'a> {
        OutputNodeReady::verify(spec, &node.input_syspath, node).unwrap()
    }
    fn source(
        probe: Probe,
        queries: Vec<io::Result<Bits>>,
        grabs: Vec<io::Result<()>>,
        reads: Vec<io::Result<ReadBatch>>,
    ) -> (CaptureDevice<Source>, Rc<RefCell<Vec<Call>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        (
            CaptureDevice::new(Source {
                probe,
                queries: queries.into(),
                grabs: grabs.into(),
                reads: reads.into(),
                calls: calls.clone(),
            }),
            calls,
        )
    }
    const OUT: &str = "/sys/devices/virtual/input/input999";
    const PHYSICAL: &str = "/sys/devices/mock/input/input5";
    #[test]
    fn output_node_proof_checks_identity_and_exact_advertisement() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &output).is_ok());
        let mut wrong = output.clone();
        wrong.id.vendor = 1;
        assert!(matches!(
            OutputNodeReady::verify(&spec, &output.input_syspath, &wrong),
            Err(CaptureError::OutputNodeMismatch)
        ));
        wrong = output.clone();
        wrong.input_syspath = crate::mock_syspath(PHYSICAL);
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &wrong).is_err());
        wrong = output.clone();
        wrong.capabilities.keys.set(48).unwrap();
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &wrong).is_err());
        wrong = output.clone();
        wrong.capabilities.events.set(4).unwrap();
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &wrong).is_err());
        wrong = output.clone();
        wrong.capabilities.properties.set(0).unwrap();
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &wrong).is_err());
    }
    #[test]
    fn neutral_grab_neutral_read_release_has_ordered_lifecycle() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let proof = ready(&spec, &output);
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), neutral()],
            vec![Ok(()), Ok(())],
            vec![Ok(ReadBatch::WouldBlock)],
        );
        assert!(input.read_batch().is_err());
        assert_eq!(input.arm(&proof).unwrap(), ArmResult::Captured);
        assert!(input.arm(&proof).is_err());
        assert_eq!(input.read_batch().unwrap(), ReadBatch::WouldBlock);
        assert_eq!(input.phase(), CapturePhase::Captured);
        input.release().unwrap();
        input.release().unwrap();
        assert_eq!(input.phase(), CapturePhase::Standby);
        input.close();
        input.close();
        assert_eq!(
            *calls.borrow(),
            vec![
                Call::Query,
                Call::Grab(true),
                Call::Query,
                Call::Read,
                Call::Grab(false),
                Call::Close
            ]
        );
    }
    #[test]
    fn held_precheck_does_not_grab_and_can_retry_after_release() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![held(), neutral(), neutral()],
            vec![Ok(())],
            vec![],
        );
        assert_eq!(
            input.arm(&ready(&spec, &output)).unwrap(),
            ArmResult::WaitingNeutral
        );
        assert_eq!(*calls.borrow(), vec![Call::Query]);
        assert_eq!(input.phase(), CapturePhase::Standby);
        assert_eq!(
            input.arm(&ready(&spec, &output)).unwrap(),
            ArmResult::Captured
        );
    }
    #[test]
    fn held_postcheck_rolls_back_without_exposing_captured_reads() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), held()],
            vec![Ok(()), Ok(())],
            vec![],
        );
        assert_eq!(
            input.arm(&ready(&spec, &output)).unwrap(),
            ArmResult::WaitingNeutral
        );
        assert_eq!(input.phase(), CapturePhase::Standby);
        assert!(input.read_batch().is_err());
        assert_eq!(
            *calls.borrow(),
            vec![
                Call::Query,
                Call::Grab(true),
                Call::Query,
                Call::Grab(false)
            ]
        );
    }
    #[test]
    fn query_errors_are_not_neutral_and_post_error_is_preserved() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let (mut input, calls) = source(node(PHYSICAL, &[30]), vec![Err(fault())], vec![], vec![]);
        assert!(matches!(
            input.arm(&ready(&spec, &output)),
            Err(CaptureError::Query(_))
        ));
        assert_eq!(*calls.borrow(), vec![Call::Query]);
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), Err(fault())],
            vec![Ok(()), Ok(())],
            vec![],
        );
        assert!(matches!(
            input.arm(&ready(&spec, &output)),
            Err(CaptureError::Query(_))
        ));
        assert_eq!(input.phase(), CapturePhase::Standby);
        assert_eq!(
            *calls.borrow(),
            vec![
                Call::Query,
                Call::Grab(true),
                Call::Query,
                Call::Grab(false)
            ]
        );
    }
    #[test]
    fn rollback_error_closes_client_and_retains_primary_error() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), Err(fault())],
            vec![Ok(()), Err(fault())],
            vec![],
        );
        assert!(matches!(
            input.arm(&ready(&spec, &output)),
            Err(CaptureError::Rollback {
                primary: Some(_),
                ..
            })
        ));
        assert_eq!(input.phase(), CapturePhase::Closed);
        assert!(input.probe().is_none());
        assert!(input.arm(&ready(&spec, &output)).is_err());
        assert_eq!(
            *calls.borrow(),
            vec![
                Call::Query,
                Call::Grab(true),
                Call::Query,
                Call::Grab(false),
                Call::Close
            ]
        );
    }
    #[test]
    fn grab_failure_has_no_post_query_or_owned_grab() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral()],
            vec![Err(io::Error::from(io::ErrorKind::PermissionDenied))],
            vec![],
        );
        assert!(matches!(
            input.arm(&ready(&spec, &output)),
            Err(CaptureError::Grab(_))
        ));
        assert_eq!(input.phase(), CapturePhase::Standby);
        assert_eq!(*calls.borrow(), vec![Call::Query, Call::Grab(true)]);
    }
    #[test]
    fn own_identity_is_rejected_before_query_even_with_a_keyboard_name() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let (mut input, calls) = source(output.clone(), vec![], vec![], vec![]);
        assert!(matches!(
            input.arm(&ready(&spec, &output)),
            Err(CaptureError::OwnOutput)
        ));
        assert!(calls.borrow().is_empty());
        let (mut input, _) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), neutral()],
            vec![Ok(())],
            vec![],
        );
        // Identical names are permitted for distinct canonical physical paths.
        assert_eq!(
            input.arm(&ready(&spec, &output)).unwrap(),
            ArmResult::Captured
        );
    }
    #[test]
    fn unsupported_interfaces_and_incomplete_masks_fail_before_query() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let mut relative = node("relative", &[30]);
        relative.capabilities.keys = bits(768, &[30]);
        let mut mixed = node(PHYSICAL, &[30]);
        mixed.capabilities.events.set(2).unwrap();
        let mut truncated = node(PHYSICAL, &[30]);
        truncated.capabilities.keys = bits(64, &[30]);
        for probe in [
            relative,
            mixed,
            truncated,
            node(PHYSICAL, &[30, 48]),
            node(PHYSICAL, &[767]),
        ] {
            let (mut input, calls) = source(probe, vec![], vec![], vec![]);
            assert!(input.arm(&ready(&spec, &output)).is_err());
            assert!(calls.borrow().is_empty());
        }
    }
    #[test]
    fn truncated_or_unadvertised_state_is_not_neutral() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        for state in [bits(64, &[]), bits(768, &[48])] {
            let (mut input, calls) = source(node(PHYSICAL, &[30]), vec![Ok(state)], vec![], vec![]);
            assert!(matches!(
                input.arm(&ready(&spec, &output)),
                Err(CaptureError::InvalidState)
            ));
            assert_eq!(*calls.borrow(), vec![Call::Query]);
        }
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), Ok(bits(64, &[]))],
            vec![Ok(()), Ok(())],
            vec![],
        );
        assert!(matches!(
            input.arm(&ready(&spec, &output)),
            Err(CaptureError::InvalidState)
        ));
        assert_eq!(calls.borrow().last(), Some(&Call::Grab(false)));
    }
    #[test]
    fn read_fault_eof_and_oversized_batch_close_without_waiting_for_output() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        for read in [
            Err(fault()),
            Ok(ReadBatch::Closed),
            Ok(ReadBatch::Events(vec![
                crate::events::Event::output(0, 0, 0);
                1025
            ])),
        ] {
            let (mut input, calls) = source(
                node(PHYSICAL, &[30]),
                vec![neutral(), neutral()],
                vec![Ok(())],
                vec![read],
            );
            input.arm(&ready(&spec, &output)).unwrap();
            let _ = input.read_batch();
            assert_eq!(input.phase(), CapturePhase::Closed);
            assert_eq!(calls.borrow().last(), Some(&Call::Close));
        }
    }
    #[test]
    fn explicit_release_failure_closes_and_drop_also_closes_owned_source() {
        let spec = spec(&[30]);
        let output = node(OUT, &[30]);
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), neutral()],
            vec![Ok(()), Err(fault())],
            vec![],
        );
        input.arm(&ready(&spec, &output)).unwrap();
        assert!(input.release().is_err());
        assert_eq!(input.phase(), CapturePhase::Closed);
        assert_eq!(calls.borrow().last(), Some(&Call::Close));
        let (mut input, calls) = source(
            node(PHYSICAL, &[30]),
            vec![neutral(), neutral()],
            vec![Ok(())],
            vec![],
        );
        input.arm(&ready(&spec, &output)).unwrap();
        drop(input);
        assert_eq!(calls.borrow().last(), Some(&Call::Close));
    }

    #[test]
    fn relative_output_node_requires_exact_axes_and_physical_keyboard_stays_strict() {
        let spec = spec(&[30])
            .with_relative_axes(crate::relative::RelativeAxes::new(&[0, 1, 11]).unwrap());
        let mut output = node(OUT, &[30]);
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &output).is_err());
        output.capabilities.events.set(2).unwrap();
        for axis in [0, 1, 11] {
            output.capabilities.relative.set(axis).unwrap();
        }
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &output).is_ok());
        assert!(!output.capabilities.keyboard_issues().is_empty());
        output.capabilities.relative.set(8).unwrap();
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &output).is_err());
    }
    #[test]
    fn undeclared_relative_mask_cannot_be_hidden_from_output_or_keyboard_admission() {
        let spec = spec(&[30]);
        let mut output = node(OUT, &[30]);
        output.capabilities.relative.set(0).unwrap();
        assert!(OutputNodeReady::verify(&spec, &output.input_syspath, &output).is_err());
        assert!(!output.capabilities.keyboard_issues().is_empty());
    }
}
