//! Native control extension boundary; no parsing or input APIs here.
use super::*;
use crate::{capture::EvdevSource, uinput::UinputDevice};
use keyweave_progress::RequestEvent;
const PARENT_ACTIVITY_NS: u64 = 2_000_000_000;
const EXTENSION_WAKE_NS: u64 = 5_000_000;
pub struct ControlObservation {
    pub at: MonoTime,
    pub parent_until: MonoTime,
    pub session_until: MonoTime,
    pub stop: bool,
}
/// One nonblocking control operation. Its observation callback must bracket IO
/// and activation. It supplies no epoch/receipt and cannot renew these leases.
pub trait WorkerExtension<S: CaptureSource, B: FrameBackend> {
    fn tick(
        &mut self,
        runtime: &mut Runtime<S, B>,
        observe: &mut dyn FnMut() -> io::Result<ControlObservation>,
    ) -> io::Result<()>;
    /// Must close owned protocol copies/candidates without waiting for cold work.
    /// Caller retains this owner after return to reap its original cold job.
    fn close(&mut self);
}
struct ParentActivity {
    until: MonoTime,
    id: u64,
    last: MonoTime,
}
impl ParentActivity {
    fn new(at: MonoTime, id: u64) -> io::Result<Self> {
        Ok(Self {
            until: at
                .checked_add(PARENT_ACTIVITY_NS)
                .ok_or_else(|| io::Error::other("parent activity overflow"))?,
            id,
            last: at,
        })
    }
    fn observe(&mut self, at: MonoTime, id: u64) -> io::Result<MonoTime> {
        self.observe_at(at, id, at)
    }
    fn observe_at(&mut self, at: MonoTime, id: u64, anchor: MonoTime) -> io::Result<MonoTime> {
        if at < self.last || at >= self.until || id < self.id || anchor > at || anchor < self.last {
            return Err(io::Error::other("parent activity expired or invalid"));
        }
        if id != self.id {
            if self.id.checked_add(1) != Some(id) {
                return Err(io::Error::other("parent activity identity gap"));
            }
            let until = anchor
                .checked_add(PARENT_ACTIVITY_NS)
                .ok_or_else(|| io::Error::other("parent activity overflow"))?;
            if at >= until {
                return Err(io::Error::other("new parent activity expired"));
            }
            self.until = until;
            self.id = id;
        }
        self.last = at;
        Ok(self.until)
    }
}
trait ConsumerControl: PollControl<EvdevSource, UinputDevice> {
    fn valid_until(&self) -> Option<MonoTime>;
}
impl ConsumerControl for ConsoleControl {
    fn valid_until(&self) -> Option<MonoTime> {
        self.lease.valid_until()
    }
}
impl ConsumerControl for X11Control<crate::x11_consumer::NativeX11Lease> {
    fn valid_until(&self) -> Option<MonoTime> {
        self.lease.valid_until()
    }
}
struct ExtendedControl<'a, C, E> {
    consumer: C,
    extension: &'a mut E,
    guard: &'a WorkerParentGuard,
    parent: Option<ParentActivity>,
    wake: Option<MonoTime>,
}
impl<C: ConsumerControl, E: WorkerExtension<EvdevSource, UinputDevice>>
    PollControl<EvdevSource, UinputDevice> for ExtendedControl<'_, C, E>
{
    fn before_poll(
        &mut self,
        runtime: &Runtime<EvdevSource, UinputDevice>,
        clock: &mut MonotonicClock,
        now: MonoTime,
    ) -> Result<MonoTime, WorkerError> {
        if runtime.phase() != RuntimePhase::Active {
            return Ok(now);
        }
        self.guard.verify_isolated_group()?;
        let now = self.consumer.before_poll(runtime, clock, now)?;
        // Initial active budget is independent of bootstrap/host watchdog; it
        // does not pretend to inherit or extend a prior parent's deadline.
        if self.parent.is_none() {
            self.parent = Some(ParentActivity::new(now, 0)?);
        }
        if let Some(parent) = self.parent.as_mut() {
            parent.observe(now, parent.id)?;
        }
        Ok(now)
    }
    fn after_poll<R: io::Read, W: io::Write>(
        &mut self,
        runtime: &mut Runtime<EvdevSource, UinputDevice>,
        client: &mut ProgressClient<R, W>,
        clock: &mut MonotonicClock,
    ) -> Result<bool, WorkerError> {
        let session_until = self
            .consumer
            .valid_until()
            .ok_or_else(|| WorkerError::Extension(io::Error::other("consumer evidence missing")))?;
        let parent = self
            .parent
            .as_mut()
            .ok_or_else(|| io::Error::other("missing parent activity"))?;
        let epoch = runtime.progress_epoch();
        let mut stopped = false;
        let result = self.extension.tick(runtime, &mut || {
            let value = observe_control(
                parent,
                client,
                epoch,
                session_until,
                &mut || clock.now(),
                &mut || self.guard.verify_isolated_group(),
            )?;
            if value.stop {
                stopped = true;
            } else {
                self.wake = Some(
                    value
                        .at
                        .checked_add(EXTENSION_WAKE_NS)
                        .ok_or_else(|| io::Error::other("extension wake overflow"))?,
                );
            }
            Ok(value)
        });
        if stopped {
            return Ok(true);
        }
        result.map_err(WorkerError::Extension)?;
        Ok(false)
    }
    fn next_wakeup(&self) -> Option<MonoTime> {
        [
            self.consumer.next_wakeup(),
            self.wake,
            self.parent.as_ref().map(|p| p.until),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn close(&mut self) {
        self.extension.close();
        self.consumer.close();
    }
}
fn observe_control<R: io::Read, W: io::Write>(
    parent: &mut ParentActivity,
    client: &mut ProgressClient<R, W>,
    epoch: u64,
    session_until: MonoTime,
    clock: &mut impl FnMut() -> io::Result<MonoTime>,
    verify: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<ControlObservation> {
    verify()?;
    let before = clock()?;
    parent.observe(before, parent.id)?;
    let event = client
        .receive_control_epoch(epoch)
        .map_err(io::Error::other)?;
    if event == RequestEvent::Stop {
        return Ok(ControlObservation {
            at: before,
            parent_until: parent.until,
            session_until,
            stop: true,
        });
    }
    let after = clock()?;
    let parent_until = parent.observe_at(after, client.last_request_id(), before)?;
    if after >= session_until {
        return Err(io::Error::other("consumer evidence expired"));
    }
    Ok(ControlObservation {
        at: after,
        parent_until,
        session_until,
        stop: false,
    })
}
/// Same guarded loop with borrowed reload extension; pending X11 owner is returned after shutdown.
pub fn run_x11_worker_with_extension<R, W, E>(
    mut runtime: Runtime<EvdevSource, UinputDevice>,
    client: ProgressClient<R, W>,
    guard: &WorkerParentGuard,
    lease: crate::x11_consumer::NativeX11Lease,
    clock: MonotonicClock,
    extension: &mut E,
) -> X11WorkerReport
where
    R: io::Read + std::os::fd::AsFd,
    W: io::Write + std::os::fd::AsFd,
    E: WorkerExtension<EvdevSource, UinputDevice>,
{
    let mut control = ExtendedControl {
        consumer: X11Control::new(lease),
        extension,
        guard,
        parent: None,
        wake: None,
    };
    let preflight = (|| {
        guard.verify_isolated_group()?;
        for request in client.wait_requests_with_control(true) {
            crate::ioctl::nonblocking(request.fd)?;
        }
        Ok::<_, io::Error>(())
    })();
    let worker = match preflight {
        Ok(()) => run_with_borrowed_poll_control(
            runtime,
            client,
            true,
            clock,
            &mut control,
            |runtime, client, clock, deadline| {
                runtime.wait_native_until(client, clock, true, deadline)
            },
        ),
        Err(error) => {
            runtime.external_fault(StopReason::ControlFault);
            runtime.finish_recovery();
            let termination = runtime.termination_report();
            drop(runtime);
            control.close();
            WorkerReport {
                outcome: Err(WorkerError::System(error)),
                termination,
            }
        }
    };
    X11WorkerReport {
        worker,
        consumer: control.consumer.lease,
        cancel_error: control.consumer.cancel_error,
    }
}
/// Borrow the original extension owner; caller can reap it after capture-first
/// recovery returns. Explicit console/bootstrap authority and same clock only.
pub fn run_console_worker_with_extension<R, W, E>(
    runtime: Runtime<EvdevSource, UinputDevice>,
    client: ProgressClient<R, W>,
    guard: &WorkerParentGuard,
    provider: crate::console::ConsoleProvider,
    mut clock: MonotonicClock,
    ready: crate::console::ConsoleReady,
    extension: &mut E,
) -> Result<WorkerReport, WorkerError>
where
    R: io::Read + std::os::fd::AsFd,
    W: io::Write + std::os::fd::AsFd,
    E: WorkerExtension<EvdevSource, UinputDevice>,
{
    guard.verify_isolated_group()?;
    for request in client.wait_requests_with_control(true) {
        crate::ioctl::nonblocking(request.fd)?;
    }
    let wake = Some(
        clock
            .now()?
            .checked_add(EXTENSION_WAKE_NS)
            .ok_or_else(|| io::Error::other("extension wake overflow"))?,
    );
    Ok(run_with_poll_control(
        runtime,
        client,
        true,
        clock,
        ExtendedControl {
            consumer: ConsoleControl {
                provider,
                lease: crate::console::Lease::new(ready),
            },
            extension,
            guard,
            parent: None,
            wake,
        },
        |runtime, client, clock, deadline| runtime.wait_native_until(client, clock, true, deadline),
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn only_new_contiguous_parent_requests_renew_strict_activity_budget() {
        let mut lease = ParentActivity::new(MonoTime(10), 0).unwrap();
        let initial = lease.until;
        assert_eq!(lease.observe(MonoTime(20), 0).unwrap(), initial);
        assert_eq!(
            lease.observe(MonoTime(30), 1).unwrap(),
            MonoTime(30 + PARENT_ACTIVITY_NS)
        );
        assert_eq!(
            lease.observe(MonoTime(40), 1).unwrap(),
            MonoTime(30 + PARENT_ACTIVITY_NS)
        );
        assert!(lease.observe(MonoTime(30 + PARENT_ACTIVITY_NS), 2).is_err());
    }
    #[test]
    fn parent_activity_rejects_gap_backwards_overflow_and_replay() {
        assert!(ParentActivity::new(MonoTime(u64::MAX), 0).is_err());
        for (at, id) in [(9, 0), (20, 2)] {
            let mut lease = ParentActivity::new(MonoTime(10), 0).unwrap();
            assert!(lease.observe(MonoTime(at), id).is_err());
        }
        let mut lease = ParentActivity::new(MonoTime(10), 0).unwrap();
        lease.observe(MonoTime(20), 1).unwrap();
        assert!(lease.observe(MonoTime(30), 0).is_err());
    }
    #[test]
    fn request_observation_anchors_before_io_and_cannot_bridge_old_expiry() {
        let mut lease = ParentActivity::new(MonoTime(10), 0).unwrap();
        lease.observe(MonoTime(20), 0).unwrap();
        assert_eq!(
            lease.observe_at(MonoTime(30), 1, MonoTime(20)).unwrap(),
            MonoTime(20 + PARENT_ACTIVITY_NS)
        );
        assert!(
            lease
                .observe_at(MonoTime(20 + PARENT_ACTIVITY_NS), 2, MonoTime(40))
                .is_err()
        );
    }
    struct Blocked;
    impl io::Read for Blocked {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::WouldBlock.into())
        }
    }
    #[test]
    fn control_stop_wins_over_session_expiry_without_second_clock_query() {
        let bytes = crate::progress::StopRequest { run_id: 7, id: 1 }.encode();
        let mut client = ProgressClient::new(io::Cursor::new(bytes), io::sink(), 7).unwrap();
        let mut parent = ParentActivity::new(MonoTime(0), 0).unwrap();
        let mut reads = 0;
        let value = observe_control(
            &mut parent,
            &mut client,
            1,
            MonoTime(10),
            &mut || {
                reads += 1;
                Ok(MonoTime(20))
            },
            &mut || Ok(()),
        )
        .unwrap();
        assert!(value.stop);
        assert_eq!(reads, 1);
        assert_eq!(parent.id, 0);
    }
    #[test]
    fn extension_observation_does_not_extend_session_or_parent_on_empty_io() {
        let mut client = ProgressClient::new(Blocked, io::sink(), 7).unwrap();
        let mut parent = ParentActivity::new(MonoTime(0), 0).unwrap();
        let original = parent.until;
        let mut times = [MonoTime(10), MonoTime(20)].into_iter();
        let value = observe_control(
            &mut parent,
            &mut client,
            1,
            MonoTime(100),
            &mut || Ok(times.next().unwrap()),
            &mut || Ok(()),
        )
        .unwrap();
        assert!(!value.stop);
        assert_eq!(value.session_until, MonoTime(100));
        assert_eq!(value.parent_until, original);
        assert_eq!(parent.until, original);
        let mut times = [MonoTime(30), MonoTime(100)].into_iter();
        assert!(
            observe_control(
                &mut parent,
                &mut client,
                1,
                MonoTime(100),
                &mut || Ok(times.next().unwrap()),
                &mut || Ok(())
            )
            .is_err()
        );
    }
    #[test]
    fn guard_failure_precedes_control_read_and_clock() {
        struct NeverRead;
        impl io::Read for NeverRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("must not read");
            }
        }
        let mut client = ProgressClient::new(NeverRead, io::sink(), 7).unwrap();
        let mut parent = ParentActivity::new(MonoTime(0), 0).unwrap();
        assert!(
            observe_control(
                &mut parent,
                &mut client,
                1,
                MonoTime(100),
                &mut || panic!("must not query clock"),
                &mut || Err(io::Error::other("guard"))
            )
            .is_err()
        );
    }
    #[test]
    fn expired_parent_activity_prevents_reload_control_io() {
        struct NeverRead;
        impl io::Read for NeverRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("must not read");
            }
        }
        let mut client = ProgressClient::new(NeverRead, io::sink(), 7).unwrap();
        let mut parent = ParentActivity::new(MonoTime(0), 0).unwrap();
        assert!(
            observe_control(
                &mut parent,
                &mut client,
                1,
                MonoTime(u64::MAX),
                &mut || Ok(MonoTime(PARENT_ACTIVITY_NS)),
                &mut || Ok(())
            )
            .is_err()
        );
    }
}
