//! Explicit delivery-time lease, renewed only after real access observation.
use keyweave_types::MonoTime;
pub const REFRESH_NS: u64 = 100_000_000;
pub const LEASE_NS: u64 = 500_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AccessError {
    #[error("input access has not been checked")]
    Ungranted,
    #[error("input access lease expired")]
    Expired,
    #[error("input access delivery time went backwards")]
    ClockBackwards,
    #[error("input access deadline overflow")]
    TimeOverflow,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessLease {
    checked_at: Option<MonoTime>,
    refresh_at: Option<MonoTime>,
    expires_at: Option<MonoTime>,
}
impl AccessLease {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn check(&self, now: MonoTime) -> Result<(), AccessError> {
        let checked_at = self.checked_at.ok_or(AccessError::Ungranted)?;
        if now < checked_at {
            return Err(AccessError::ClockBackwards);
        }
        if self.expires_at.is_none_or(|at| now >= at) {
            return Err(AccessError::Expired);
        }
        Ok(())
    }
    pub fn refresh_due(&self, now: MonoTime) -> Result<bool, AccessError> {
        self.check(now)?;
        Ok(self.refresh_at.is_some_and(|at| now >= at))
    }
    pub fn next_refresh(&self) -> Option<MonoTime> {
        self.refresh_at
    }
    /// Caller proves metadata access first and supplies completion time. A
    /// previously issued but expired lease is terminal; late results cannot renew.
    pub fn renew_after_probe(&mut self, now: MonoTime) -> Result<(), AccessError> {
        if self.checked_at.is_some() {
            self.check(now)?;
        }
        let refresh_at = now
            .checked_add(REFRESH_NS)
            .ok_or(AccessError::TimeOverflow)?;
        let expires_at = now.checked_add(LEASE_NS).ok_or(AccessError::TimeOverflow)?;
        self.checked_at = Some(now);
        self.refresh_at = Some(refresh_at);
        self.expires_at = Some(expires_at);
        Ok(())
    }
}
#[cfg(test)]
mod tests;
