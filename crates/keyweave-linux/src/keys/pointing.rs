//! Generic evdev mouse-button numbering; canonical is not descriptor verification.
use super::*;
use std::collections::BTreeSet;
pub const BTN_LEFT: u16 = 0x110;
pub const BUTTON_CODES: [u16; 8] = [0x110, 0x111, 0x112, 0x113, 0x114, 0x115, 0x116, 0x117];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointingButtons {
    codes: BTreeSet<u16>,
}
impl PointingButtons {
    pub fn new(codes: &[u16]) -> Result<Self, KeyError> {
        if codes.is_empty()
            || codes.len() > BUTTON_CODES.len()
            || codes.iter().any(|code| !BUTTON_CODES.contains(code))
        {
            return Err(KeyError::UnsupportedNative);
        }
        let unique: BTreeSet<_> = codes.iter().copied().collect();
        if unique.len() != codes.len() {
            return Err(KeyError::UnsupportedNative);
        }
        Ok(Self { codes: unique })
    }
    pub fn codes(&self) -> impl Iterator<Item = u16> + '_ {
        self.codes.iter().copied()
    }
    pub fn contains(&self, code: u16) -> bool {
        self.codes.contains(&code)
    }
}
pub fn resolve_pointing_input(
    code: u16,
    hid_scan: Option<KeyId>,
    policy: IdentityPolicy,
) -> Result<ResolvedKey, KeyError> {
    if !BUTTON_CODES.contains(&code) {
        return resolve_input(code, hid_scan, policy);
    }
    let key = KeyId::new(keyweave_types::BUTTON_PAGE, code - BTN_LEFT + 1);
    if let Some(scan) = hid_scan {
        if scan != key {
            return Err(KeyError::ScanMismatch);
        }
        return Ok(ResolvedKey {
            key,
            evidence: IdentityEvidence::VerifiedHidScan,
            has_known_aliases: false,
        });
    }
    if policy == IdentityPolicy::RequireHidScan {
        return Err(KeyError::ScanRequired);
    }
    Ok(ResolvedKey {
        key,
        evidence: IdentityEvidence::EvdevCanonical,
        has_known_aliases: false,
    })
}
pub fn pointing_output_code(key: KeyId) -> Result<u16, KeyError> {
    if key.page == keyweave_types::BUTTON_PAGE {
        if !(1..=8).contains(&key.usage) {
            return Err(KeyError::UnsupportedHid);
        }
        return Ok(BTN_LEFT + key.usage - 1);
    }
    output_code(key)
}
