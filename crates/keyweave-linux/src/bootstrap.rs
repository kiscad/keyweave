//! Finite consumer capture preparation. Caller owns independent supervision;
//! active consumer renewal is required after the final Runtime handoff.
mod x11;
pub use x11::{
    X11Bootstrap, X11BootstrapFailure, X11Prepared, X11StartupCleanup, X11StartupCleanupReport,
    X11StartupOutcome,
};

use crate::{
    activation::{ActivationError, CaptureGroup},
    capture::{
        ArmResult, CaptureDevice, CaptureError, CaptureSource, EvdevSource, OutputNodeReady,
    },
    console::{ConsoleProvider, ConsoleReady},
    output::{DeviceSpec, RepeatPolicy},
    probe::Probe,
    pump::FrameBackend,
    reactor::MonotonicClock,
    runtime::{MAX_DEVICES, Runtime, RuntimeError, validate_config},
    uinput::UinputDevice,
    worker::WorkerParentGuard,
};
use keyweave_engine::{ConfigSnapshot, EngineError};
use keyweave_types::{DeviceInstanceId, MonoTime};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const BOOTSTRAP_TIMEOUT_NS: u64 = 10_000_000_000;
pub const MAX_BOOTSTRAP_STEPS: usize = 512;
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error(transparent)]
    Progress(#[from] crate::progress::ClientError),
    #[error("bootstrap progress endpoint failed")]
    Endpoint,
    #[error("invalid bootstrap input identity or paths")]
    Inputs,
    #[error("bootstrap deadline expired or overflowed")]
    Deadline,
    #[error("bootstrap clock moved backwards")]
    Clock,
    #[error("bootstrap attempt budget exhausted")]
    Attempts,
    #[error("invalid bootstrap phase")]
    Phase,
    #[error("consumer readiness lost after capture admission")]
    ConsumerLost,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Activation(#[from] ActivationError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootstrapStep {
    Pending,
    Ready,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Output,
    Discovery,
    Inputs,
    Group,
    Arm,
    Verify,
    Ready,
    Closed,
}

struct Plan {
    inputs: Vec<(DeviceInstanceId, PathBuf)>,
    config: Arc<ConfigSnapshot>,
    spec: DeviceSpec,
    mode: crate::runtime::ExecutionMode,
    relative: bool,
}
impl Plan {
    fn new(
        inputs: Vec<(DeviceInstanceId, PathBuf)>,
        config: Arc<ConfigSnapshot>,
        spec: DeviceSpec,
    ) -> Result<Self, BootstrapError> {
        Self::new_with_mode(inputs, config, spec, crate::runtime::ExecutionMode::Legacy)
    }
    fn new_with_mode(
        inputs: Vec<(DeviceInstanceId, PathBuf)>,
        config: Arc<ConfigSnapshot>,
        spec: DeviceSpec,
        mode: crate::runtime::ExecutionMode,
    ) -> Result<Self, BootstrapError> {
        Self::new_inner(inputs, config, spec, mode, false)
    }
    fn new_inner(
        inputs: Vec<(DeviceInstanceId, PathBuf)>,
        config: Arc<ConfigSnapshot>,
        spec: DeviceSpec,
        mode: crate::runtime::ExecutionMode,
        relative: bool,
    ) -> Result<Self, BootstrapError> {
        if relative
            && (mode != crate::runtime::ExecutionMode::StagedKeyboard
                || spec.relative_axes().is_none())
        {
            return Err(RuntimeError::StagedPointer.into());
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        if inputs.is_empty()
            || inputs.len() > MAX_DEVICES
            || inputs.iter().any(|(id, path)| {
                id.id == 0
                    || id.generation == 0
                    || !ids.insert(id.id)
                    || !path.is_absolute()
                    || !paths.insert(path.clone())
            })
        {
            return Err(BootstrapError::Inputs);
        }
        if spec.repeat() != RepeatPolicy::Kernel {
            return Err(RuntimeError::RepeatPolicy.into());
        }
        validate_config(&config, &spec)?;
        match mode.validate_snapshot(config.clone()) {
            Err(RuntimeError::Engine(error)) => return Err(BootstrapError::Engine(error)),
            Err(error) => return Err(BootstrapError::Runtime(error)),
            Ok(()) => {}
        }
        Ok(Self {
            inputs,
            config,
            spec,
            mode,
            relative,
        })
    }
}
trait ReadyEvidence {
    fn fresh_for(&self, path: &Path, now: MonoTime) -> bool;
    fn valid_until(&self) -> MonoTime;
    fn arm<S: CaptureSource>(
        &self,
        group: &mut CaptureGroup<S>,
        output: &OutputNodeReady<'_>,
        now: MonoTime,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<ArmResult, ActivationError>;
}
impl ReadyEvidence for ConsoleReady {
    fn valid_until(&self) -> MonoTime {
        self.valid_until()
    }
    fn fresh_for(&self, path: &Path, now: MonoTime) -> bool {
        self.fresh_for(path, now)
    }
    fn arm<S: CaptureSource>(
        &self,
        group: &mut CaptureGroup<S>,
        output: &OutputNodeReady<'_>,
        now: MonoTime,
        _: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<ArmResult, ActivationError> {
        group.try_arm_console(output, self, now)
    }
}
impl ReadyEvidence for crate::x11_consumer::X11Observation {
    fn valid_until(&self) -> MonoTime {
        self.valid_until()
    }
    fn fresh_for(&self, path: &Path, now: MonoTime) -> bool {
        self.fresh_for(path, now)
    }
    fn arm<S: CaptureSource>(
        &self,
        group: &mut CaptureGroup<S>,
        output: &OutputNodeReady<'_>,
        _: MonoTime,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<ArmResult, ActivationError> {
        group.try_arm_x11(output, self, clock)
    }
}
// Private injection boundary; no public factory can replace real guard/IO checks.
trait Platform {
    type Ready: ReadyEvidence;
    fn next_query_deadline(&self) -> Option<MonoTime> {
        None
    }
    fn poll_interval(&self) -> u64 {
        crate::reactor::HOUSEKEEPING_NS
    }
    fn retain_on_close(&self) -> bool {
        false
    }
    fn abort(&mut self) {}
    fn query_pending(&self) -> bool {
        false
    }
    type Source: CaptureSource;
    type Output: FrameBackend;
    fn verify(&mut self) -> io::Result<()>;
    fn now(&mut self) -> io::Result<MonoTime>;
    fn create(&mut self, spec: DeviceSpec) -> io::Result<Self::Output>;
    fn discover(&mut self, output: &Self::Output) -> io::Result<Option<Probe>>;
    fn open(&mut self, path: &Path) -> io::Result<CaptureDevice<Self::Source>>;
    fn observe(&mut self, output: &Self::Output) -> io::Result<Option<Self::Ready>>;
}
struct Bootstrap<P: Platform> {
    platform: Option<P>,
    plan: Plan,
    inputs: Vec<(DeviceInstanceId, CaptureDevice<P::Source>)>,
    group: Option<CaptureGroup<P::Source>>,
    output: Option<P::Output>,
    node: Option<Probe>,
    ready: Option<P::Ready>,
    phase: Phase,
    deadline: Option<MonoTime>,
    last: Option<MonoTime>,
    attempts: usize,
}
struct Handoff<P: Platform> {
    runtime: Runtime<P::Source, P::Output>,
    platform: P,
    ready: P::Ready,
}
impl<P: Platform> Bootstrap<P> {
    fn new(plan: Plan, platform: P) -> Self {
        Self {
            platform: Some(platform),
            plan,
            inputs: vec![],
            group: None,
            output: None,
            node: None,
            ready: None,
            phase: Phase::Output,
            deadline: None,
            last: None,
            attempts: 0,
        }
    }
    fn time(&mut self) -> Result<MonoTime, BootstrapError> {
        let now = self.platform.as_mut().ok_or(BootstrapError::Phase)?.now()?;
        if self.last.is_some_and(|old| now < old) {
            return Err(BootstrapError::Clock);
        }
        self.last = Some(now);
        let deadline = match self.deadline {
            Some(deadline) => deadline,
            None => {
                let deadline = now
                    .checked_add(BOOTSTRAP_TIMEOUT_NS)
                    .ok_or(BootstrapError::Deadline)?;
                self.deadline = Some(deadline);
                deadline
            }
        };
        if now >= deadline {
            return Err(BootstrapError::Deadline);
        }
        Ok(now)
    }
    fn next_wakeup(&self) -> Option<MonoTime> {
        if matches!(self.phase, Phase::Closed | Phase::Ready) {
            return None;
        }
        let platform = self.platform.as_ref()?;
        let mut next = self.last?.checked_add(platform.poll_interval())?;
        if let Some(limit) = self.deadline {
            next = next.min(limit);
        }
        if let Some(limit) = platform.next_query_deadline() {
            next = next.min(limit);
        }
        if self.phase == Phase::Verify
            && let Some(ready) = &self.ready
        {
            next = next.min(ready.valid_until());
        }
        Some(next)
    }
    fn close(&mut self) {
        if self.phase == Phase::Closed {
            return;
        }
        // This explicit order applies to partially opened and already grabbed
        // resources, including Drop and post-arm provider failure.
        self.group.take();
        self.inputs.clear();
        self.node.take();
        self.ready.take();
        self.output.take();
        if self
            .platform
            .as_ref()
            .is_some_and(Platform::retain_on_close)
        {
            if let Some(platform) = &mut self.platform {
                platform.abort();
            }
        } else {
            self.platform.take();
        } // synchronous provider closes last
        self.phase = Phase::Closed;
    }
    fn step(&mut self) -> Result<BootstrapStep, BootstrapError> {
        if matches!(self.phase, Phase::Closed | Phase::Ready) {
            self.close();
            return Err(BootstrapError::Phase);
        }
        let result = (|| {
            if self.attempts >= MAX_BOOTSTRAP_STEPS {
                return Err(BootstrapError::Attempts);
            }
            self.attempts += 1;
            self.platform
                .as_mut()
                .ok_or(BootstrapError::Phase)?
                .verify()?;
            self.time()?;
            self.advance()?;
            let now = self.time()?; // slow IO cannot extend deadline or grant a late Ready
            if self.phase == Phase::Ready && !self.ready_fresh(now) {
                return Err(BootstrapError::ConsumerLost);
            }
            Ok(if self.phase == Phase::Ready {
                BootstrapStep::Ready
            } else {
                BootstrapStep::Pending
            })
        })();
        if result.is_err() {
            self.close();
        }
        result
    }
    fn advance(&mut self) -> Result<(), BootstrapError> {
        if matches!(self.phase, Phase::Arm | Phase::Verify) {
            return self.admit();
        }
        let platform = self.platform.as_mut().ok_or(BootstrapError::Phase)?;
        match self.phase {
            Phase::Output => {
                self.output = Some(platform.create(self.plan.spec.clone())?);
                self.phase = Phase::Discovery;
            }
            Phase::Discovery => {
                let output = self.output.as_ref().ok_or(BootstrapError::Phase)?;
                if let Some(node) = platform.discover(output)? {
                    let path = output.input_syspath().ok_or(BootstrapError::Inputs)?;
                    OutputNodeReady::verify(output.spec(), path, &node)?;
                    self.node = Some(node);
                    self.phase = Phase::Inputs;
                }
            }
            Phase::Inputs => {
                let (id, path) = self
                    .plan
                    .inputs
                    .get(self.inputs.len())
                    .ok_or(BootstrapError::Phase)?;
                self.inputs.push((*id, platform.open(path)?));
                if self.inputs.len() == self.plan.inputs.len() {
                    self.phase = Phase::Group;
                }
            }
            Phase::Group => {
                let inputs = std::mem::take(&mut self.inputs);
                self.group = Some(if self.plan.relative {
                    CaptureGroup::new_staged_relative(inputs, self.plan.config.clone())?
                } else {
                    CaptureGroup::new_with_mode(inputs, self.plan.config.clone(), self.plan.mode)?
                });
                self.phase = Phase::Arm;
            }
            Phase::Arm | Phase::Verify | Phase::Ready | Phase::Closed => {
                return Err(BootstrapError::Phase);
            }
        }
        Ok(())
    }
    fn ready_fresh(&self, now: MonoTime) -> bool {
        self.output
            .as_ref()
            .and_then(FrameBackend::input_syspath)
            .is_some_and(|path| {
                self.ready
                    .as_ref()
                    .is_some_and(|ready| ready.fresh_for(path, now))
            })
    }
    fn admit(&mut self) -> Result<(), BootstrapError> {
        if self.phase == Phase::Verify {
            let now = self.time()?;
            if !self.ready_fresh(now) {
                return Err(BootstrapError::ConsumerLost);
            }
        }
        let output = self.output.as_ref().ok_or(BootstrapError::Phase)?;
        let ready = self
            .platform
            .as_mut()
            .ok_or(BootstrapError::Phase)?
            .observe(output)?;
        let now = self.time()?;
        if self.phase == Phase::Verify && !self.ready_fresh(now) {
            return Err(BootstrapError::ConsumerLost);
        }
        let output = self.output.as_ref().ok_or(BootstrapError::Phase)?;
        let path = output.input_syspath().ok_or(BootstrapError::Inputs)?;
        let Some(ready) = ready.filter(|ready| ready.fresh_for(path, now)) else {
            if self.phase == Phase::Verify
                && !self.platform.as_ref().is_some_and(Platform::query_pending)
            {
                return Err(BootstrapError::ConsumerLost);
            }
            return Ok(());
        };
        if self.phase == Phase::Verify {
            self.ready = Some(ready);
            self.phase = Phase::Ready;
        } else {
            let node = self.node.as_ref().ok_or(BootstrapError::Phase)?;
            let proof = OutputNodeReady::verify(output.spec(), path, node)?;
            let group = self.group.as_mut().ok_or(BootstrapError::Phase)?;
            let platform = self.platform.as_mut().ok_or(BootstrapError::Phase)?;
            if ready.arm(group, &proof, now, &mut || platform.now())? == ArmResult::Captured {
                self.ready = Some(ready);
                self.phase = Phase::Verify;
            }
        }
        Ok(())
    }
    fn finish(self) -> Result<Handoff<P>, BootstrapError> {
        match self.finish_preserving() {
            Ok(handoff) => Ok(handoff),
            Err((error, held)) => {
                drop(held);
                Err(error)
            }
        }
    }
    fn finish_preserving(mut self) -> Result<Handoff<P>, (BootstrapError, Box<Self>)> {
        let result = (|| {
            if self.phase != Phase::Ready {
                return Err(BootstrapError::Phase);
            }
            self.platform
                .as_mut()
                .ok_or(BootstrapError::Phase)?
                .verify()?;
            let now = self.time()?;
            if !self.ready_fresh(now) {
                return Err(BootstrapError::ConsumerLost);
            }
            let group = self.group.take().ok_or(BootstrapError::Phase)?;
            let output = self.output.take().ok_or(BootstrapError::Phase)?;
            let runtime = group.into_runtime(output)?;
            Ok(Handoff {
                runtime,
                platform: self.platform.take().ok_or(BootstrapError::Phase)?,
                ready: self.ready.take().ok_or(BootstrapError::Phase)?,
            })
        })();
        match result {
            Ok(handoff) => Ok(handoff),
            Err(error) => {
                self.close();
                Err((error, Box::new(self)))
            }
        }
    }
}
impl<P: Platform> Drop for Bootstrap<P> {
    fn drop(&mut self) {
        self.close();
    }
}
fn drive_controlled<P, R, W, F>(
    mut boot: Bootstrap<P>,
    client: &mut crate::progress::ProgressClient<R, W>,
    mut wait: F,
) -> Result<Option<Handoff<P>>, BootstrapError>
where
    P: Platform,
    R: io::Read,
    W: io::Write,
    F: FnMut(
        &mut Bootstrap<P>,
        &crate::progress::ProgressClient<R, W>,
    ) -> Result<(), BootstrapError>,
{
    if drive_until_ready(&mut boot, client, &mut wait)? {
        boot.finish().map(Some)
    } else {
        Ok(None)
    }
}
fn drive_until_ready<P, R, W, F>(
    boot: &mut Bootstrap<P>,
    client: &mut crate::progress::ProgressClient<R, W>,
    wait: &mut F,
) -> Result<bool, BootstrapError>
where
    P: Platform,
    R: io::Read,
    W: io::Write,
    F: FnMut(
        &mut Bootstrap<P>,
        &crate::progress::ProgressClient<R, W>,
    ) -> Result<(), BootstrapError>,
{
    let result = (|| {
        loop {
            if client.receive_startup_command()? == crate::progress::RequestEvent::Stop {
                boot.close();
                return Ok(false);
            }
            if boot.step()? == BootstrapStep::Ready {
                return Ok(true);
            }
            wait(boot, client)?;
        }
    })();
    if result.is_err() {
        boot.close();
    }
    result
}

struct Native<'a> {
    guard: &'a WorkerParentGuard,
    uinput: PathBuf,
    provider: ConsoleProvider,
    clock: MonotonicClock,
}
impl Platform for Native<'_> {
    type Ready = ConsoleReady;
    type Source = EvdevSource;
    type Output = UinputDevice;
    fn verify(&mut self) -> io::Result<()> {
        self.guard.verify_isolated_group()
    }
    fn now(&mut self) -> io::Result<MonoTime> {
        self.clock.now()
    }
    fn create(&mut self, spec: DeviceSpec) -> io::Result<UinputDevice> {
        UinputDevice::create(&self.uinput, spec)
    }
    fn discover(&mut self, output: &UinputDevice) -> io::Result<Option<Probe>> {
        output
            .discover_output_node()
            .map(|node| node.map(|node| node.probe().clone()))
    }
    fn open(&mut self, path: &Path) -> io::Result<CaptureDevice<EvdevSource>> {
        CaptureDevice::open(path)
    }
    fn observe(&mut self, output: &UinputDevice) -> io::Result<Option<ConsoleReady>> {
        self.provider.observe(output, &mut self.clock)
    }
}
/// Construct only after claiming reporting/progress endpoints and establishing
/// independent parent supervision. Each step is finite in operations, not a
/// promise that native syscalls return within the timeout. No sleeps/threads.
pub struct ConsoleBootstrap<'a> {
    inner: Bootstrap<Native<'a>>,
}
impl<'a> ConsoleBootstrap<'a> {
    pub fn new(
        guard: &'a WorkerParentGuard,
        vt: &Path,
        uinput: &Path,
        inputs: Vec<(DeviceInstanceId, PathBuf)>,
        config: Arc<ConfigSnapshot>,
        spec: DeviceSpec,
    ) -> Result<Self, BootstrapError> {
        Self::new_with_mode(
            guard,
            vt,
            uinput,
            inputs,
            config,
            spec,
            crate::runtime::ExecutionMode::Legacy,
        )
    }
    pub fn new_with_mode(
        guard: &'a WorkerParentGuard,
        vt: &Path,
        uinput: &Path,
        inputs: Vec<(DeviceInstanceId, PathBuf)>,
        config: Arc<ConfigSnapshot>,
        spec: DeviceSpec,
        mode: crate::runtime::ExecutionMode,
    ) -> Result<Self, BootstrapError> {
        let plan = match mode {
            crate::runtime::ExecutionMode::Legacy => Plan::new(inputs, config, spec)?,
            crate::runtime::ExecutionMode::StagedKeyboard => {
                Plan::new_with_mode(inputs, config, spec, mode)?
            }
        }; // validate before opening devices
        if !uinput.is_absolute() {
            return Err(BootstrapError::Inputs);
        }
        guard.verify_isolated_group()?;
        let provider = ConsoleProvider::open(vt, guard)?;
        Ok(Self {
            inner: Bootstrap::new(
                plan,
                Native {
                    guard,
                    uinput: uinput.into(),
                    provider,
                    clock: MonotonicClock::new(),
                },
            ),
        })
    }
    /// Call only until Ready; then consume into_parts promptly. A repeated
    /// step after Ready is an error and closes capture rather than keeping it.
    pub fn step(&mut self) -> Result<BootstrapStep, BootstrapError> {
        self.inner.step()
    }
    /// Stop before every step, no standby publications. None means startup
    /// cancelled and all boot owners dropped, not a Runtime recovery report.
    pub fn run<R, W>(
        self,
        client: &mut crate::progress::ProgressClient<R, W>,
    ) -> Result<Option<ConsolePrepared>, BootstrapError>
    where
        R: io::Read + std::os::fd::AsFd,
        W: io::Write + std::os::fd::AsFd,
    {
        for request in client.wait_requests_with_control(true) {
            crate::ioctl::nonblocking(request.fd)?;
        }
        let handoff = drive_controlled(self.inner, client, |boot, client| {
            boot.time()?;
            let deadline = boot.deadline;
            let requests = client.wait_requests_with_control(true);
            let native = boot.platform.as_mut().ok_or(BootstrapError::Phase)?;
            let wake = crate::reactor::wait(
                &mut native.clock,
                &requests,
                deadline,
                crate::reactor::HOUSEKEEPING_NS,
            )?;
            if wake.any_fault() {
                return Err(BootstrapError::Endpoint);
            }
            boot.time()?;
            Ok(())
        })?;
        Ok(handoff.map(
            |Handoff {
                 runtime,
                 platform: native,
                 ready,
             }| ConsolePrepared {
                runtime,
                provider: native.provider,
                clock: native.clock,
                ready,
            },
        ))
    }
    /// Consumes the only capture/output owners. Check/renew the returned ready
    /// observation before any Runtime dispatch and throughout active execution.
    /// The same clock must drive Runtime, provider and worker protocol.
    pub fn into_parts(
        self,
    ) -> Result<
        (
            Runtime<EvdevSource, UinputDevice>,
            ConsoleProvider,
            MonotonicClock,
            ConsoleReady,
        ),
        BootstrapError,
    > {
        let Handoff {
            runtime,
            platform: native,
            ready,
        } = self.inner.finish()?;
        Ok((runtime, native.provider, native.clock, ready))
    }
}
/// No automatic active dispatch; caller transfers the same owners/client to the
/// console worker loop. Field order keeps Runtime before the read-only provider.
pub struct ConsolePrepared {
    pub runtime: Runtime<EvdevSource, UinputDevice>,
    pub provider: ConsoleProvider,
    pub clock: MonotonicClock,
    pub ready: ConsoleReady,
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
