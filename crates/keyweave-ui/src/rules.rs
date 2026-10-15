//! Lossless rule drafts and bounded, non-executing Karabiner asset parsing.
use crate::visual::VisualDraft;
use serde_json::{Value, json};
pub const MAX_COMPLEX_RULES: usize = 4096;
fn rule_shape(rule: &Value) -> Result<(), String> {
    let object = rule.as_object().ok_or("Rule must be an object")?;
    if object.get("description").is_some_and(|v| !v.is_string())
        || object.get("enabled").is_some_and(|v| !v.is_boolean())
    {
        return Err("Rule description/enabled has an invalid type".into());
    }
    // Unsupported fields are retained for the shared compiler to diagnose.
    // eval_js is data here; it is never evaluated, including disabled rules.
    if object.contains_key("eval_js") {
        return Ok(());
    }
    let array = object
        .get("manipulators")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or("Rule needs a nonempty manipulators array or eval_js data")?;
    if array.iter().any(|v| !v.is_object()) {
        return Err("Manipulator must be an object".into());
    }
    Ok(())
}
pub(crate) fn validate_profiles(profiles: &[Value]) -> Result<(), String> {
    for profile in profiles {
        if let Some(complex) = profile.get("complex_modifications") {
            let complex = complex
                .as_object()
                .ok_or("Complex modifications must be an object")?;
            if let Some(rules) = complex.get("rules") {
                let rules = rules
                    .as_array()
                    .filter(|r| r.len() <= MAX_COMPLEX_RULES)
                    .ok_or("Visual editing needs at most 4096 complex rules")?;
                // Existing malformed rules remain editable as JSON; we do not discard them.
                if rules.iter().any(|r| !r.is_object()) {
                    return Err("Complex rules must be objects; edit JSON first".into());
                }
            }
        }
    }
    Ok(())
}
pub struct RuleAsset {
    raw: Value,
    single: bool,
}
impl RuleAsset {
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw = keyweave_config::parse_jsonc(text).map_err(|e| e.to_string())?;
        let object = raw.as_object().ok_or("Rule asset must be an object")?;
        let single = !object.contains_key("title");
        let asset = Self { raw, single };
        if !single {
            if !asset.raw["title"].is_string() {
                return Err("Asset title must be a string".into());
            }
            let rules = asset
                .raw
                .get("rules")
                .and_then(Value::as_array)
                .filter(|r| !r.is_empty() && r.len() <= MAX_COMPLEX_RULES)
                .ok_or("Asset needs 1..4096 rules")?;
            for rule in rules {
                rule_shape(rule)?;
            }
        } else {
            rule_shape(&asset.raw)?;
        }
        Ok(asset)
    }
    pub fn title(&self) -> &str {
        self.raw
            .get(if self.single { "description" } else { "title" })
            .and_then(Value::as_str)
            .unwrap_or("Unnamed rule")
    }
    pub fn rules(&self) -> &[Value] {
        if self.single {
            std::slice::from_ref(&self.raw)
        } else {
            self.raw["rules"].as_array().map_or(&[], Vec::as_slice)
        }
    }
    /// Original wrapper retained for inspection; only selected rule objects are appended.
    pub fn raw(&self) -> &Value {
        &self.raw
    }
}
impl VisualDraft {
    pub fn rules(&self, profile: usize) -> &[Value] {
        self.raw["profiles"]
            .get(profile)
            .and_then(|p| p.get("complex_modifications"))
            .and_then(|c| c.get("rules"))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }
    fn rules_mut(&mut self, profile: usize) -> Result<&mut Vec<Value>, String> {
        let profile = self.raw["profiles"]
            .get_mut(profile)
            .ok_or("Profile no longer exists")?;
        if profile.get("complex_modifications").is_none() {
            profile["complex_modifications"] = json!({"rules":[]});
        }
        let complex = &mut profile["complex_modifications"];
        if complex.get("rules").is_none() {
            complex["rules"] = json!([]);
        }
        complex["rules"]
            .as_array_mut()
            .ok_or_else(|| "Complex rules must be an array".into())
    }
    pub fn replace_rule(
        &mut self,
        profile: usize,
        index: usize,
        expected: &Value,
        text: &str,
    ) -> Result<(), String> {
        if self.rules(profile).get(index) != Some(expected) {
            return Err("Rule changed since editor opened; reopen it".into());
        }
        let rule = keyweave_config::parse_jsonc(text).map_err(|e| e.to_string())?;
        rule_shape(&rule)?;
        self.rules_mut(profile)?[index] = rule;
        self.dirty = true;
        Ok(())
    }
    pub fn set_rule_enabled(
        &mut self,
        profile: usize,
        index: usize,
        enabled: bool,
    ) -> Result<(), String> {
        if self.rules(profile).get(index).is_none() {
            return Err("Rule no longer exists".into());
        }
        self.rules_mut(profile)?[index]["enabled"] = Value::Bool(enabled);
        self.dirty = true;
        Ok(())
    }
    pub fn move_rule(&mut self, profile: usize, from: usize, to: usize) -> Result<(), String> {
        let len = self.rules(profile).len();
        if from >= len || to >= len {
            return Err("Rule position out of range".into());
        }
        let rules = self.rules_mut(profile)?;
        let rule = rules.remove(from);
        rules.insert(to, rule);
        self.dirty = true;
        Ok(())
    }
    pub fn remove_rule(&mut self, profile: usize, index: usize) -> Result<(), String> {
        if index >= self.rules(profile).len() {
            return Err("Rule no longer exists".into());
        }
        self.rules_mut(profile)?.remove(index);
        self.dirty = true;
        Ok(())
    }
    pub fn import_rules(
        &mut self,
        profile: usize,
        asset: &RuleAsset,
        selected: &[bool],
    ) -> Result<usize, String> {
        if profile >= self.len() || selected.len() != asset.rules().len() {
            return Err("Profile or asset selection changed".into());
        }
        let count = selected.iter().filter(|s| **s).count();
        if count == 0 || self.rules(profile).len().saturating_add(count) > MAX_COMPLEX_RULES {
            return Err("Select rules within the 4096 rule limit".into());
        }
        // All shape/selection checks precede mutation; no unsupported field is stripped.
        let copied = asset
            .rules()
            .iter()
            .zip(selected)
            .filter(|(_, s)| **s)
            .map(|(r, _)| r.clone())
            .collect::<Vec<_>>();
        self.rules_mut(profile)?.extend(copied);
        self.dirty = true;
        Ok(count)
    }
}
