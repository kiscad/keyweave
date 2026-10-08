//! Physical Ctrl+Alt+Shift+F12, independent of all mapping configuration.
use crate::probe::Bits;
use keyweave_types::MonoTime;
use thiserror::Error;
pub const HOLD_NS: u64 = 1_000_000_000;
// Canonical evdev codes, checked against the generated HID/native table in tests.
const KEYS: [u16; 7] = [29, 97, 56, 100, 42, 54, 88];
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum EmergencyError {
    #[error("emergency clock went backwards")]
    ClockBackwards,
    #[error("emergency deadline overflows monotonic time")]
    TimeOverflow,
    #[error("invalid physical key value")]
    InvalidValue,
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
    pub fn supported(keys: &Bits) -> bool {
        keys.contains(KEYS[6])
            && KEYS[..2].iter().any(|key| keys.contains(*key))
            && KEYS[2..4].iter().any(|key| keys.contains(*key))
            && KEYS[4..6].iter().any(|key| keys.contains(*key))
    }
    fn complete(mask: u8) -> bool {
        mask & 3 != 0 && mask & 12 != 0 && mask & 48 != 0 && mask & 64 != 0
    }
    /// Whole-frame validation precedes this call. Only physical down/up change
    /// the seven tracked bits; orphan repeat never establishes a pressed key.
    pub fn observe(&mut self, code: u16, value: i32, now: MonoTime) -> Result<(), EmergencyError> {
        if now < self.now {
            return Err(EmergencyError::ClockBackwards);
        }
        if !(0..=2).contains(&value) {
            return Err(EmergencyError::InvalidValue);
        }
        let bit = KEYS
            .iter()
            .position(|key| *key == code)
            .map_or(0, |index| 1u8 << index);
        let mask = match value {
            0 => self.mask & !bit,
            1 => self.mask | bit,
            _ => self.mask,
        };
        let deadline = if Self::complete(mask) && !self.fired {
            Some(match self.deadline {
                Some(deadline) => deadline,
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
    /// Trigger once at >= hold duration. Caller invokes before new physical
    /// input at this time; an earlier observed up cancels the deadline.
    pub fn advance(&mut self, now: MonoTime) -> Result<bool, EmergencyError> {
        if now < self.now {
            return Err(EmergencyError::ClockBackwards);
        }
        self.now = now;
        if self.deadline.is_some_and(|deadline| deadline <= now) {
            self.fired = true;
            self.deadline = None;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn reset(&mut self) {
        self.mask = 0;
        self.deadline = None;
        self.fired = false;
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn chord() -> EmergencyChord {
        let mut chord = EmergencyChord::new();
        for code in [29, 56, 42, 88] {
            chord.observe(code, 1, MonoTime(10)).unwrap();
        }
        chord
    }
    #[test]
    fn threshold_and_single_fire_are_exact() {
        let mut chord = chord();
        assert_eq!(chord.next_deadline(), Some(MonoTime(10 + HOLD_NS)));
        assert!(!chord.advance(MonoTime(9 + HOLD_NS)).unwrap());
        assert!(chord.advance(MonoTime(10 + HOLD_NS)).unwrap());
        assert!(!chord.advance(MonoTime(11 + HOLD_NS)).unwrap());
        chord.observe(88, 0, MonoTime(12 + HOLD_NS)).unwrap();
        chord.observe(88, 1, MonoTime(13 + HOLD_NS)).unwrap();
        assert!(chord.next_deadline().is_none());
    }
    #[test]
    fn missing_groups_and_orphan_repeats_cannot_arm() {
        for missing in [29, 56, 42, 88] {
            let mut chord = EmergencyChord::new();
            for code in [29, 56, 42, 88] {
                chord
                    .observe(code, if code == missing { 2 } else { 1 }, MonoTime(0))
                    .unwrap();
            }
            assert!(chord.next_deadline().is_none());
            assert!(!chord.advance(MonoTime(HOLD_NS)).unwrap());
        }
    }
    #[test]
    fn every_left_right_variant_and_overlap_is_supported() {
        for ctrl in [29, 97] {
            for alt in [56, 100] {
                for shift in [42, 54] {
                    let mut chord = EmergencyChord::new();
                    for code in [ctrl, alt, shift, 88] {
                        chord.observe(code, 1, MonoTime(0)).unwrap();
                    }
                    assert!(chord.advance(MonoTime(HOLD_NS)).unwrap());
                }
            }
        }
        let mut chord = chord();
        chord.observe(97, 1, MonoTime(20)).unwrap();
        chord.observe(29, 0, MonoTime(30)).unwrap();
        assert_eq!(chord.next_deadline(), Some(MonoTime(10 + HOLD_NS)));
    }
    #[test]
    fn repeat_duplicate_and_other_keys_do_not_refresh_deadline() {
        let mut chord = chord();
        for (code, value) in [(88, 2), (29, 1), (30, 1), (30, 0)] {
            chord.observe(code, value, MonoTime(20)).unwrap();
        }
        assert_eq!(chord.next_deadline(), Some(MonoTime(10 + HOLD_NS)));
    }
    #[test]
    fn release_cancels_and_repress_restarts_full_interval() {
        let mut chord = chord();
        chord.observe(88, 0, MonoTime(HOLD_NS)).unwrap();
        assert!(!chord.advance(MonoTime(HOLD_NS + 10)).unwrap());
        chord.observe(88, 1, MonoTime(HOLD_NS + 20)).unwrap();
        assert_eq!(chord.next_deadline(), Some(MonoTime(2 * HOLD_NS + 20)));
        chord.reset();
        assert!(chord.next_deadline().is_none());
        assert!(matches!(
            chord.observe(88, 1, MonoTime(0)),
            Err(EmergencyError::ClockBackwards)
        ));
    }
    #[test]
    fn failures_do_not_partially_change_detector() {
        let mut chord = chord();
        let saved = chord.clone();
        assert_eq!(
            chord.observe(88, 0, MonoTime(9)),
            Err(EmergencyError::ClockBackwards)
        );
        assert_eq!(chord, saved);
        assert_eq!(
            chord.observe(88, 3, MonoTime(20)),
            Err(EmergencyError::InvalidValue)
        );
        assert_eq!(chord, saved);
        let mut chord = EmergencyChord::new();
        for code in [29, 56, 42] {
            chord.observe(code, 1, MonoTime(u64::MAX)).unwrap();
        }
        let saved = chord.clone();
        assert_eq!(
            chord.observe(88, 1, MonoTime(u64::MAX)),
            Err(EmergencyError::TimeOverflow)
        );
        assert_eq!(chord, saved);
    }
    #[test]
    fn native_constants_match_hid_table_and_admission_requires_all_groups() {
        for (name, code) in [
            "left_control",
            "right_control",
            "left_alt",
            "right_alt",
            "left_shift",
            "right_shift",
            "f12",
        ]
        .into_iter()
        .zip(KEYS)
        {
            assert_eq!(
                crate::keys::output_code(name.parse().unwrap()).unwrap(),
                code
            );
        }
        let mut bits = Bits::empty(768).unwrap();
        for code in [97, 100, 54, 88] {
            bits.set(code).unwrap();
        }
        assert!(EmergencyChord::supported(&bits));
        let mut bits = Bits::empty(768).unwrap();
        for code in [97, 100, 54] {
            bits.set(code).unwrap();
        }
        assert!(!EmergencyChord::supported(&bits));
    }
}
