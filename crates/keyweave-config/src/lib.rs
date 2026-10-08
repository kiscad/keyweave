//! Bounded JSONC parsing and fail-closed compilation. No script or shell evaluation.
use keyweave_engine::{ConfigSnapshot, Mapping};
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
    Ok(serde_json::from_slice(&output)?)
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
    };
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
        if profile
            .get(field)
            .is_some_and(|v| !v.as_array().is_some_and(Vec::is_empty))
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
        if object.get("parameters").is_some_and(|v| !v.is_object()) {
            return Err(structure(
                &format!("{pointer}/complex_modifications/parameters"),
                "expected object",
            ));
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
    for mapping in snapshot.simple.iter().chain(&snapshot.complex) {
        if snapshot.expansion_bound(mapping.from).saturating_mul(2)
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
}
impl Compiler<'_> {
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
            &["type", "from", "to", "description"]
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
        let from = self.key(object.get("from"), &format!("{pointer}/from"))?;
        let to = object
            .get("to")
            .ok_or_else(|| diagnostic(&format!("{pointer}/to"), "KW-CONFIG-001", "missing to"))?;
        let values: Vec<_> = if let Some(values) = to.as_array() {
            values.iter().collect()
        } else if to.is_object() {
            vec![to]
        } else {
            return Err(diagnostic(
                &format!("{pointer}/to"),
                "KW-CONFIG-001",
                "expected object or array",
            ));
        };
        if values.is_empty() || values.len() > MAX_ACTIONS {
            return Err(diagnostic(
                &format!("{pointer}/to"),
                "KW-CONFIG-001",
                "expected 1..=256 output actions; use vk_none to suppress",
            ));
        }
        let to = values
            .iter()
            .enumerate()
            .map(|(i, value)| self.key(Some(value), &format!("{pointer}/to/{i}")))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Mapping {
            id: pointer.into(),
            from,
            to,
            allow_extra_modifiers: !complex,
        })
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
fn escape_pointer(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}
