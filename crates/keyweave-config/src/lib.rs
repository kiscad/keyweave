//! Bounded JSONC parsing and fail-closed compilation. No script or shell evaluation.
mod unique_json;
use keyweave_engine::{
    Action, ActionTarget, Condition, ConfigSnapshot, DelayedAction, InputSourceFilter, Mapping,
    MappingExtras, Modifier, ModifierFilter, Pattern,
};
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
        delayed_delay_ns: 500_000_000,
        held_threshold_ns: 500_000_000,
        alone_timeout_ns: 1_000_000_000,
        pattern_count: std::cell::Cell::new(0),
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
        if let Ok(Some(threshold)) = held_threshold(
            object.get("parameters"),
            &format!("{pointer}/complex_modifications/parameters"),
        ) {
            compiler.held_threshold_ns = threshold;
        }
        if let Ok(Some(delay)) = delayed_delay(
            object.get("parameters"),
            &format!("{pointer}/complex_modifications/parameters"),
        ) {
            compiler.delayed_delay_ns = delay;
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
    if let Err(error) = snapshot.variable_names() {
        compiler.reject("/profiles", "KW-LIMIT-001", &error.to_string());
    }
    let bounds = snapshot.output_bounds();
    for mapping in snapshot.simple.iter().chain(&snapshot.complex) {
        if bounds.get(&mapping.from).copied().unwrap_or(10) > keyweave_engine::MAX_OUTPUTS_PER_EVENT
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
    delayed_delay_ns: u64,
    held_threshold_ns: u64,
    alone_timeout_ns: u64,
    pattern_count: std::cell::Cell<usize>,
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
                "to_delayed_action",
                "to_if_held_down",
                "to_if_alone",
                "to_after_key_up",
                "parameters",
                "conditions",
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
        if !complex
            && to
                .iter()
                .any(|action| !matches!(action.target, ActionTarget::Key(_)))
        {
            return Err(diagnostic(
                &format!("{pointer}/to"),
                "KW-CAP-001",
                "simple mapping requires key actions",
            ));
        }
        let extras = MappingExtras {
            delayed: self.delayed_action(
                object.get("to_delayed_action"),
                object.get("parameters"),
                &format!("{pointer}/to_delayed_action"),
                &format!("{pointer}/parameters"),
            )?,
            from_modifiers,
            conditions: self
                .conditions(object.get("conditions"), &format!("{pointer}/conditions"))?,
            if_held_down: self.actions(
                object.get("to_if_held_down"),
                &format!("{pointer}/to_if_held_down"),
                true,
            )?,
            held_threshold_ns: held_threshold(
                object.get("parameters"),
                &format!("{pointer}/parameters"),
            )?
            .unwrap_or(self.held_threshold_ns),
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
            && extras.delayed.as_ref().is_none_or(|delayed| {
                delayed.if_invoked.is_empty() && delayed.if_canceled.is_empty()
            })
            && extras.if_held_down.is_empty()
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
    fn delayed_action(
        &self,
        value: Option<&Value>,
        parameters: Option<&Value>,
        pointer: &str,
        parameter_pointer: &str,
    ) -> Result<Option<DelayedAction>, Diagnostic> {
        let Some(value) = value else {
            return Ok(None);
        };
        let object = value.as_object().ok_or_else(|| {
            diagnostic(pointer, "KW-CONFIG-001", "expected delayed_action object")
        })?;
        for name in object.keys() {
            if !["to_if_invoked", "to_if_canceled"].contains(&name.as_str()) {
                return Err(diagnostic(
                    &format!("{pointer}/{}", escape_pointer(name)),
                    "KW-CAP-001",
                    "unsupported delayed_action field",
                ));
            }
        }
        Ok(Some(DelayedAction {
            if_invoked: self.actions(
                object.get("to_if_invoked"),
                &format!("{pointer}/to_if_invoked"),
                true,
            )?,
            if_canceled: self.actions(
                object.get("to_if_canceled"),
                &format!("{pointer}/to_if_canceled"),
                true,
            )?,
            delay_ns: delayed_delay(parameters, parameter_pointer)?
                .unwrap_or(self.delayed_delay_ns),
        }))
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
            .map(|(i, value)| {
                let location = if to.is_array() {
                    format!("{pointer}/{i}")
                } else {
                    pointer.to_owned()
                };
                self.output_action(value, &location)
            })
            .collect()
    }
    fn output_action(&self, value: &Value, pointer: &str) -> Result<Action, Diagnostic> {
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected output action object"))?;
        let conditions_value = object.remove("conditions");
        let conditions =
            self.conditions(conditions_value.as_ref(), &format!("{pointer}/conditions"))?;
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
        let mut boolean = |field: &str| -> Result<bool, Diagnostic> {
            object
                .remove(field)
                .map(|value| {
                    value.as_bool().ok_or_else(|| {
                        diagnostic(
                            &format!("{pointer}/{field}"),
                            "KW-CONFIG-001",
                            "expected boolean action option",
                        )
                    })
                })
                .transpose()
                .map(|value| value.unwrap_or(false))
        };
        let lazy = boolean("lazy")?;
        let halt = boolean("halt")?;
        let mut modifiers = std::collections::BTreeSet::new();
        if let Some(value) = object.remove("modifiers") {
            let location = format!("{pointer}/modifiers");
            let is_array = value.is_array();
            let values: Vec<_> = if let Some(items) = value.as_array() {
                items.iter().collect()
            } else if value.is_string() {
                vec![&value]
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
                let location = if is_array {
                    format!("{location}/{index}")
                } else {
                    location.clone()
                };
                let name = value.as_str().ok_or_else(|| {
                    diagnostic(&location, "KW-CONFIG-001", "expected modifier name")
                })?;
                modifiers.insert(Modifier::named(name).ok_or_else(|| {
                    diagnostic(
                        &location,
                        "KW-CAP-001",
                        format!(
                            "modifier {name} is unknown or requires unimplemented lock/vendor state"
                        ),
                    )
                })?);
            }
        }
        let modifiers: Vec<KeyId> = modifiers
            .into_iter()
            .filter_map(|modifier| modifier.keys().first().copied())
            .collect();
        let target = if let Some(variable) = object.remove("set_variable") {
            if object.keys().any(|name| name != "description") || !modifiers.is_empty() {
                return Err(diagnostic(
                    pointer,
                    "KW-CAP-001",
                    "set_variable must be the only target and cannot carry modifier scope",
                ));
            }
            ActionTarget::SetVariable(
                self.variable_action(&variable, &format!("{pointer}/set_variable"))?,
            )
        } else {
            ActionTarget::Key(self.key(Some(&Value::Object(object)), pointer)?)
        };
        Ok(Action {
            target,
            repeat,
            lazy,
            halt,
            modifiers,
            conditions,
        })
    }
    fn variable_name(&self, value: Option<&Value>, pointer: &str) -> Result<String, Diagnostic> {
        value
            .and_then(Value::as_str)
            .filter(|name| valid_variable_name(name))
            .map(str::to_owned)
            .ok_or_else(|| {
                diagnostic(
                    pointer,
                    "KW-CONFIG-001",
                    "variable name must be nonempty and at most 256 bytes",
                )
            })
    }
    fn variable_value(
        &self,
        value: Option<&Value>,
        pointer: &str,
    ) -> Result<VariableValue, Diagnostic> {
        let value: VariableValue = serde_json::from_value(value.cloned().unwrap_or(Value::Null))
            .map_err(|_| {
                diagnostic(
                    pointer,
                    "KW-CONFIG-001",
                    "variable value requires i64 integer, boolean or string",
                )
            })?;
        if !value.bounded() {
            return Err(diagnostic(
                pointer,
                "KW-LIMIT-001",
                "variable string exceeds 4096 bytes",
            ));
        }
        Ok(value)
    }
    fn variable_action(&self, value: &Value, pointer: &str) -> Result<VariableAction, Diagnostic> {
        let object = value
            .as_object()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected set_variable object"))?;
        for name in object.keys() {
            if !["name", "value", "key_up_value", "type"].contains(&name.as_str()) {
                return Err(diagnostic(
                    &format!("{pointer}/{}", escape_pointer(name)),
                    "KW-CAP-001",
                    "unsupported variable field/expression",
                ));
            }
        }
        let name = self.variable_name(object.get("name"), &format!("{pointer}/name"))?;
        let operation = match object.get("type") {
            None => "set",
            Some(Value::String(kind)) => kind.as_str(),
            _ => {
                return Err(diagnostic(
                    &format!("{pointer}/type"),
                    "KW-CONFIG-001",
                    "variable action type requires set or unset",
                ));
            }
        };
        let operation = match operation {
            "set" => {
                let value = object
                    .get("value")
                    .map(|value| self.variable_value(Some(value), &format!("{pointer}/value")))
                    .transpose()?;
                let key_up_value = object
                    .get("key_up_value")
                    .map(|value| {
                        self.variable_value(Some(value), &format!("{pointer}/key_up_value"))
                    })
                    .transpose()?;
                if value.is_none() && key_up_value.is_none() {
                    return Err(diagnostic(
                        pointer,
                        "KW-CONFIG-001",
                        "set requires value or key_up_value",
                    ));
                }
                VariableOperation::Set {
                    value,
                    key_up_value,
                }
            }
            "unset" => {
                if object.contains_key("value") || object.contains_key("key_up_value") {
                    return Err(diagnostic(
                        pointer,
                        "KW-CONFIG-001",
                        "unset cannot carry ignored values",
                    ));
                }
                VariableOperation::Unset
            }
            _ => {
                return Err(diagnostic(
                    &format!("{pointer}/type"),
                    "KW-CONFIG-001",
                    "variable action type requires set or unset",
                ));
            }
        };
        Ok(VariableAction { name, operation })
    }

    fn conditions(
        &self,
        value: Option<&Value>,
        pointer: &str,
    ) -> Result<Vec<Condition>, Diagnostic> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let values = value
            .as_array()
            .filter(|values| values.len() <= 256)
            .ok_or_else(|| {
                diagnostic(pointer, "KW-CONFIG-001", "expected at most 256 conditions")
            })?;
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let location = format!("{pointer}/{index}");
                let object = value.as_object().ok_or_else(|| {
                    diagnostic(&location, "KW-CONFIG-001", "expected condition object")
                })?;
                let kind = object.get("type").and_then(Value::as_str);
                if matches!(kind, Some("variable_if" | "variable_unless")) {
                    for name in object.keys() {
                        if !["type", "name", "value", "description"].contains(&name.as_str()) {
                            return Err(diagnostic(
                                &format!("{location}/{}", escape_pointer(name)),
                                "KW-CAP-001",
                                "unsupported variable condition field",
                            ));
                        }
                    }
                    let name =
                        self.variable_name(object.get("name"), &format!("{location}/name"))?;
                    let value =
                        self.variable_value(object.get("value"), &format!("{location}/value"))?;
                    return Ok(Condition::Variable {
                        name,
                        value,
                        unless: kind == Some("variable_unless"),
                    });
                }
                if matches!(
                    kind,
                    Some(
                        "frontmost_application_if"
                            | "frontmost_application_unless"
                            | "input_source_if"
                            | "input_source_unless"
                    )
                ) {
                    return self.text_condition(object, &location, kind.unwrap_or_default());
                }
                let (unless, exists) = match kind {
                    Some("device_if") => (false, false),
                    Some("device_unless") => (true, false),
                    Some("device_exists_if") => (false, true),
                    Some("device_exists_unless") => (true, true),
                    _ => {
                        return Err(diagnostic(
                            &format!("{location}/type"),
                            "KW-CAP-001",
                            "condition type is not implemented; entire containing rule disabled",
                        ));
                    }
                };
                if !self.capabilities.reliable_device_identity {
                    return Err(diagnostic(
                        &location,
                        "KW-CAP-001",
                        "backend cannot supply reliable device identity/inventory",
                    ));
                }
                for name in object.keys() {
                    if !["type", "identifiers", "description"].contains(&name.as_str()) {
                        return Err(diagnostic(
                            &format!("{location}/{}", escape_pointer(name)),
                            "KW-CAP-001",
                            "unsupported condition field",
                        ));
                    }
                }
                let identifiers = object
                    .get("identifiers")
                    .and_then(Value::as_array)
                    .filter(|values| values.len() <= 256)
                    .ok_or_else(|| {
                        diagnostic(
                            &format!("{location}/identifiers"),
                            "KW-CONFIG-001",
                            "expected at most 256 device identifiers",
                        )
                    })?;
                let mut filters = Vec::new();
                for (index, value) in identifiers.iter().enumerate() {
                    let location = format!("{location}/identifiers/{index}");
                    let mut object = value.as_object().cloned().ok_or_else(|| {
                        diagnostic(
                            &location,
                            "KW-CONFIG-001",
                            "expected device identifier object",
                        )
                    })?;
                    object.remove("description");
                    for name in object.keys() {
                        if ![
                            "vendor_id",
                            "product_id",
                            "location_id",
                            "device_address",
                            "is_keyboard",
                            "is_pointing_device",
                            "is_game_pad",
                            "is_consumer",
                            "is_built_in_keyboard",
                        ]
                        .contains(&name.as_str())
                        {
                            return Err(diagnostic(
                                &format!("{location}/{}", escape_pointer(name)),
                                "KW-CAP-001",
                                "unsupported device identifier field",
                            ));
                        }
                        let valid = match name.as_str() {
                            "vendor_id" | "product_id" => object[name]
                                .as_u64()
                                .is_some_and(|value| u32::try_from(value).is_ok()),
                            "location_id" => object[name]
                                .as_i64()
                                .is_some_and(|value| i32::try_from(value).is_ok()),
                            "device_address" => object[name]
                                .as_str()
                                .is_some_and(|value| value.len() <= 256),
                            _ => object[name].is_boolean(),
                        };
                        if !valid {
                            return Err(diagnostic(
                                &format!("{location}/{}", escape_pointer(name)),
                                "KW-CONFIG-001",
                                "invalid device identifier type or bounded value",
                            ));
                        }
                    }
                    let properties: DeviceProperties =
                        serde_json::from_value(Value::Object(object)).map_err(|_| {
                            diagnostic(&location, "KW-CONFIG-001", "invalid device identifier")
                        })?;
                    if properties.has_constraints() {
                        filters.push(properties);
                    }
                }
                Ok(Condition::Device {
                    identifiers: filters,
                    unless,
                    exists,
                })
            })
            .collect()
    }

    fn pattern(&self, value: &Value, pointer: &str) -> Result<Pattern, Diagnostic> {
        let text = value
            .as_str()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected regex string"))?;
        let count = self.pattern_count.get();
        if count >= 256 {
            return Err(diagnostic(
                pointer,
                "KW-LIMIT-001",
                "selected configuration exceeds 256 regex patterns",
            ));
        }
        self.pattern_count.set(count + 1);
        Pattern::compile(text).map_err(|error| diagnostic(pointer, "KW-REGEX-001", error))
    }
    fn patterns(&self, value: Option<&Value>, pointer: &str) -> Result<Vec<Pattern>, Diagnostic> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        let values = value
            .as_array()
            .filter(|values| values.len() <= 256)
            .ok_or_else(|| {
                diagnostic(
                    pointer,
                    "KW-CONFIG-001",
                    "expected at most 256 regex patterns",
                )
            })?;
        values
            .iter()
            .enumerate()
            .map(|(index, value)| self.pattern(value, &format!("{pointer}/{index}")))
            .collect()
    }
    fn text_condition(
        &self,
        object: &Map<String, Value>,
        pointer: &str,
        kind: &str,
    ) -> Result<Condition, Diagnostic> {
        let application = kind.starts_with("frontmost_application_");
        let allowed: &[&str] = if application {
            &["type", "bundle_identifiers", "file_paths", "description"]
        } else {
            &["type", "input_sources", "description"]
        };
        for name in object.keys() {
            if !allowed.contains(&name.as_str()) {
                return Err(diagnostic(
                    &format!("{pointer}/{}", escape_pointer(name)),
                    "KW-CAP-001",
                    "unsupported application/input-source condition field",
                ));
            }
        }
        let unless = kind.ends_with("_unless");
        if application {
            return Ok(Condition::Application {
                bundle_identifiers: self.patterns(
                    object.get("bundle_identifiers"),
                    &format!("{pointer}/bundle_identifiers"),
                )?,
                file_paths: self
                    .patterns(object.get("file_paths"), &format!("{pointer}/file_paths"))?,
                unless,
            });
        }
        let values = object
            .get("input_sources")
            .and_then(Value::as_array)
            .filter(|values| values.len() <= 256)
            .ok_or_else(|| {
                diagnostic(
                    &format!("{pointer}/input_sources"),
                    "KW-CONFIG-001",
                    "expected at most 256 input-source specifiers",
                )
            })?;
        let sources = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let pointer = format!("{pointer}/input_sources/{index}");
                let object = value.as_object().ok_or_else(|| {
                    diagnostic(
                        &pointer,
                        "KW-CONFIG-001",
                        "expected input-source specifier object",
                    )
                })?;
                for name in object.keys() {
                    if ![
                        "language",
                        "input_source_id",
                        "input_mode_id",
                        "description",
                    ]
                    .contains(&name.as_str())
                    {
                        return Err(diagnostic(
                            &format!("{pointer}/{}", escape_pointer(name)),
                            "KW-CAP-001",
                            "unsupported input-source specifier field",
                        ));
                    }
                }
                Ok(InputSourceFilter {
                    language: object
                        .get("language")
                        .map(|value| self.pattern(value, &format!("{pointer}/language")))
                        .transpose()?,
                    input_source_id: object
                        .get("input_source_id")
                        .map(|value| self.pattern(value, &format!("{pointer}/input_source_id")))
                        .transpose()?,
                    input_mode_id: object
                        .get("input_mode_id")
                        .map(|value| self.pattern(value, &format!("{pointer}/input_mode_id")))
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        Ok(Condition::InputSource { sources, unless })
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
fn delayed_delay(value: Option<&Value>, pointer: &str) -> Result<Option<u64>, Diagnostic> {
    alone_timeout(value, pointer)?;
    Ok(value
        .and_then(|value| value.get("basic.to_delayed_action_delay_milliseconds"))
        .and_then(Value::as_u64)
        .map(|milliseconds| milliseconds * 1_000_000))
}
fn held_threshold(value: Option<&Value>, pointer: &str) -> Result<Option<u64>, Diagnostic> {
    alone_timeout(value, pointer)?;
    Ok(value
        .and_then(|value| value.get("basic.to_if_held_down_threshold_milliseconds"))
        .and_then(Value::as_u64)
        .map(|milliseconds| milliseconds * 1_000_000))
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
