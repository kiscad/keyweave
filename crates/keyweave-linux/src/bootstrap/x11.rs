//! Explicit native X11 startup; independent supervision and Stop handling belong to caller.
use super::*;
use crate::{
    device::EvdevDevice,
    x11_consumer::{NativeX11Lease, NativeX11Observer, X11Observation},
    x11_probe::ProbePhase,
};
struct NativeX11<'a> {
    guard: &'a WorkerParentGuard,
    uinput: PathBuf,
    observer: NativeX11Observer,
    node: Option<EvdevDevice>,
    clock: MonotonicClock,
    cancel_error: Option<io::Error>,
}
impl Platform for NativeX11<'_> {
    fn next_query_deadline(&self) -> Option<MonoTime> {
        self.observer.next_deadline()
    }
    fn poll_interval(&self) -> u64 {
        if self.observer.job_phase().is_some() {
            2_000_000
        } else {
            crate::reactor::HOUSEKEEPING_NS
        }
    }
    type Ready = X11Observation;
    type Source = EvdevSource;
    type Output = UinputDevice;
    fn retain_on_close(&self) -> bool {
        true
    }
    fn abort(&mut self) {
        if let Err(error) = self.observer.cancel()
            && self.cancel_error.is_none()
        {
            self.cancel_error = Some(error);
        }
    }
    fn query_pending(&self) -> bool {
        self.observer.job_phase().is_some() && !self.observer.job_failed()
    }
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
        if self.node.is_none() {
            self.node = output.discover_output_node()?;
        }
        Ok(self.node.as_ref().map(|node| node.probe().clone()))
    }
    fn open(&mut self, path: &Path) -> io::Result<CaptureDevice<EvdevSource>> {
        CaptureDevice::open_with_relative(path)
    }
    fn observe(&mut self, output: &UinputDevice) -> io::Result<Option<X11Observation>> {
        let node = self
            .node
            .as_ref()
            .ok_or_else(|| io::Error::other("X11 output node missing"))?;
        if self.observer.job_phase().is_none()
            && !self
                .observer
                .begin(output, node, &mut || self.clock.now())?
        {
            return Ok(None);
        }
        self.observer.poll(output, node, &mut || self.clock.now())?;
        if self.observer.job_failed() {
            return Err(io::Error::other("X11 metadata job failed"));
        }
        Ok(self.observer.take_observation())
    }
}
pub struct X11Bootstrap<'a> {
    inner: Bootstrap<NativeX11<'a>>,
}
pub struct X11Prepared {
    pub runtime: Runtime<EvdevSource, UinputDevice>,
    pub consumer: NativeX11Lease,
    pub clock: MonotonicClock,
}
pub enum X11StartupOutcome {
    Ready(Box<X11Prepared>),
    Stopped(Option<Box<X11StartupCleanup>>),
}
pub struct X11BootstrapFailure {
    pub error: BootstrapError,
    pub cleanup: Option<X11StartupCleanup>,
}
pub struct X11StartupCleanup {
    observer: NativeX11Observer,
    node: Option<EvdevDevice>,
    cancel_error: Option<io::Error>,
    observed_exit: bool,
}
pub struct X11StartupCleanupReport {
    pub cancel_error: Option<io::Error>,
    pub observed_exit: bool,
    pub owner_dropped: bool,
}
impl X11StartupCleanup {
    fn from_native(mut native: NativeX11<'_>) -> Self {
        native.abort();
        Self {
            observer: native.observer,
            node: native.node,
            cancel_error: native.cancel_error,
            observed_exit: false,
        }
    }
    pub fn step(&mut self) -> io::Result<Option<ProbePhase>> {
        let phase = self.observer.poll_canceled()?;
        if phase == Some(ProbePhase::Exited) {
            self.observed_exit = true;
        }
        Ok(phase)
    }
    pub fn pending(&self) -> bool {
        self.observer.job_phase().is_some()
    }
    /// Pending result retains the original owner, including after observation timeout.
    pub fn finish(self) -> Result<X11StartupCleanupReport, Box<Self>> {
        if self.pending() {
            return Err(Box::new(self));
        }
        let Self {
            observer,
            node,
            cancel_error,
            observed_exit,
        } = self;
        drop(node);
        drop(observer);
        Ok(X11StartupCleanupReport {
            cancel_error,
            observed_exit,
            owner_dropped: true,
        })
    }
}
impl<'a> X11Bootstrap<'a> {
    /// Caller authenticates helper and owns independent progress supervision before stepping.
    pub fn new(
        guard: &'a WorkerParentGuard,
        uinput: &Path,
        helper: &Path,
        inputs: Vec<(DeviceInstanceId, PathBuf)>,
        config: Arc<ConfigSnapshot>,
        spec: DeviceSpec,
    ) -> Result<Self, BootstrapError> {
        let plan = Plan::new_inner(
            inputs,
            config,
            spec,
            crate::runtime::ExecutionMode::StagedKeyboard,
            true,
        )?;
        guard.verify_isolated_group()?;
        let observer = NativeX11Observer::new(guard, helper)?;
        Ok(Self {
            inner: Bootstrap::new(
                plan,
                NativeX11 {
                    guard,
                    uinput: uinput.into(),
                    observer,
                    node: None,
                    clock: MonotonicClock::new(),
                    cancel_error: None,
                },
            ),
        })
    }
    /// Handle Stop before every step. On error consume into_cleanup; do not restart this boot.
    pub fn step(&mut self) -> Result<BootstrapStep, BootstrapError> {
        self.inner.step()
    }
    pub fn next_wakeup(&self) -> Option<MonoTime> {
        self.inner.next_wakeup()
    }
    /// Stop before every step. No startup progress is published. Cleanup owners are returned.
    pub fn run<R, W>(
        mut self,
        client: &mut crate::progress::ProgressClient<R, W>,
    ) -> Result<X11StartupOutcome, Box<X11BootstrapFailure>>
    where
        R: io::Read + std::os::fd::AsFd,
        W: io::Write + std::os::fd::AsFd,
    {
        let result = (|| {
            for request in client.wait_requests_with_control(true) {
                crate::ioctl::nonblocking(request.fd)?;
            }
            drive_until_ready(&mut self.inner, client, &mut |boot, client| {
                boot.time()?;
                let deadline = boot.next_wakeup();
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
            })
        })();
        match result {
            Ok(true) => self
                .into_prepared()
                .map(|ready| X11StartupOutcome::Ready(Box::new(ready))),
            Ok(false) => Ok(X11StartupOutcome::Stopped(
                self.into_cleanup().map(Box::new),
            )),
            Err(error) => Err(Box::new(X11BootstrapFailure {
                error,
                cleanup: self.into_cleanup(),
            })),
        }
    }
    pub fn into_cleanup(mut self) -> Option<X11StartupCleanup> {
        self.inner.close();
        self.inner
            .platform
            .take()
            .map(X11StartupCleanup::from_native)
    }
    pub fn into_prepared(self) -> Result<X11Prepared, Box<X11BootstrapFailure>> {
        let handoff = match self.inner.finish_preserving() {
            Ok(handoff) => handoff,
            Err((error, mut held)) => {
                return Err(Box::new(X11BootstrapFailure {
                    error,
                    cleanup: held.platform.take().map(X11StartupCleanup::from_native),
                }));
            }
        };
        let Handoff {
            runtime,
            mut platform,
            ready,
        } = handoff;
        let Some(node) = platform.node.take() else {
            drop(runtime);
            return Err(Box::new(X11BootstrapFailure {
                error: BootstrapError::Phase,
                cleanup: Some(X11StartupCleanup::from_native(platform)),
            }));
        };
        if let Err(error) = platform.observer.restore_completed(ready) {
            platform.node = Some(node);
            drop(runtime);
            return Err(Box::new(X11BootstrapFailure {
                error: BootstrapError::Io(error),
                cleanup: Some(X11StartupCleanup::from_native(platform)),
            }));
        }
        Ok(X11Prepared {
            runtime,
            consumer: NativeX11Lease::new(platform.observer, node),
            clock: platform.clock,
        })
    }
}
