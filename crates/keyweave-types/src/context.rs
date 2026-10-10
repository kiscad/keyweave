use crate::{DeviceInstanceId, MonoTime};
use serde::{Deserialize, Serialize};

pub const MAX_CONTEXT_DEVICES: usize = 256;
pub const MAX_CONTEXT_LIFETIME_NS: u64 = 86_400_000_000_000;

/// None means unknown, including booleans; providers must report known false explicitly.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceProperties {
    pub vendor_id: Option<u32>,
    pub product_id: Option<u32>,
    pub location_id: Option<i32>,
    pub device_address: Option<String>,
    pub is_keyboard: Option<bool>,
    pub is_pointing_device: Option<bool>,
    pub is_game_pad: Option<bool>,
    pub is_consumer: Option<bool>,
    pub is_built_in_keyboard: Option<bool>,
}
impl DeviceProperties {
    pub fn has_constraints(&self) -> bool {
        self.vendor_id.is_some()
            || self.product_id.is_some()
            || self.location_id.is_some()
            || self.device_address.is_some()
            || self.is_keyboard.is_some()
            || self.is_pointing_device.is_some()
            || self.is_game_pad.is_some()
            || self.is_consumer.is_some()
            || self.is_built_in_keyboard.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceContext {
    pub device: DeviceInstanceId,
    pub properties: DeviceProperties,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApplicationContext {
    pub bundle_identifier: Option<String>,
    pub file_path: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputSourceContext {
    pub language: Option<String>,
    pub input_source_id: Option<String>,
    pub input_mode_id: Option<String>,
}

/// Complete is an explicit provider claim, never inferred from a partial list.
/// Legacy context snapshots retain the original complete-inventory contract.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceInventoryScope {
    #[default]
    Complete,
    Partial,
}
impl DeviceInventoryScope {
    pub fn is_complete(&self) -> bool {
        *self == Self::Complete
    }
}

/// Effective on [observed_at_ns, valid_until_ns). No OS lookup occurs in the engine.
/// devices=None means unavailable inventory. Some([]) proves empty only for Complete.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSnapshot {
    pub revision: u64,
    pub provider: String,
    pub observed_at_ns: MonoTime,
    pub valid_until_ns: MonoTime,
    pub devices: Option<Vec<DeviceContext>>,
    #[serde(default, skip_serializing_if = "DeviceInventoryScope::is_complete")]
    pub device_inventory_scope: DeviceInventoryScope,
    #[serde(default)]
    pub application: Option<ApplicationContext>,
    #[serde(default)]
    pub input_source: Option<InputSourceContext>,
}
impl ContextSnapshot {
    pub fn fresh(&self, now: MonoTime) -> bool {
        self.revision != 0 && self.observed_at_ns <= now && now < self.valid_until_ns
    }
}
