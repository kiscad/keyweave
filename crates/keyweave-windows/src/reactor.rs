//! Shared epoch and bounded waits; engine still receives explicit delivery time.
use crate::{
    hook::HookFault,
    pump::SendBackend,
    runtime::{RuntimeError, SessionActor},
};
use keyweave_types::MonoTime;
use std::{cell::RefCell, rc::Rc, time::Instant};
pub const HOUSEKEEPING_MS: u32 = 20;
#[derive(Clone)]
pub struct SessionClock {
    epoch: Instant,
}
impl Default for SessionClock {
    fn default() -> Self {
        Self::new()
    }
}
impl SessionClock {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
    pub fn now(&self) -> Option<MonoTime> {
        let elapsed = Instant::now().checked_duration_since(self.epoch)?;
        u64::try_from(elapsed.as_nanos()).ok().map(MonoTime)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitPlan {
    timeout_ms: u32,
}
impl WaitPlan {
    pub fn for_deadline(now: MonoTime, deadline: Option<MonoTime>) -> Self {
        let timeout_ms = deadline.map_or(HOUSEKEEPING_MS, |at| {
            let ns = at.0.saturating_sub(now.0);
            ns.div_ceil(1_000_000).min(u64::from(HOUSEKEEPING_MS)) as u32
        });
        Self { timeout_ms }
    }
    pub fn timeout_ms(self) -> u32 {
        self.timeout_ms
    }
}
/// Host shares one clock epoch with ActorHandler; call only outside the callback.
pub struct SessionControl<B: SendBackend, C: FnMut() -> Option<MonoTime>> {
    pub actor: Rc<RefCell<SessionActor<B>>>,
    pub clock: C,
}
impl<B: SendBackend, C: FnMut() -> Option<MonoTime>> SessionControl<B, C> {
    pub fn next_wait(&mut self) -> Result<WaitPlan, HookFault> {
        let mut actor = self
            .actor
            .try_borrow_mut()
            .map_err(|_| HookFault::Reentrant)?;
        let Some(now) = (self.clock)() else {
            actor.control_failed(RuntimeError::Clock);
            return Err(HookFault::Handler);
        };
        if let Err(error) = actor.tick(now) {
            actor.control_failed(error);
            return Err(HookFault::Handler);
        }
        Ok(WaitPlan::for_deadline(now, actor.next_wakeup()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waits_round_up_and_never_overflow_or_wait_indefinitely() {
        for (now, deadline, expected) in [
            (0, None, 20),
            (2, Some(1), 0),
            (1, Some(1), 0),
            (0, Some(1), 1),
            (0, Some(1_000_001), 2),
            (0, Some(u64::MAX), 20),
            (u64::MAX, Some(u64::MAX), 0),
        ] {
            assert_eq!(
                WaitPlan::for_deadline(MonoTime(now), deadline.map(MonoTime)).timeout_ms(),
                expected
            );
        }
    }
    #[test]
    fn native_clock_clones_keep_one_epoch() {
        let clock = SessionClock::new();
        let before = clock.now();
        let clone = clock.clone();
        assert_eq!(clock.epoch, clone.epoch);
        let after = clone.now();
        assert!(before.is_some() && after.is_some() && after >= before);
    }
}
