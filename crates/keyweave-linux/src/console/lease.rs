use super::ConsoleReady;
use keyweave_types::MonoTime;
use std::{io, path::Path};
pub(crate) const REFRESH_NS: u64 = 100_000_000;
#[derive(Debug, thiserror::Error)]
pub enum ConsoleLeaseError {
    #[error("console evidence is missing, expired or invalid")]
    NotReady,
    #[error("console evidence belongs to another output")]
    OutputMismatch,
    #[error("console lease clock moved backwards")]
    Clock,
    #[error("console provider query failed: {0}")]
    Provider(#[source] io::Error),
}
/// Single active run, no recovery/rearm. Invalidating never retains old grant.
pub(crate) struct Lease {
    ready: Option<ConsoleReady>,
    refresh: MonoTime,
    first: bool,
    last: Option<MonoTime>,
}
impl Lease {
    pub(crate) fn new(ready: ConsoleReady) -> Self {
        Self {
            refresh: ready.observed,
            ready: Some(ready),
            first: true,
            last: None,
        }
    }
    fn fault<T>(&mut self, error: ConsoleLeaseError) -> Result<T, ConsoleLeaseError> {
        self.ready = None;
        Err(error)
    }
    fn verify(&mut self, path: &Path, now: MonoTime) -> Result<(), ConsoleLeaseError> {
        if self.last.is_some_and(|old| now < old) {
            return self.fault(ConsoleLeaseError::Clock);
        }
        self.last = Some(now);
        let Some(ready) = self.ready.as_ref() else {
            return Err(ConsoleLeaseError::NotReady);
        };
        if ready.output != path {
            return self.fault(ConsoleLeaseError::OutputMismatch);
        }
        if !ready.fresh_for(path, now) {
            return self.fault(ConsoleLeaseError::NotReady);
        }
        Ok(())
    }
    pub(crate) fn check(&mut self, path: &Path, now: MonoTime) -> Result<bool, ConsoleLeaseError> {
        self.verify(path, now)?;
        Ok(self.first || now >= self.refresh)
    }
    pub(crate) fn renew(
        &mut self,
        path: &Path,
        now: MonoTime,
        ready: Option<ConsoleReady>,
    ) -> Result<(), ConsoleLeaseError> {
        // Do not bridge a gap while a query was in flight, even with a new grant.
        self.verify(path, now)?;
        let Some(ready) = ready else {
            return self.fault(ConsoleLeaseError::NotReady);
        };
        if ready.output != path {
            return self.fault(ConsoleLeaseError::OutputMismatch);
        }
        let Some(refresh) = ready.observed.checked_add(REFRESH_NS) else {
            return self.fault(ConsoleLeaseError::NotReady);
        };
        if !ready.fresh_for(path, now) || refresh >= ready.until || now >= refresh {
            return self.fault(ConsoleLeaseError::NotReady);
        }
        self.ready = Some(ready);
        self.refresh = refresh;
        self.first = false;
        Ok(())
    }
    pub(crate) fn provider_failed(&mut self, error: io::Error) -> ConsoleLeaseError {
        self.ready = None;
        ConsoleLeaseError::Provider(error)
    }
    pub(crate) fn next_wakeup(&self) -> Option<MonoTime> {
        self.ready
            .as_ref()
            .map(|ready| self.refresh.min(ready.until))
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    const OUTPUT: &str = "/sys/devices/virtual/input/input42";
    fn ready(at: u64) -> ConsoleReady {
        ConsoleReady::for_test(
            OUTPUT.into(),
            MonoTime(at),
            MonoTime(at + super::super::CONSOLE_LIFETIME_NS),
        )
    }
    #[test]
    fn first_dispatch_requires_query_and_refresh_is_anchored_to_observation() {
        let mut lease = Lease::new(ready(0));
        let path = Path::new(OUTPUT);
        assert!(lease.check(path, MonoTime(10)).unwrap());
        lease.renew(path, MonoTime(20), Some(ready(10))).unwrap();
        assert_eq!(lease.next_wakeup(), Some(MonoTime(10 + REFRESH_NS)));
        assert!(!lease.check(path, MonoTime(10 + REFRESH_NS - 1)).unwrap());
        assert!(lease.check(path, MonoTime(10 + REFRESH_NS)).unwrap());
    }
    #[test]
    fn unknown_or_wrong_output_revokes_old_grant_permanently() {
        for grant in [
            None,
            Some(ConsoleReady::for_test(
                "other".into(),
                MonoTime(1),
                MonoTime(250_000_001),
            )),
        ] {
            let mut lease = Lease::new(ready(0));
            let path = Path::new(OUTPUT);
            assert!(lease.renew(path, MonoTime(1), grant).is_err());
            assert_eq!(lease.next_wakeup(), None);
            assert!(lease.check(path, MonoTime(2)).is_err());
            assert!(lease.renew(path, MonoTime(2), Some(ready(2))).is_err());
        }
    }
    #[test]
    fn exact_expiry_or_backwards_time_cannot_be_renewed() {
        let mut lease = Lease::new(ready(0));
        let path = Path::new(OUTPUT);
        assert!(
            lease
                .renew(path, MonoTime(250_000_000), Some(ready(250_000_000)))
                .is_err()
        );
        let mut lease = Lease::new(ready(0));
        lease.check(path, MonoTime(10)).unwrap();
        assert!(matches!(
            lease.check(path, MonoTime(9)),
            Err(ConsoleLeaseError::Clock)
        ));
        assert_eq!(lease.next_wakeup(), None);
    }
    #[test]
    fn slow_query_or_malformed_time_window_does_not_extend_validity() {
        let path = Path::new(OUTPUT);
        for grant in [
            ready(0),
            ConsoleReady::for_test(OUTPUT.into(), MonoTime(10), MonoTime(20)),
            ready(200_000_000),
        ] {
            let mut lease = Lease::new(ready(0));
            assert!(
                lease
                    .renew(path, MonoTime(REFRESH_NS), Some(grant))
                    .is_err()
            );
            assert_eq!(lease.next_wakeup(), None);
        }
    }
    #[test]
    fn provider_error_retains_errno_and_revokes_before_retry() {
        let mut lease = Lease::new(ready(0));
        assert!(
            matches!(lease.provider_failed(io::Error::from_raw_os_error(5)),ConsoleLeaseError::Provider(error) if error.raw_os_error()==Some(5))
        );
        assert!(lease.check(Path::new(OUTPUT), MonoTime(1)).is_err());
    }
}
