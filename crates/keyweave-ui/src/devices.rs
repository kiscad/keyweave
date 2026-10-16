//! Transactional configured-device drafts, never a hardware inventory.
use crate::visual::{MAX_ROWS, VisualDraft, event_definition, simple_row};
use serde_json::{Value, json};
pub const MAX_DEVICES: usize = 256;
pub(crate) fn simple_count(profile: &Value) -> usize {
    let global = profile
        .get("simple_modifications")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    global
        + profile
            .get("devices")
            .and_then(Value::as_array)
            .map_or(0, |devices| {
                devices
                    .iter()
                    .map(|d| {
                        d.get("simple_modifications")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len)
                    })
                    .sum::<usize>()
            })
}
pub(crate) fn validate_profiles(profiles: &[Value]) -> Result<(), String> {
    for profile in profiles {
        if let Some(value) = profile.get("devices") {
            let devices = value
                .as_array()
                .filter(|a| a.len() <= MAX_DEVICES)
                .ok_or("Visual devices need an array of at most 256 entries")?;
            for device in devices {
                let object = device
                    .as_object()
                    .ok_or("Device entries must be objects; edit JSON first")?;
                if let Some(rows) = object.get("simple_modifications")
                    && rows.as_array().is_none_or(|a| a.len() > MAX_ROWS)
                {
                    return Err(
                        "Device simple mappings must be an array of at most 4096 entries".into(),
                    );
                }
            }
        }
        if simple_count(profile) > MAX_ROWS {
            return Err(
                "Visual profile needs at most 4096 global and device simple mappings combined"
                    .into(),
            );
        }
    }
    Ok(())
}
fn id(text: &str) -> Result<u32, String> {
    let text = text.trim();
    let result = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Use a decimal or 0x hexadecimal u32 device ID".into());
        }
        u32::from_str_radix(hex, 16)
    } else {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Use a decimal or 0x hexadecimal u32 device ID".into());
        }
        text.parse()
    };
    result.map_err(|_| "Device ID exceeds u32".into())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    pub vendor: String,
    pub product: String,
    pub keyboard: bool,
    pub pointing: bool,
}
fn selector(value: &Value) -> Result<Selector, String> {
    let object = value
        .get("identifiers")
        .and_then(Value::as_object)
        .ok_or("Device identifiers need a JSON object; edit JSON first")?;
    let number = |name| match object.get(name) {
        None => Ok("0".to_owned()),
        Some(v) => v
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .map(|v| v.to_string())
            .ok_or_else(|| "Invalid device ID; edit JSON first".to_owned()),
    };
    let flag = |name| match object.get(name) {
        None => Ok(false),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| "Invalid device type flag; edit JSON first".to_owned()),
    };
    Ok(Selector {
        vendor: number("vendor_id")?,
        product: number("product_id")?,
        keyboard: flag("is_keyboard")?,
        pointing: flag("is_pointing_device")?,
    })
}
#[derive(Clone)]
pub struct DeviceRow {
    pub from: String,
    pub to: String,
    original: Option<(Value, String, String)>,
}
pub enum Row {
    Editable(DeviceRow),
    Advanced(Value),
}
impl Row {
    fn value(&self) -> Result<Value, String> {
        match self {
            Self::Advanced(value) => Ok(value.clone()),
            Self::Editable(row) => {
                if let Some((raw, from, to)) = &row.original
                    && from == &row.from
                    && to == &row.to
                {
                    return Ok(raw.clone());
                }
                Ok(json!({"from":event_definition(&row.from)?,"to":[event_definition(&row.to)?]}))
            }
        }
    }
    fn changed(&self) -> bool {
        match self {
            Self::Advanced(_) => false,
            Self::Editable(row) => row
                .original
                .as_ref()
                .is_none_or(|(_, from, to)| from != &row.from || to != &row.to),
        }
    }
}
pub struct DeviceEdit {
    profile: usize,
    index: usize,
    expected: Option<Value>,
    raw: Value,
    baseline: Selector,
    pub selector: Selector,
    pub rows: Vec<Row>,
    initial_rows: Vec<Value>,
}
impl VisualDraft {
    pub fn device_count(&self, profile: usize) -> usize {
        self.raw["profiles"]
            .get(profile)
            .and_then(|p| p.get("devices"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
    pub fn device(&self, profile: usize, index: usize) -> Option<&Value> {
        self.raw["profiles"]
            .get(profile)?
            .get("devices")?
            .as_array()?
            .get(index)
    }
    pub fn delete_device(
        &mut self,
        profile: usize,
        index: usize,
        expected: &Value,
    ) -> Result<(), String> {
        if self.device(profile, index) != Some(expected) {
            return Err("Device changed since deletion was selected; reopen confirmation".into());
        }
        self.raw["profiles"][profile]["devices"]
            .as_array_mut()
            .ok_or("Devices must be an array")?
            .remove(index);
        self.dirty = true;
        Ok(())
    }
}
impl DeviceEdit {
    pub fn open(draft: &VisualDraft, profile: usize, index: Option<usize>) -> Result<Self, String> {
        if profile >= draft.len() {
            return Err("Profile no longer exists".into());
        }
        let (index, expected, raw) = if let Some(index) = index {
            let value = draft
                .device(profile, index)
                .ok_or("Device no longer exists")?
                .clone();
            (index, Some(value.clone()), value)
        } else {
            let index = draft.device_count(profile);
            if index >= MAX_DEVICES {
                return Err("Visual device limit reached".into());
            }
            (
                index,
                None,
                json!({"identifiers":{"vendor_id":0,"product_id":0,"is_keyboard":false,"is_pointing_device":false},"simple_modifications":[]}),
            )
        };
        let selector = selector(&raw)?;
        let initial_rows = raw
            .get("simple_modifications")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let rows = initial_rows
            .iter()
            .map(|value| match simple_row(value) {
                Some(row) => Row::Editable(DeviceRow {
                    original: Some((value.clone(), row.from.clone(), row.to.clone())),
                    from: row.from,
                    to: row.to,
                }),
                None => Row::Advanced(value.clone()),
            })
            .collect();
        Ok(Self {
            profile,
            index,
            expected,
            raw,
            baseline: selector.clone(),
            selector,
            rows,
            initial_rows,
        })
    }
    pub fn pending(&self) -> bool {
        self.expected.is_none()
            || self.selector != self.baseline
            || self.rows.len() != self.initial_rows.len()
            || self.rows.iter().any(Row::changed)
            || self
                .rows
                .iter()
                .zip(&self.initial_rows)
                .any(|(r, v)| match r {
                    Row::Advanced(raw) => raw != v,
                    Row::Editable(r) => r.original.as_ref().is_none_or(|(raw, _, _)| raw != v),
                })
    }
    pub fn add_row(&mut self) -> Result<(), String> {
        if self.rows.len() >= MAX_ROWS {
            return Err("Device mapping limit reached".into());
        }
        self.rows.push(Row::Editable(DeviceRow {
            from: "a".into(),
            to: "b".into(),
            original: None,
        }));
        Ok(())
    }
    pub fn remove_row(&mut self, index: usize) -> Result<(), String> {
        if !matches!(self.rows.get(index), Some(Row::Editable(_))) {
            return Err("Advanced mapping must be edited as JSON".into());
        }
        self.rows.remove(index);
        Ok(())
    }
    pub fn commit(&self, draft: &mut VisualDraft) -> Result<(), String> {
        if self.profile >= draft.len()
            || match &self.expected {
                Some(expected) => draft.device(self.profile, self.index) != Some(expected),
                None => draft.device_count(self.profile) != self.index,
            }
        {
            return Err("Device changed since this editor opened; reopen device editor".into());
        }
        let mut value = self.raw.clone();
        if self.selector != self.baseline {
            let vendor = id(&self.selector.vendor)?;
            let product = id(&self.selector.product)?;
            let identifiers = value["identifiers"]
                .as_object_mut()
                .ok_or("Identifiers must be an object")?;
            identifiers.insert("vendor_id".into(), json!(vendor));
            identifiers.insert("product_id".into(), json!(product));
            identifiers.insert("is_keyboard".into(), json!(self.selector.keyboard));
            identifiers.insert("is_pointing_device".into(), json!(self.selector.pointing));
        }
        let rows = self
            .rows
            .iter()
            .map(Row::value)
            .collect::<Result<Vec<_>, _>>()?;
        let old_count = self
            .expected
            .as_ref()
            .and_then(|v| v.get("simple_modifications"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let current = simple_count(&draft.raw["profiles"][self.profile]);
        if current.saturating_sub(old_count).saturating_add(rows.len()) > MAX_ROWS {
            return Err("Profile global and device simple mapping budget reached".into());
        }
        if self.raw.get("simple_modifications").is_some() || !rows.is_empty() {
            value["simple_modifications"] = Value::Array(rows);
        }
        if self.expected.as_ref() == Some(&value) {
            return Ok(());
        }
        if self.expected.is_some() {
            draft.raw["profiles"][self.profile]["devices"][self.index] = value;
        } else {
            let profile = &mut draft.raw["profiles"][self.profile];
            if profile.get("devices").is_none() {
                profile["devices"] = json!([]);
            }
            let devices = profile["devices"]
                .as_array_mut()
                .ok_or("Devices must be an array")?;
            if devices.len() >= MAX_DEVICES {
                return Err("Visual device limit reached".into());
            }
            devices.push(value);
        }
        draft.dirty = true;
        Ok(())
    }
}
