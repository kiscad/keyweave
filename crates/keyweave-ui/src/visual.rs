//! Explicit JSON draft: preserve unknown values; never edit source files.
use keyweave_types::{BUTTON_PAGE, CONSUMER_PAGE, KEYBOARD_PAGE, KeyId};
use serde_json::{Value, json};
pub const MAX_PROFILES: usize = 128;
pub const MAX_ROWS: usize = 4096;
pub struct VisualDraft {
    source: String,
    pub(crate) raw: Value,
    pub(crate) dirty: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimpleRow {
    pub from: String,
    pub to: String,
}
fn event_key(value: &Value) -> Option<KeyId> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    if let Some(name) = object.get("key_code").and_then(Value::as_str) {
        KeyId::named(KEYBOARD_PAGE, name).ok()
    } else if let Some(name) = object.get("consumer_key_code").and_then(Value::as_str) {
        KeyId::named(CONSUMER_PAGE, name).ok()
    } else {
        object
            .get("pointing_button")
            .and_then(Value::as_str)?
            .parse::<KeyId>()
            .ok()
            .filter(|key| key.page == BUTTON_PAGE)
    }
}
pub(crate) fn event_definition(name: &str) -> Result<Value, String> {
    let key: KeyId = name
        .trim()
        .parse()
        .map_err(|e: keyweave_types::UnknownKey| e.to_string())?;
    let canonical = key.name();
    match key.page {
        KEYBOARD_PAGE
            if KeyId::standard_names()
                .iter()
                .any(|(page, _, usage)| *page == key.page && *usage == key.usage) =>
        {
            Ok(json!({"key_code":canonical}))
        }
        CONSUMER_PAGE if canonical.starts_with("consumer:") => {
            Ok(json!({"consumer_key_code":canonical.trim_start_matches("consumer:")}))
        }
        BUTTON_PAGE if (1..=32).contains(&key.usage) => Ok(json!({"pointing_button":canonical})),
        _ => Err("Choose a named keyboard key, consumer:key or button1..32".into()),
    }
}
pub(crate) fn simple_row(value: &Value) -> Option<SimpleRow> {
    if value.as_object()?.len() != 2 {
        return None;
    }
    let from = event_key(value.get("from")?)?;
    let to = value.get("to")?.as_array()?;
    if to.len() != 1 {
        return None;
    }
    Some(SimpleRow {
        from: from.name(),
        to: event_key(&to[0])?.name(),
    })
}
impl VisualDraft {
    pub fn parse(source: &str) -> Result<Self, String> {
        let raw = keyweave_config::parse_jsonc(source).map_err(|e| e.to_string())?;
        let profiles = raw
            .get("profiles")
            .and_then(Value::as_array)
            .filter(|p| !p.is_empty() && p.len() <= MAX_PROFILES)
            .ok_or("Visual editing needs 1..128 profiles")?;
        for profile in profiles {
            let profile = profile.as_object().ok_or("Profile must be an object")?;
            if profile.get("name").is_some_and(|v| !v.is_string())
                || profile.get("selected").is_some_and(|v| !v.is_boolean())
            {
                return Err("Profile name/selected has an invalid type; edit JSON first".into());
            }
            if let Some(rows) = profile.get("simple_modifications")
                && rows.as_array().is_none_or(|rows| rows.len() > MAX_ROWS)
            {
                return Err("Visual editing needs an array of at most 4096 simple mappings".into());
            }
        }
        crate::rules::validate_profiles(profiles)?;
        crate::devices::validate_profiles(profiles)?;
        Ok(Self {
            source: source.into(),
            raw,
            dirty: false,
        })
    }
    fn profiles(&self) -> &Vec<Value> {
        self.raw["profiles"]
            .as_array()
            .unwrap_or_else(|| unreachable!("validated draft"))
    }
    pub fn len(&self) -> usize {
        self.profiles().len()
    }
    pub fn is_empty(&self) -> bool {
        self.profiles().is_empty()
    }
    pub fn dirty(&self) -> bool {
        self.dirty
    }
    pub fn profile_name(&self, index: usize) -> &str {
        self.profiles()
            .get(index)
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Unnamed profile")
    }
    pub fn active(&self) -> usize {
        self.profiles()
            .iter()
            .position(|p| p.get("selected") == Some(&Value::Bool(true)))
            .unwrap_or(0)
    }
    pub fn select(&mut self, index: usize) -> Result<(), String> {
        if index >= self.len() {
            return Err("Profile no longer exists".into());
        }
        for (i, profile) in self.raw["profiles"]
            .as_array_mut()
            .ok_or("Invalid profiles")?
            .iter_mut()
            .enumerate()
        {
            profile["selected"] = Value::Bool(i == index);
        }
        self.dirty = true;
        Ok(())
    }
    pub fn rename(&mut self, index: usize, name: &str) -> Result<(), String> {
        if index >= self.len() || name.trim().is_empty() || name.chars().count() > 256 {
            return Err("Choose an existing profile and a name of 1..256 characters".into());
        }
        self.raw["profiles"][index]["name"] = Value::String(name.into());
        self.dirty = true;
        Ok(())
    }
    pub fn add_profile(&mut self) -> Result<usize, String> {
        if self.len() >= MAX_PROFILES {
            return Err("Visual profile limit reached".into());
        }
        let index = self.len();
        self.raw["profiles"]
            .as_array_mut()
            .ok_or("Invalid profiles")?
            .push(json!({"name":"New profile","selected":false,"simple_modifications":[]}));
        self.dirty = true;
        Ok(index)
    }
    pub fn row_count(&self, profile: usize) -> usize {
        self.profiles()
            .get(profile)
            .and_then(|p| p.get("simple_modifications"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
    pub fn row(&self, profile: usize, index: usize) -> Option<SimpleRow> {
        let value = self
            .profiles()
            .get(profile)?
            .get("simple_modifications")?
            .as_array()?
            .get(index)?;
        simple_row(value)
    }

    pub fn set_row(
        &mut self,
        profile: usize,
        index: usize,
        from: &str,
        to: &str,
    ) -> Result<(), String> {
        if self.row(profile, index).is_none() {
            return Err("Advanced mapping must be edited as JSON".into());
        }
        let replacement = json!({"from":event_definition(from)?,"to":[event_definition(to)?]});
        self.raw["profiles"][profile]["simple_modifications"][index] = replacement;
        self.dirty = true;
        Ok(())
    }
    pub fn add_row(&mut self, profile: usize, from: &str, to: &str) -> Result<(), String> {
        if profile >= self.len()
            || crate::devices::simple_count(&self.raw["profiles"][profile]) >= MAX_ROWS
        {
            return Err("Profile missing or visual mapping limit reached".into());
        }
        let row = json!({"from":event_definition(from)?,"to":[event_definition(to)?]});
        let value = &mut self.raw["profiles"][profile];
        if value.get("simple_modifications").is_none() {
            value["simple_modifications"] = Value::Array(vec![]);
        }
        value["simple_modifications"]
            .as_array_mut()
            .ok_or("Mappings must be an array")?
            .push(row);
        self.dirty = true;
        Ok(())
    }
    pub fn remove_row(&mut self, profile: usize, index: usize) -> Result<(), String> {
        if index >= self.row_count(profile) {
            return Err("Mapping no longer exists".into());
        }
        self.raw["profiles"][profile]["simple_modifications"]
            .as_array_mut()
            .ok_or("Mappings must be an array")?
            .remove(index);
        self.dirty = true;
        Ok(())
    }
    pub fn edited_json(&self, current: &str) -> Result<String, String> {
        if current != self.source {
            return Err("Text changed since this draft opened; reopen visual editor".into());
        }
        let mut text = serde_json::to_string_pretty(&self.raw).map_err(|e| e.to_string())?;
        text.push('\n');
        if text.len() > keyweave_config::MAX_CONFIG_BYTES {
            return Err("Edited JSON exceeds 8 MiB".into());
        }
        Ok(text)
    }
}
