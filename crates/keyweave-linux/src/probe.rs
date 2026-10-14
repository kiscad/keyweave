//! Portable interpretation of native capability/state queries, not a grab gate.
use crate::{events::KEY_MAX, keys::native_codes};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bits {
    words: Vec<u64>,
    count: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[error("invalid or oversized native bit mask")]
pub struct InvalidBits;
impl Bits {
    pub fn empty(count: usize) -> Result<Self, InvalidBits> {
        if count == 0 || count > usize::from(KEY_MAX) + 1 {
            return Err(InvalidBits);
        }
        Ok(Self {
            words: vec![0; count.div_ceil(64)],
            count,
        })
    }
    /// Audited native ioctl ABI has 64-bit native-endian unsigned long words.
    pub fn from_native_words(bytes: &[u8], count: usize) -> Result<Self, InvalidBits> {
        let mut bits = Self::empty(count)?;
        if bytes.len() != bits.words.len() * 8 {
            return Err(InvalidBits);
        }
        // The exact length check above excludes any incomplete trailing word.
        for (word, chunk) in bits.words.iter_mut().zip(bytes.as_chunks::<8>().0) {
            *word = u64::from_ne_bytes(*chunk);
        }
        // Bits outside the requested known range are not silently truncated.
        if !count.is_multiple_of(64)
            && bits
                .words
                .last()
                .is_some_and(|word| word >> (count % 64) != 0)
        {
            return Err(InvalidBits);
        }
        Ok(bits)
    }
    pub fn set(&mut self, code: u16) -> Result<(), InvalidBits> {
        let code = usize::from(code);
        if code >= self.count {
            return Err(InvalidBits);
        }
        self.words[code / 64] |= 1u64 << (code % 64);
        Ok(())
    }
    pub fn contains(&self, code: u16) -> bool {
        let code = usize::from(code);
        code < self.count && self.words[code / 64] & (1u64 << (code % 64)) != 0
    }
    pub fn codes(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.count as u16).filter(|code| self.contains(*code))
    }
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }
    pub fn count(&self) -> usize {
        self.count
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceId {
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
}
#[derive(Clone, Debug)]
pub struct Probe {
    pub id: DeviceId,
    /// Byte string, never sufficient to recognize our virtual output.
    pub name: Option<Vec<u8>>,
    pub input_syspath: std::path::PathBuf,
    pub capabilities: Capabilities,
}
#[derive(Clone, Debug)]
pub struct Capabilities {
    pub events: Bits,
    pub keys: Bits,
    pub relative: Bits,
    pub misc: Bits,
    pub leds: Bits,
    pub properties: Bits,
}
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum AdmissionIssue {
    #[error("EV_KEY capability is absent or no key is advertised")]
    NoKeys,
    #[error("unsupported pointer/touch/sensor property {0}")]
    Property(u16),
    #[error("unsupported event class {0}")]
    EventClass(u16),
    #[error("unsupported native key code {0}")]
    Key(u16),
    #[error("unsupported miscellaneous code {0}")]
    Misc(u16),
    #[error("unsupported LED code {0}")]
    Led(u16),
    #[error("unsupported relative axis {0}")]
    Relative(u16),
}
impl Capabilities {
    /// Explicit relative scope; existing key/property checks remain in force.
    pub fn relative_issues(&self) -> Vec<AdmissionIssue> {
        let mut issues = self.keyboard_issues();
        if self.events.contains(crate::relative::EV_REL) && !self.relative.is_empty() {
            issues.retain(|issue| !matches!(issue, AdmissionIssue::EventClass(2)));
        }
        issues.retain(|issue| {
            !matches!(issue,AdmissionIssue::Key(code) if crate::keys::BUTTON_CODES.contains(code))
                && !matches!(issue, AdmissionIssue::Property(0))
        });
        issues.extend(
            self.relative
                .codes()
                .filter(|axis| !crate::relative::SUPPORTED_AXES.contains(axis))
                .map(AdmissionIssue::Relative),
        );
        issues
    }
    /// Conservative keyboard/consumer contract. This does not establish own
    /// device exclusion, output readiness, config compatibility or neutral state.
    pub fn keyboard_issues(&self) -> Vec<AdmissionIssue> {
        let mut issues = Vec::new();
        if !self.events.contains(1) || self.keys.is_empty() {
            issues.push(AdmissionIssue::NoKeys);
        }
        for code in self.events.codes() {
            if ![0, 1, 4, 17, 20].contains(&code) {
                issues.push(AdmissionIssue::EventClass(code));
            }
        }
        if !self.relative.is_empty() && !self.events.contains(crate::relative::EV_REL) {
            issues.push(AdmissionIssue::EventClass(crate::relative::EV_REL));
        }
        let supported: std::collections::BTreeSet<_> = native_codes().collect();
        for code in self.keys.codes() {
            if !supported.contains(&code) {
                issues.push(AdmissionIssue::Key(code));
            }
        }
        for code in self.misc.codes() {
            if ![4, 5].contains(&code) {
                issues.push(AdmissionIssue::Misc(code));
            }
        }
        for code in self.properties.codes() {
            issues.push(AdmissionIssue::Property(code));
        }
        for code in self.leds.codes() {
            if code > 2 {
                issues.push(AdmissionIssue::Led(code));
            }
        }
        issues
    }
}
