//! Physical evdev key identities and explicit HID-loss boundaries.
mod table;
use crate::events::{EV_KEY, Event};
use keyweave_types::{KeyEdge, KeyId};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub key: KeyId,
    pub code: u16,
    pub canonical: bool,
}
pub fn mappings() -> &'static [Mapping] {
    table::MAPPINGS
}
pub fn native_codes() -> impl Iterator<Item = u16> {
    mappings()
        .iter()
        .filter(|mapping| mapping.canonical)
        .map(|mapping| mapping.code)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityPolicy {
    CanonicalEvdev,
    RequireHidScan,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityEvidence {
    EvdevCanonical,
    VerifiedHidScan,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedKey {
    pub key: KeyId,
    pub evidence: IdentityEvidence,
    pub has_known_aliases: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum KeyError {
    #[error("native key code has no supported HID mapping")]
    UnsupportedNative,
    #[error("HID key has no supported native mapping")]
    UnsupportedHid,
    #[error("HID output aliases another key at the evdev boundary")]
    OutputAlias,
    #[error("verified HID scan is required by identity policy")]
    ScanRequired,
    #[error("HID scan and native key code disagree")]
    ScanMismatch,
    #[error("normalizer requires EV_KEY with value 0, 1 or 2")]
    InvalidEvent,
}
/// A scan hint must come from a verified HID driver/descriptor path. Do not
/// interpret arbitrary PS/2 scan values or vendor MSC records as USB HID usages.
pub fn resolve_input(
    code: u16,
    hid_scan: Option<KeyId>,
    policy: IdentityPolicy,
) -> Result<ResolvedKey, KeyError> {
    let mapping = mappings()
        .iter()
        .find(|mapping| mapping.code == code && mapping.canonical)
        .ok_or(KeyError::UnsupportedNative)?;
    let has_known_aliases = mappings()
        .iter()
        .filter(|mapping| mapping.code == code)
        .count()
        > 1;
    if let Some(key) = hid_scan {
        if !mappings()
            .iter()
            .any(|candidate| candidate.code == code && candidate.key == key)
        {
            return Err(KeyError::ScanMismatch);
        }
        return Ok(ResolvedKey {
            key,
            evidence: IdentityEvidence::VerifiedHidScan,
            has_known_aliases,
        });
    }
    if policy == IdentityPolicy::RequireHidScan {
        return Err(KeyError::ScanRequired);
    }
    Ok(ResolvedKey {
        key: mapping.key,
        evidence: IdentityEvidence::EvdevCanonical,
        has_known_aliases,
    })
}
pub fn output_code(key: KeyId) -> Result<u16, KeyError> {
    let mapping = mappings()
        .iter()
        .find(|mapping| mapping.key == key)
        .ok_or(KeyError::UnsupportedHid)?;
    if !mapping.canonical {
        return Err(KeyError::OutputAlias);
    }
    Ok(mapping.code)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalizedKey {
    Event {
        identity: ResolvedKey,
        edge: KeyEdge,
    },
    /// Orphan up/repeat has no saved down; never infer another source's identity.
    Ignored { code: u16, edge: KeyEdge },
}
/// One instance per device generation. Saved native-code bindings outlive scan
/// changes, configuration reload and missing scan records on up/repeat.
pub struct KeyNormalizer {
    policy: IdentityPolicy,
    pressed: BTreeMap<u16, ResolvedKey>,
}
impl KeyNormalizer {
    pub fn new(policy: IdentityPolicy) -> Self {
        Self {
            policy,
            pressed: BTreeMap::new(),
        }
    }
    pub fn pressed_count(&self) -> usize {
        self.pressed.len()
    }
    pub fn process(
        &mut self,
        event: Event,
        hid_scan: Option<KeyId>,
    ) -> Result<NormalizedKey, KeyError> {
        if event.kind != EV_KEY {
            return Err(KeyError::InvalidEvent);
        }
        let edge = match event.value {
            0 => KeyEdge::Up,
            1 => KeyEdge::Down,
            2 => KeyEdge::Repeat,
            _ => return Err(KeyError::InvalidEvent),
        };
        match edge {
            KeyEdge::Down => {
                let identity = if let Some(saved) = self.pressed.get(&event.code) {
                    *saved
                } else {
                    let identity = resolve_input(event.code, hid_scan, self.policy)?;
                    self.pressed.insert(event.code, identity);
                    identity
                };
                Ok(NormalizedKey::Event { identity, edge })
            }
            KeyEdge::Up => Ok(self.pressed.remove(&event.code).map_or(
                NormalizedKey::Ignored {
                    code: event.code,
                    edge,
                },
                |identity| NormalizedKey::Event { identity, edge },
            )),
            KeyEdge::Repeat => Ok(self.pressed.get(&event.code).copied().map_or(
                NormalizedKey::Ignored {
                    code: event.code,
                    edge,
                },
                |identity| NormalizedKey::Event { identity, edge },
            )),
        }
    }
    /// Lifecycle caller removes original engine bindings separately. No fabricated up.
    pub fn reset(&mut self) {
        self.pressed.clear();
    }
}
