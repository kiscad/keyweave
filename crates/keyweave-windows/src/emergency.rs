//! Fixed physical-stream escape, independent of configuration and layout.
//! Windows Hook has no trustworthy per-event keyboard identity: this detector
//! deliberately aggregates keyboards. Injected filtering belongs to dispatcher.
use keyweave_types::{KEYBOARD_PAGE, KeyEdge, KeyId, MonoTime};
pub const HOLD_NS: u64 = 1_000_000_000;
const USAGES: [u16; 7] = [0xe0, 0xe4, 0xe2, 0xe6, 0xe1, 0xe5, 0x45];
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EmergencyError {
    #[error("emergency delivery time went backwards")]
    ClockBackwards,
    #[error("emergency deadline overflow")]
    TimeOverflow,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmergencyChord {
    mask: u8,
    now: MonoTime,
    deadline: Option<MonoTime>,
    fired: bool,
}
impl EmergencyChord {
    pub fn new() -> Self {
        Self::default()
    }
    fn complete(mask: u8) -> bool {
        mask & 3 != 0 && mask & 12 != 0 && mask & 48 != 0 && mask & 64 != 0
    }
    /// Caller first advances at this same time, before ordinary events. Repeat
    /// cannot establish a key or extend an already scheduled escape deadline.
    pub fn observe(
        &mut self,
        key: KeyId,
        edge: KeyEdge,
        now: MonoTime,
    ) -> Result<(), EmergencyError> {
        if now < self.now {
            return Err(EmergencyError::ClockBackwards);
        }
        let bit = if key.page == KEYBOARD_PAGE {
            USAGES
                .iter()
                .position(|usage| *usage == key.usage)
                .map_or(0, |index| 1 << index)
        } else {
            0
        };
        let mask = match edge {
            KeyEdge::Down => self.mask | bit,
            KeyEdge::Up => self.mask & !bit,
            KeyEdge::Repeat => self.mask,
        };
        let deadline = if Self::complete(mask) && !self.fired {
            Some(match self.deadline {
                Some(at) => at,
                None => now
                    .checked_add(HOLD_NS)
                    .ok_or(EmergencyError::TimeOverflow)?,
            })
        } else {
            None
        };
        self.mask = mask;
        self.now = now;
        self.deadline = deadline;
        Ok(())
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.deadline
    }
    /// Returns true once. The enclosing session is terminal after this result.
    pub fn advance(&mut self, now: MonoTime) -> Result<bool, EmergencyError> {
        if now < self.now {
            return Err(EmergencyError::ClockBackwards);
        }
        self.now = now;
        if self.deadline.is_some_and(|at| at <= now) {
            self.deadline = None;
            self.fired = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
#[cfg(test)]
mod tests;
