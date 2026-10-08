//! OS independent event identities. Times are integer monotonic nanoseconds.
mod context;
mod variables;
pub use variables::*;
mod key_names;
pub use context::*;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};
use thiserror::Error;

pub const KEYBOARD_PAGE: u16 = 0x07;
pub const CONSUMER_PAGE: u16 = 0x0c;
pub const BUTTON_PAGE: u16 = 0x09;

/// A physical HID usage, never a layout-dependent character or OS virtual key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId {
    pub page: u16,
    pub usage: u16,
}

#[derive(Debug, Error)]
#[error("unknown HID key name: {0}")]
pub struct UnknownKey(pub String);

impl KeyId {
    pub const fn new(page: u16, usage: u16) -> Self {
        Self { page, usage }
    }

    pub fn named(page: u16, name: &str) -> Result<Self, UnknownKey> {
        key_names::NAMES
            .iter()
            .find(|(p, n, _)| *p == page && *n == name)
            .map(|(p, _, u)| Self::new(*p, *u))
            .ok_or_else(|| UnknownKey(name.to_owned()))
    }

    pub fn name(self) -> String {
        if let Some((_, name, _)) = key_names::NAMES
            .iter()
            .rev()
            .find(|(p, _, u)| self.page == *p && self.usage == *u)
        {
            if self.page == KEYBOARD_PAGE {
                return (*name).to_owned();
            }
            if self.page == CONSUMER_PAGE {
                return format!("consumer:{name}");
            }
        }
        if self.page == BUTTON_PAGE && (1..=32).contains(&self.usage) {
            return format!("button{}", self.usage);
        }
        format!("hid:{:04x}:{:04x}", self.page, self.usage)
    }

    pub fn is_modifier(self) -> bool {
        self.page == KEYBOARD_PAGE && (0xe0..=0xe7).contains(&self.usage)
    }

    pub fn standard_names() -> &'static [(u16, &'static str, u16)] {
        key_names::NAMES
    }
}

impl FromStr for KeyId {
    type Err = UnknownKey;
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        if let Some(name) = name.strip_prefix("consumer:") {
            return Self::named(CONSUMER_PAGE, name);
        }
        if let Some(button) = name.strip_prefix("button")
            && let Ok(usage @ 1..=32) = button.parse::<u16>()
        {
            return Ok(Self::new(BUTTON_PAGE, usage));
        }
        if let Some(native) = name.strip_prefix("hid:")
            && let Some((page, usage)) = native.split_once(':')
            && let (Ok(page), Ok(usage)) = (
                u16::from_str_radix(page, 16),
                u16::from_str_radix(usage, 16),
            )
        {
            return Ok(Self::new(page, usage));
        }
        Self::named(KEYBOARD_PAGE, name)
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}
impl Serialize for KeyId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.name())
    }
}
impl<'de> Deserialize<'de> for KeyId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

macro_rules! identity {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}
identity!(MonoTime);
identity!(SourceToken);
identity!(ConfigGeneration);

impl MonoTime {
    pub fn checked_add(self, duration_ns: u64) -> Option<Self> {
        self.0.checked_add(duration_ns).map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceInstanceId {
    pub id: u64,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    #[default]
    Physical,
    OurVirtual,
    OtherInjected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyEdge {
    Down,
    Up,
    Repeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeadlineToken(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    CancelDelayed { token: DeadlineToken },
    TimerFired { token: DeadlineToken },
    Key { key: KeyId, edge: KeyEdge },
    Wheel { horizontal: i32, vertical: i32 },
    PointerMove { x: i32, y: i32 },
    ContextChanged { context: ContextSnapshot },
    DeviceRemoved,
    Reset,
}

/// `seq` must strictly increase; `at_ns` must never go backwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEnvelope {
    pub seq: u64,
    pub at_ns: MonoTime,
    pub device: DeviceInstanceId,
    #[serde(default)]
    pub provenance: Provenance,
    pub event: InputEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputIntent {
    pub at_ns: MonoTime,
    pub key: KeyId,
    pub edge: KeyEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDecision {
    Capture,
    Pass,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub name: String,
    pub reliable_device_identity: bool,
    pub consumer_keys: bool,
    pub pointing_buttons: bool,
}
impl BackendCapabilities {
    pub fn offline() -> Self {
        Self {
            name: "offline".into(),
            reliable_device_identity: true,
            consumer_keys: true,
            pointing_buttons: true,
        }
    }
    pub fn linux() -> Self {
        Self {
            name: "linux-evdev".into(),
            reliable_device_identity: true,
            consumer_keys: true,
            pointing_buttons: false,
        }
    }
    pub fn windows_hook() -> Self {
        Self {
            name: "windows-hook".into(),
            reliable_device_identity: false,
            consumer_keys: false,
            pointing_buttons: false,
        }
    }
    pub fn supports(&self, key: KeyId) -> bool {
        key.page == KEYBOARD_PAGE
            || (key.page == CONSUMER_PAGE && self.consumer_keys)
            || (key.page == BUTTON_PAGE && self.pointing_buttons)
    }
}
