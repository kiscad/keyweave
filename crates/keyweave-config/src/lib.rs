//! Bounded JSONC parsing and fail-closed compilation. No script or shell evaluation.
mod unique_json;
use keyweave_engine::{Action, ConfigSnapshot, Mapping, MappingExtras, Modifier, ModifierFilter};
use keyweave_types::*;
use serde::Serialize;
use serde_json::{Map, Value};
use thiserror::Error;

pub const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;
const MAX_RULES: usize = 4096;
const MAX_ACTIONS: usize = 256;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub pointer: String,
    pub message: String,
}
#[derive(Debug)]
pub struct CompiledConfig {
    pub raw: Value,
    pub snapshot: ConfigSnapshot,
    pub diagnostics: Vec<Diagnostic>,
    pub disabled_rules: usize,
}
impl CompiledConfig {
    pub fn executable_without_loss(&self) -> bool {
        self.disabled_rules == 0
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration exceeds {MAX_CONFIG_BYTES} bytes")]
    TooLarge,
    #[error("JSONC comment is unterminated")]
    UnterminatedComment,
    #[error("JSONC: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{pointer}: {message}")]
    Structure { pointer: String, message: String },
}
fn structure(pointer: &str, message: impl Into<String>) -> ConfigError {
    ConfigError::Structure {
        pointer: pointer.into(),
        message: message.into(),
    }
}

/// Replace comments with spaces without altering byte offsets or newline positions.
/// Strings, escape sequences and UTF-8 are kept byte-for-byte. Trailing commas remain invalid.
pub fn parse_jsonc(text: &str) -> Result<Value, ConfigError> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge);
    }
    let input = text.as_bytes();
    let mut output = input.to_vec();
    let mut index = 0;
    let mut in_string = false;
    while index < input.len() {
        if in_string {
            if input[index] == b'\\' {
                index += 2;
                continue;
            }
            if input[index] == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if input[index] == b'"' {
            in_string = true;
            index += 1;
            continue;
        }
        if input[index..].starts_with(b"//") {
            while index < input.len() && input[index] != b'\n' && input[index] != b'\r' {
                output[index] = b' ';
                index += 1;
            }
        } else if input[index..].starts_with(b"/*") {
            output[index] = b' ';
            output[index + 1] = b' ';
            index += 2;
            loop {
                if index + 1 >= input.len() {
                    return Err(ConfigError::UnterminatedComment);
                }
                if input[index..].starts_with(b"*/") {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 2;
                    break;
                }
                if input[index] != b'\n' && input[index] != b'\r' {
                    output[index] = b' ';
                }
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    Ok(serde_json::from_slice::<unique_json::UniqueValue>(&output)?.0)
}

pub fn compile(
    text: &str,
    capabilities: &BackendCapabilities,
    generation: ConfigGeneration,
) -> Result<CompiledConfig, ConfigError> {
    let raw = parse_jsonc(text)?;
    let root = raw
        .as_object()
        .ok_or_else(|| structure("", "expected object"))?;
    if let Some(extension) = root.get("keyweave") {
        let extension = extension
            .as_object()
            .ok_or_else(|| structure("/keyweave", "expected object"))?;
        if extension.get("schema_version").and_then(Value::as_u64) != Some(1) {
            return Err(structure(
                "/keyweave/schema_version",
                "only schema version 1 is supported",
            ));
        }
        if extension.keys().any(|name| name != "schema_version") {
            return Err(structure("/keyweave", "unknown executable extension"));
        }
    }
    let profiles = root
        .get("profiles")
        .and_then(Value::as_array)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| structure("/profiles", "expected non-empty profiles array"))?;
    for (index, candidate) in profiles.iter().enumerate() {
        let location = format!("/profiles/{index}");
        let object = candidate
            .as_object()
            .ok_or_else(|| structure(&location, "expected profile object"))?;
        if object
            .get("selected")
            .is_some_and(|value| !value.is_boolean())
        {
            return Err(structure(
                &format!("{location}/selected"),
                "expected boolean",
            ));
        }
        if object.get("name").is_some_and(|value| !value.is_string()) {
            return Err(structure(&format!("{location}/name"), "expected string"));
        }
    }
    let selected: Vec<_> = profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| p.get("selected") == Some(&Value::Bool(true)))
        .map(|(i, _)| i)
        .collect();
    if selected.len() > 1 {
        return Err(structure("/profiles", "multiple profiles selected"));
    }
    let selected = selected.first().copied().unwrap_or(0);
    let pointer = format!("/profiles/{selected}");
    let profile = profiles[selected]
        .as_object()
        .ok_or_else(|| structure(&pointer, "expected profile object"))?;
    if profile.get("selected").is_some_and(|v| !v.is_boolean()) {
        return Err(structure(
            &format!("{pointer}/selected"),
            "expected boolean",
        ));
    }
    let name = profile
        .get("name")
        .map(|v| {
            v.as_str()
                .ok_or_else(|| structure(&format!("{pointer}/name"), "expected string"))
        })
        .transpose()?
        .unwrap_or("Unnamed")
        .to_owned();
    let mut compiler = Compiler {
        capabilities,
        diagnostics: Vec::new(),
        disabled: 0,
        count: 0,
        alone_timeout_ns: 1_000_000_000,
    };
    compiler.reject_unknown_fields(root, "", &["profiles", "keyweave"]);
    compiler.reject_unknown_fields(
        profile,
        &pointer,
        &[
            "name",
            "selected",
            "simple_modifications",
            "complex_modifications",
            "devices",
            "fn_function_keys",
            "parameters",
            "virtual_hid_keyboard",
            "ignore_pointing_device_events_by_default",
        ],
    );
    for field in ["parameters", "virtual_hid_keyboard"] {
        if let Some(value) = profile.get(field) {
            let object = value
                .as_object()
                .ok_or_else(|| structure(&format!("{pointer}/{field}"), "expected object"))?;
            for name in object.keys() {
                compiler.reject(
                    &format!("{pointer}/{field}/{}", escape_pointer(name)),
                    "KW-CAP-001",
                    "profile hardware/output setting is not implemented",
                );
            }
        }
    }
    if let Some(value) = profile.get("ignore_pointing_device_events_by_default") {
        if !value.is_boolean() {
            return Err(structure(
                &format!("{pointer}/ignore_pointing_device_events_by_default"),
                "expected boolean",
            ));
        }
        compiler.reject(
            &format!("{pointer}/ignore_pointing_device_events_by_default"),
            "KW-CAP-001",
            "pointing-device capture policy is not implemented",
        );
    }
    let mut simple = Vec::new();
    if let Some(value) = profile.get("simple_modifications") {
        for (index, item) in array(value, &format!("{pointer}/simple_modifications"))?
            .iter()
            .enumerate()
        {
            let location = format!("{pointer}/simple_modifications/{index}");
            compiler.count_rule(&location)?;
            match compiler.mapping(item, &location, false) {
                Ok(mapping) => simple.push(mapping),
                Err(diagnostic) => {
                    compiler.diagnostics.push(diagnostic);
                    compiler.disabled += 1;
                }
            }
        }
    }
    for field in ["devices", "fn_function_keys"] {
        if let Some(value) = profile.get(field)
            && !array(value, &format!("{pointer}/{field}"))?.is_empty()
        {
            compiler.reject(&format!("{pointer}/{field}"), "KW-CAP-001", "device/Fn mapping is not implemented in this build; profile cannot be activated without loss");
        }
    }
    let mut complex = Vec::new();
    if let Some(value) = profile.get("complex_modifications") {
        let object = value.as_object().ok_or_else(|| {
            structure(
                &format!("{pointer}/complex_modifications"),
                "expected object",
            )
        })?;
        compiler.reject_unknown_fields(
            object,
            &format!("{pointer}/complex_modifications"),
            &["rules", "parameters"],
        );
        if object.get("parameters").is_some_and(|v| !v.is_object()) {
            return Err(structure(
                &format!("{pointer}/complex_modifications/parameters"),
                "expected object",
            ));
        }
        match alone_timeout(
            object.get("parameters"),
            &format!("{pointer}/complex_modifications/parameters"),
        ) {
            Ok(Some(timeout)) => compiler.alone_timeout_ns = timeout,
            Ok(None) => {}
            Err(diagnostic) => {
                compiler.diagnostics.push(diagnostic);
                compiler.disabled += 1;
            }
        }
        if let Some(rules) = object.get("rules") {
            for (index, rule) in array(rules, &format!("{pointer}/complex_modifications/rules"))?
                .iter()
                .enumerate()
            {
                let location = format!("{pointer}/complex_modifications/rules/{index}");
                let rule = rule
                    .as_object()
                    .ok_or_else(|| structure(&location, "expected rule object"))?;
                if rule.get("enabled").is_some_and(|v| !v.is_boolean()) {
                    return Err(structure(
                        &format!("{location}/enabled"),
                        "expected boolean",
                    ));
                }
                if rule.get("enabled") == Some(&Value::Bool(false)) {
                    continue;
                }
                if rule
                    .get("description")
                    .is_some_and(|value| !value.is_string())
                {
                    return Err(structure(
                        &format!("{location}/description"),
                        "expected string",
                    ));
                }
                let disabled_before = compiler.disabled;
                compiler.reject_unknown_fields(
                    rule,
                    &location,
                    &["enabled", "description", "manipulators", "eval_js"],
                );
                if compiler.disabled != disabled_before {
                    continue;
                }
                if rule.contains_key("eval_js") {
                    compiler.reject(&format!("{location}/eval_js"), "KW-CAP-002", "JavaScript rule generation is not implemented; rule disabled without execution");
                    continue;
                }
                let manipulators = rule
                    .get("manipulators")
                    .ok_or_else(|| structure(&location, "missing manipulators"))?;
                let manipulators = array(manipulators, &format!("{location}/manipulators"))?;
                if manipulators.is_empty() {
                    return Err(structure(&location, "manipulators cannot be empty"));
                }
                let mut compiled = Vec::new();
                let mut failed = false;
                for (mi, manipulator) in manipulators.iter().enumerate() {
                    let location = format!("{location}/manipulators/{mi}");
                    compiler.count_rule(&location)?;
                    match compiler.mapping(manipulator, &location, true) {
                        Ok(mapping) => compiled.push(mapping),
                        Err(diagnostic) => {
                            compiler.diagnostics.push(diagnostic);
                            failed = true;
                        }
                    }
                }
                if failed {
                    compiler.disabled += 1;
                } else {
                    complex.extend(compiled);
                }
            }
        }
    }
    let snapshot = ConfigSnapshot {
        generation,
        name,
        simple,
        complex,
    };
    let bounds = snapshot.expansion_bounds();
    for mapping in snapshot.simple.iter().chain(&snapshot.complex) {
        if bounds
            .get(&mapping.from)
            .copied()
            .unwrap_or(1)
            .saturating_mul(2)
            > keyweave_engine::MAX_OUTPUTS_PER_EVENT
        {
            compiler.reject(
                &format!("{}/to", mapping.id),
                "KW-LIMIT-001",
                "sequence expansion across phases exceeds the per-event output limit",
            );
            break;
        }
    }
    Ok(CompiledConfig {
        raw,
        snapshot,
        diagnostics: compiler.diagnostics,
        disabled_rules: compiler.disabled,
    })
}

fn array<'a>(value: &'a Value, location: &str) -> Result<&'a Vec<Value>, ConfigError> {
    value
        .as_array()
        .ok_or_else(|| structure(location, "expected array"))
}
fn diagnostic(pointer: &str, code: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity: Severity::Error,
        pointer: pointer.into(),
        message: message.into(),
    }
}
struct Compiler<'a> {
    capabilities: &'a BackendCapabilities,
    diagnostics: Vec<Diagnostic>,
    disabled: usize,
    count: usize,
    alone_timeout_ns: u64,
}
impl Compiler<'_> {
    fn reject_unknown_fields(
        &mut self,
        object: &Map<String, Value>,
        pointer: &str,
        allowed: &[&str],
    ) {
        const METADATA: &[&str] = &["title", "description", "maintainers", "authors", "metadata"];
        for name in object.keys() {
            if !allowed.contains(&name.as_str()) && !METADATA.contains(&name.as_str()) {
                self.reject(
                    &format!("{pointer}/{}", escape_pointer(name)),
                    "KW-CAP-001",
                    "unknown or unsupported functional field; activation refused",
                );
            }
        }
    }
    fn reject(&mut self, pointer: &str, code: &str, message: &str) {
        self.diagnostics.push(diagnostic(pointer, code, message));
        self.disabled += 1;
    }
    fn count_rule(&mut self, pointer: &str) -> Result<(), ConfigError> {
        self.count += 1;
        if self.count > MAX_RULES {
            return Err(structure(pointer, "rule limit exceeded"));
        }
        Ok(())
    }
    fn mapping(&self, value: &Value, pointer: &str, complex: bool) -> Result<Mapping, Diagnostic> {
        let object = value
            .as_object()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected mapping object"))?;
        let allowed: &[&str] = if complex {
            &[
                "type",
                "from",
                "to",
                "description",
                "to_if_alone",
                "to_after_key_up",
                "parameters",
            ]
        } else {
            &["from", "to"]
        };
        for name in object.keys() {
            if !allowed.contains(&name.as_str()) {
                return Err(diagnostic(
                    &format!("{pointer}/{}", escape_pointer(name)),
                    "KW-CAP-001",
                    format!("field {name} is not implemented; entire containing rule disabled"),
                ));
            }
        }
        if complex && object.get("type").and_then(Value::as_str) != Some("basic") {
            return Err(diagnostic(
                &format!("{pointer}/type"),
                "KW-CAP-001",
                "only basic manipulators are implemented",
            ));
        }
        let (from, from_modifiers) =
            self.input_key(object.get("from"), &format!("{pointer}/from"), complex)?;
        let to = self.actions(object.get("to"), &format!("{pointer}/to"), complex)?;
        let extras = MappingExtras {
            from_modifiers,
            if_alone: self.actions(
                object.get("to_if_alone"),
                &format!("{pointer}/to_if_alone"),
                true,
            )?,
            after_key_up: self.actions(
                object.get("to_after_key_up"),
                &format!("{pointer}/to_after_key_up"),
                true,
            )?,
            alone_timeout_ns: alone_timeout(
                object.get("parameters"),
                &format!("{pointer}/parameters"),
            )?
            .unwrap_or(self.alone_timeout_ns),
        };
        if !object.contains_key("to")
            && extras.if_alone.is_empty()
            && extras.after_key_up.is_empty()
        {
            return Err(diagnostic(
                pointer,
                "KW-CONFIG-001",
                "mapping requires to or an extra output action",
            ));
        }
        Ok(Mapping {
            id: pointer.into(),
            from,
            to,
            allow_extra_modifiers: !complex,
            extras,
        })
    }
    fn actions(
        &self,
        value: Option<&Value>,
        pointer: &str,
        allow_empty: bool,
    ) -> Result<Vec<Action>, Diagnostic> {
        let Some(to) = value else {
            return if allow_empty {
                Ok(Vec::new())
            } else {
                Err(diagnostic(pointer, "KW-CONFIG-001", "missing to"))
            };
        };
        let values: Vec<_> = if let Some(values) = to.as_array() {
            values.iter().collect()
        } else if to.is_object() {
            vec![to]
        } else {
            return Err(diagnostic(
                pointer,
                "KW-CONFIG-001",
                "expected object or array",
            ));
        };
        if (!allow_empty && values.is_empty()) || values.len() > MAX_ACTIONS {
            return Err(diagnostic(
                pointer,
                "KW-CONFIG-001",
                "expected at most 256 output actions; simple to cannot be empty",
            ));
        }
        values
            .iter()
            .enumerate()
            .map(|(i, value)| self.output_action(value, &format!("{pointer}/{i}")))
            .collect()
    }
    fn output_action(&self, value: &Value, pointer: &str) -> Result<Action, Diagnostic> {
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected output action object"))?;
        let repeat = object
            .remove("repeat")
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    diagnostic(
                        &format!("{pointer}/repeat"),
                        "KW-CONFIG-001",
                        "repeat must be boolean",
                    )
                })
            })
            .transpose()?
            .unwrap_or(true);
        self.key(Some(&Value::Object(object)), pointer)
            .map(|key| Action { key, repeat })
    }
    fn input_key(
        &self,
        value: Option<&Value>,
        pointer: &str,
        complex: bool,
    ) -> Result<(KeyId, ModifierFilter), Diagnostic> {
        if !complex {
            return self
                .key(value, pointer)
                .map(|key| (key, ModifierFilter::default()));
        }
        let mut from = value
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected from object"))?;
        let mut filter = ModifierFilter::default();
        if let Some(modifiers) = from.remove("modifiers") {
            let modifiers = modifiers.as_object().ok_or_else(|| {
                diagnostic(
                    &format!("{pointer}/modifiers"),
                    "KW-CONFIG-001",
                    "expected object",
                )
            })?;
            for (field, value) in modifiers {
                let location = format!("{pointer}/modifiers/{}", escape_pointer(field));
                if field == "description" {
                    continue;
                }
                let target = match field.as_str() {
                    "mandatory" => &mut filter.mandatory,
                    "optional" => &mut filter.optional,
                    _ => {
                        return Err(diagnostic(
                            &location,
                            "KW-CAP-001",
                            "unknown modifier matching field",
                        ));
                    }
                };
                let is_array = value.is_array();
                let values: Vec<&Value> = if let Some(items) = value.as_array() {
                    items.iter().collect()
                } else if value.is_string() {
                    vec![value]
                } else {
                    return Err(diagnostic(
                        &location,
                        "KW-CONFIG-001",
                        "expected modifier string or array",
                    ));
                };
                if values.len() > 64 {
                    return Err(diagnostic(
                        &location,
                        "KW-LIMIT-001",
                        "modifier list exceeds 64 names",
                    ));
                }
                for (index, value) in values.iter().enumerate() {
                    let location = if !is_array {
                        location.clone()
                    } else {
                        format!("{location}/{index}")
                    };
                    let name = value.as_str().ok_or_else(|| {
                        diagnostic(&location, "KW-CONFIG-001", "expected modifier name")
                    })?;
                    let modifier = Modifier::named(name).ok_or_else(|| diagnostic(&location, "KW-CAP-001", format!("modifier {name} is unknown or requires unimplemented lock/vendor state")))?;
                    target.insert(modifier);
                }
            }
        }
        self.key(Some(&Value::Object(from)), pointer)
            .map(|key| (key, filter))
    }
    fn key(&self, value: Option<&Value>, pointer: &str) -> Result<KeyId, Diagnostic> {
        let object: &Map<String, Value> = value.and_then(Value::as_object).ok_or_else(|| {
            diagnostic(pointer, "KW-CONFIG-001", "expected key definition object")
        })?;
        if object.len() != 1 {
            return Err(diagnostic(
                pointer,
                "KW-CAP-001",
                "key modifiers/extra options are not yet implemented; rule disabled",
            ));
        }
        let (field, value) = object
            .iter()
            .next()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "empty event definition"))?;
        let name = value
            .as_str()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "key name must be a string"))?;
        let key = match field.as_str() {
            "key_code" => KeyId::named(KEYBOARD_PAGE, name),
            "consumer_key_code" => KeyId::named(CONSUMER_PAGE, name),
            "pointing_button" => name.parse::<KeyId>().and_then(|k| {
                if k.page == BUTTON_PAGE {
                    Ok(k)
                } else {
                    Err(UnknownKey(name.into()))
                }
            }),
            _ => {
                return Err(diagnostic(
                    pointer,
                    "KW-CAP-001",
                    format!("event type {field} is not implemented"),
                ));
            }
        }
        .map_err(|error| diagnostic(pointer, "KW-KEY-001", error.to_string()))?;
        if !self.capabilities.supports(key) {
            return Err(diagnostic(
                pointer,
                "KW-CAP-001",
                format!("{} cannot output {key}", self.capabilities.name),
            ));
        }
        Ok(key)
    }
}
fn alone_timeout(value: Option<&Value>, pointer: &str) -> Result<Option<u64>, Diagnostic> {
    let Some(value) = value else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected parameters object"))?;
    let mut timeout = None;
    for (name, value) in object {
        let location = format!("{pointer}/{}", escape_pointer(name));
        if ![
            "basic.to_if_alone_timeout_milliseconds",
            "basic.to_if_held_down_threshold_milliseconds",
            "basic.to_delayed_action_delay_milliseconds",
            "basic.simultaneous_threshold_milliseconds",
        ]
        .contains(&name.as_str())
        {
            return Err(diagnostic(
                &location,
                "KW-CAP-001",
                "timing parameter is not implemented",
            ));
        }
        let milliseconds = value
            .as_u64()
            .filter(|value| *value <= 86_400_000)
            .ok_or_else(|| {
                diagnostic(
                    &location,
                    "KW-CONFIG-001",
                    "expected integer milliseconds in 0..=86400000",
                )
            })?;
        if name == "basic.to_if_alone_timeout_milliseconds" {
            timeout = Some(milliseconds * 1_000_000);
        }
    }
    Ok(timeout)
}
fn escape_pointer(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}
