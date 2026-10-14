//! Single-run asynchronous lease. Revocation never permits rearm or gap bridging.
use super::{NativeX11Observer, X11Observation};
use crate::{
    device::EvdevDevice, graphical_session::SESSION_LIFETIME_NS, uinput::UinputDevice,
    x11_probe::ProbePhase,
};
use keyweave_types::MonoTime;
use std::{io, path::Path};
const REFRESH_NS: u64 = 100_000_000;
const POLL_NS: u64 = 2_000_000;
#[derive(Debug, thiserror::Error)]
pub enum X11LeaseError {
    #[error("X11 consumer evidence is missing, expired or invalid")]
    NotReady,
    #[error("X11 consumer output identity changed")]
    OutputMismatch,
    #[error("X11 consumer lease clock regressed")]
    Clock,
    #[error("X11 consumer observation failed: {0}")]
    Provider(#[source] io::Error),
}
struct Window {
    ready: Option<X11Observation>,
    last: Option<MonoTime>,
    first: bool,
}
impl Window {
    fn new(ready: X11Observation) -> Self {
        Self {
            ready: Some(ready),
            last: None,
            first: true,
        }
    }
    fn revoke(&mut self) {
        self.ready = None;
    }
    fn verify(&mut self, path: &Path, now: MonoTime) -> Result<(), X11LeaseError> {
        let result = (|| {
            if self.last.is_some_and(|last| now < last) {
                return Err(X11LeaseError::Clock);
            }
            self.last = Some(now);
            let ready = self.ready.as_ref().ok_or(X11LeaseError::NotReady)?;
            if ready.output != path {
                return Err(X11LeaseError::OutputMismatch);
            }
            if ready.observed.checked_add(SESSION_LIFETIME_NS) != Some(ready.until)
                || !ready.fresh_for(path, now)
            {
                return Err(X11LeaseError::NotReady);
            }
            Ok(())
        })();
        if result.is_err() {
            self.revoke();
        }
        result
    }
    fn needs_refresh(&self, now: MonoTime) -> bool {
        self.first
            || self.ready.as_ref().is_some_and(|ready| {
                ready
                    .observed
                    .checked_add(REFRESH_NS)
                    .is_none_or(|refresh| now >= refresh)
            })
    }
    fn renew(
        &mut self,
        path: &Path,
        now: MonoTime,
        ready: X11Observation,
    ) -> Result<(), X11LeaseError> {
        self.verify(path, now)?;
        let old = self.ready.as_ref().ok_or(X11LeaseError::NotReady)?;
        if ready.output != path
            || !ready.fresh_for(path, now)
            || ready.observed.checked_add(SESSION_LIFETIME_NS) != Some(ready.until)
            || ready.observed <= old.observed
            || ready.until <= old.until
        {
            self.revoke();
            return Err(X11LeaseError::NotReady);
        }
        self.ready = Some(ready);
        self.first = false;
        Ok(())
    }
    fn wakeup(&self, pending: bool) -> Option<MonoTime> {
        let ready = self.ready.as_ref()?;
        let next = if pending {
            self.last?.checked_add(POLL_NS)?
        } else if self.first {
            ready.observed
        } else {
            ready.observed.checked_add(REFRESH_NS)?
        };
        Some(next.min(ready.until))
    }
}
/// Owns output-node observer FD, never a physical input. Same monotonic clock as admission.
pub struct NativeX11Lease {
    observer: NativeX11Observer,
    node: EvdevDevice,
    window: Window,
}
impl NativeX11Lease {
    /// Consumes this observer's completed evidence; cannot mix another observer's token.
    pub fn new(mut observer: NativeX11Observer, node: EvdevDevice) -> Self {
        let ready = if observer.job_phase().is_none() {
            observer.take_observation()
        } else {
            None
        };
        // Missing completed evidence is checked before dispatch; keep pending owners for cleanup.
        let window = ready.map_or(
            Window {
                ready: None,
                last: None,
                first: true,
            },
            Window::new,
        );
        Self {
            observer,
            node,
            window,
        }
    }
    pub fn check(
        &mut self,
        output: &UinputDevice,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<MonoTime, X11LeaseError> {
        let result = self.check_inner(output, clock);
        if result.is_err() {
            self.window.revoke();
            let _ = self.observer.cancel();
        }
        result
    }
    fn check_inner(
        &mut self,
        output: &UinputDevice,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<MonoTime, X11LeaseError> {
        let now = clock().map_err(X11LeaseError::Provider)?;
        let path = output.input_syspath();
        self.window.verify(path, now)?;
        if self.observer.job_phase().is_some() {
            self.observer
                .poll(output, &self.node, clock)
                .map_err(X11LeaseError::Provider)?;
            if self.observer.job_failed() {
                return Err(X11LeaseError::NotReady);
            }
            let after = clock().map_err(X11LeaseError::Provider)?;
            self.window.verify(path, after)?;
            if let Some(ready) = self.observer.take_observation() {
                self.window.renew(path, after, ready)?;
            } else if self.observer.job_phase().is_none() {
                return Err(X11LeaseError::NotReady);
            }
        }
        let now = clock().map_err(X11LeaseError::Provider)?;
        self.window.verify(path, now)?;
        if self.observer.job_phase().is_none()
            && self.window.needs_refresh(now)
            && !self
                .observer
                .begin(output, &self.node, clock)
                .map_err(X11LeaseError::Provider)?
        {
            return Err(X11LeaseError::NotReady);
        }
        let after = clock().map_err(X11LeaseError::Provider)?;
        self.window.verify(path, after)?;
        Ok(after)
    }
    pub fn next_wakeup(&self) -> Option<MonoTime> {
        let window = self.window.wakeup(self.observer.job_phase().is_some())?;
        Some(
            self.observer
                .next_deadline()
                .map_or(window, |job| job.min(window)),
        )
    }
    pub fn cancel(&mut self) -> io::Result<()> {
        self.window.revoke();
        self.observer.cancel()
    }
    pub fn poll_canceled(&mut self) -> io::Result<Option<ProbePhase>> {
        self.observer.poll_canceled()
    }
    pub fn job_phase(&self) -> Option<ProbePhase> {
        self.observer.job_phase()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn path() -> std::path::PathBuf {
        crate::mock_syspath("/sys/devices/virtual/input/input42")
    }
    fn ready(at: u64) -> X11Observation {
        X11Observation::for_test(path(), MonoTime(at), MonoTime(at + SESSION_LIFETIME_NS))
    }
    #[test]
    fn pending_query_keeps_original_expiry_and_poll_wakeup() {
        let mut window = Window::new(ready(0));
        window.verify(&path(), MonoTime(1)).unwrap();
        assert!(window.needs_refresh(MonoTime(1)));
        assert_eq!(window.wakeup(true), Some(MonoTime(1 + POLL_NS)));
        window
            .verify(&path(), MonoTime(SESSION_LIFETIME_NS - 1))
            .unwrap();
        assert_eq!(window.wakeup(true), Some(MonoTime(SESSION_LIFETIME_NS)));
        assert!(
            window
                .verify(&path(), MonoTime(SESSION_LIFETIME_NS))
                .is_err()
        );
        assert_eq!(window.wakeup(true), None);
    }
    #[test]
    fn renewal_before_expiry_moves_deadlines_from_new_start() {
        let mut window = Window::new(ready(0));
        window
            .renew(&path(), MonoTime(110_000_000), ready(100_000_000))
            .unwrap();
        assert_eq!(window.wakeup(false), Some(MonoTime(200_000_000)));
        assert!(!window.needs_refresh(MonoTime(199_999_999)));
        assert!(window.needs_refresh(MonoTime(200_000_000)));
    }
    #[test]
    fn completed_new_report_cannot_bridge_old_expiry() {
        let mut window = Window::new(ready(0));
        assert!(
            window
                .renew(&path(), MonoTime(SESSION_LIFETIME_NS), ready(200_000_000))
                .is_err()
        );
        assert!(
            window
                .renew(
                    &path(),
                    MonoTime(SESSION_LIFETIME_NS + 1),
                    ready(SESSION_LIFETIME_NS)
                )
                .is_err()
        );
        assert_eq!(window.wakeup(false), None);
    }
    #[test]
    fn same_or_malformed_observation_never_renews() {
        for next in [
            ready(0),
            X11Observation::for_test(path(), MonoTime(1), MonoTime(2)),
            X11Observation::for_test(
                crate::mock_syspath("/sys/other"),
                MonoTime(1),
                MonoTime(1 + SESSION_LIFETIME_NS),
            ),
        ] {
            let mut window = Window::new(ready(0));
            assert!(window.renew(&path(), MonoTime(10), next).is_err());
            assert_eq!(window.wakeup(false), None);
        }
    }
    #[test]
    fn identity_clock_or_initial_window_failure_is_sticky() {
        let mut window = Window::new(ready(0));
        window.verify(&path(), MonoTime(10)).unwrap();
        assert!(matches!(
            window.verify(&path(), MonoTime(9)),
            Err(X11LeaseError::Clock)
        ));
        assert!(window.verify(&path(), MonoTime(11)).is_err());
        let mut window = Window::new(ready(0));
        assert!(matches!(
            window.verify(&crate::mock_syspath("/sys/other"), MonoTime(1)),
            Err(X11LeaseError::OutputMismatch)
        ));
        assert!(window.verify(&path(), MonoTime(2)).is_err());
        let mut window = Window::new(X11Observation::for_test(path(), MonoTime(0), MonoTime(10)));
        assert!(window.verify(&path(), MonoTime(1)).is_err());
    }
}
