//! Scan 1 identities, saved native bindings and semantic KEYBDINPUT packets.
mod table;
use keyweave_types::{KEYBOARD_PAGE, KeyEdge, KeyId, OutputIntent};
pub const MAX_BATCH: usize = 256;
const EXTENDED: u32 = 0x01;
const LOWER_INJECTED: u32 = 0x02;
const INJECTED: u32 = 0x10;
const ALT_DOWN: u32 = 0x20;
const UP: u32 = 0x80;
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("invalid Windows hook record")]
    Record,
    #[error("unsupported scan identity")]
    Native,
    #[error("unsupported HID output")]
    Hid,
    #[error("ambiguous scan identity requires stronger evidence")]
    Ambiguous,
    #[error("key needs a dedicated native sequence")]
    SpecialSequence,
    #[error("injection marker must be nonzero")]
    Marker,
    #[error("native output batch exceeds limit")]
    Capacity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanId {
    code: u8,
    extended: bool,
}
impl ScanId {
    pub fn new(code: u32, extended: bool) -> Result<Self, KeyError> {
        let code = u8::try_from(code)
            .ok()
            .filter(|code| *code != 0)
            .ok_or(KeyError::Record)?;
        Ok(Self { code, extended })
    }
    pub fn code(self) -> u8 {
        self.code
    }
    pub fn extended(self) -> bool {
        self.extended
    }
    fn slot(self) -> usize {
        usize::from(self.code) + 256 * usize::from(self.extended)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InjectionTag(usize);
impl InjectionTag {
    pub fn new(value: usize) -> Result<Self, KeyError> {
        if value == 0 {
            Err(KeyError::Marker)
        } else {
            Ok(Self(value))
        }
    }
    pub fn value(self) -> usize {
        self.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookRecord {
    pub message: u32,
    pub vk_code: u32,
    pub scan_code: u32,
    pub flags: u32,
    pub extra_info: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Physical,
    OwnInjected,
    OtherInjected,
}
impl HookRecord {
    fn validate(self) -> Result<KeyEdge, KeyError> {
        let edge = match self.message {
            0x0100 | 0x0104 => KeyEdge::Down,
            0x0101 | 0x0105 => KeyEdge::Up,
            _ => return Err(KeyError::Record),
        };
        if !(1..=254).contains(&self.vk_code)
            || self.flags & !(EXTENDED | LOWER_INJECTED | INJECTED | ALT_DOWN | UP) != 0
            || (self.flags & UP != 0) != (edge == KeyEdge::Up)
            || self.flags & LOWER_INJECTED != 0 && self.flags & INJECTED == 0
        {
            return Err(KeyError::Record);
        }
        Ok(edge)
    }
    pub fn origin(self, tag: InjectionTag) -> Origin {
        if self.flags & (INJECTED | LOWER_INJECTED) != 0 {
            if self.flags & INJECTED != 0 && self.extra_info == tag.value() {
                Origin::OwnInjected
            } else {
                Origin::OtherInjected
            }
        } else {
            Origin::Physical
        }
    }
    fn scan(self) -> Result<ScanId, KeyError> {
        ScanId::new(self.scan_code, self.flags & EXTENDED != 0)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Mapping {
    pub key: KeyId,
    pub scan: ScanId,
    /// Canonical ordinary scan; special entries only document primary bytes.
    pub canonical: bool,
    pub special: bool,
}
pub fn mappings() -> &'static [Mapping] {
    table::MAPPINGS
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityPolicy {
    /// Scan 1 position evidence, never proof of the originating HID usage.
    CanonicalScan,
    StrictUnique,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedKey {
    /// Canonical Scan 1 identity, including on an unambiguous table lookup.
    pub key: KeyId,
    pub known_aliases: bool,
}
pub fn resolve(record: HookRecord, policy: IdentityPolicy) -> Result<ResolvedKey, KeyError> {
    record.validate()?;
    let scan = record.scan()?;
    if matches!(record.vk_code, 0x03 | 0x13 | 0x2c)
        || scan.code == 0x45
        || !scan.extended && scan.code == 0x54
    {
        return Err(KeyError::SpecialSequence);
    }
    let mapping = mappings()
        .iter()
        .find(|entry| entry.scan == scan && entry.canonical)
        .ok_or(KeyError::Native)?;
    if mapping.special {
        return Err(KeyError::SpecialSequence);
    }
    let known_aliases = mappings().iter().filter(|entry| entry.scan == scan).count() > 1;
    if known_aliases && policy == IdentityPolicy::StrictUnique {
        return Err(KeyError::Ambiguous);
    }
    Ok(ResolvedKey {
        key: mapping.key,
        known_aliases,
    })
}
pub fn output_scan(key: KeyId) -> Result<ScanId, KeyError> {
    if key.page != KEYBOARD_PAGE {
        return Err(KeyError::Hid);
    }
    let mapping = mappings()
        .iter()
        .find(|entry| entry.key == key)
        .ok_or(KeyError::Hid)?;
    if !mapping.canonical {
        return Err(KeyError::Ambiguous);
    }
    if mapping.special {
        return Err(KeyError::SpecialSequence);
    }
    Ok(mapping.scan)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Normalized {
    Injected(Origin),
    OrphanUp,
    Key {
        identity: ResolvedKey,
        edge: KeyEdge,
    },
}
pub struct KeyNormalizer {
    held: [Option<ResolvedKey>; 512],
    policy: IdentityPolicy,
    count: usize,
}
impl KeyNormalizer {
    pub fn new(policy: IdentityPolicy) -> Self {
        Self {
            held: [None; 512],
            policy,
            count: 0,
        }
    }
    pub fn pressed(&self) -> usize {
        self.count
    }
    pub fn reset(&mut self) {
        self.held.fill(None);
        self.count = 0;
    }
    pub fn normalize(
        &mut self,
        record: HookRecord,
        tag: InjectionTag,
    ) -> Result<Normalized, KeyError> {
        // Origin bypass comes first, before scan decoding or touching held state;
        // Unicode and another app's synthetic records never enter our engine.
        let origin = record.origin(tag);
        if origin != Origin::Physical {
            return Ok(Normalized::Injected(origin));
        }
        let edge = record.validate()?;
        let slot = record.scan()?.slot();
        if let Some(identity) = self.held[slot] {
            if edge == KeyEdge::Up {
                self.held[slot] = None;
                self.count -= 1;
            }
            return Ok(Normalized::Key {
                identity,
                edge: if edge == KeyEdge::Down {
                    KeyEdge::Repeat
                } else {
                    edge
                },
            });
        }
        let identity = resolve(record, self.policy)?;
        if edge == KeyEdge::Up {
            return Ok(Normalized::OrphanUp);
        }
        self.held[slot] = Some(identity);
        self.count += 1;
        Ok(Normalized::Key { identity, edge })
    }
}
/// Semantic fields, not a repr(C) ABI struct and not an acceptance receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardPacket {
    pub vk: u16,
    pub scan: u16,
    pub flags: u32,
    pub time: u32,
    pub extra_info: usize,
}
pub fn encode(key: KeyId, edge: KeyEdge, tag: InjectionTag) -> Result<KeyboardPacket, KeyError> {
    let scan = output_scan(key)?;
    Ok(KeyboardPacket {
        vk: 0,
        scan: u16::from(scan.code),
        flags: 8 | u32::from(scan.extended) | if edge == KeyEdge::Up { 2 } else { 0 },
        time: 0,
        extra_info: tag.value(),
    })
}
pub fn encode_batch(
    edges: &[OutputIntent],
    tag: InjectionTag,
) -> Result<Vec<KeyboardPacket>, KeyError> {
    if edges.len() > MAX_BATCH {
        return Err(KeyError::Capacity);
    }
    edges
        .iter()
        .map(|edge| encode(edge.key, edge.edge, tag))
        .collect()
}

#[cfg(test)]
mod tests;
