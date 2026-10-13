use super::*;
use keyweave_engine::simultaneous::{KeyOrder, KeyUpWhen, MAX_CHORD_KEYS, Simultaneous};
pub(super) fn threshold(value: Option<&Value>, pointer: &str) -> Result<Option<u64>, Diagnostic> {
    alone_timeout(value, pointer)?; // Shared strict known-parameter/type/range checks.
    Ok(value
        .and_then(|value| value.get("basic.simultaneous_threshold_milliseconds"))
        .and_then(Value::as_u64)
        .map(|value| value.min(1000) * 1_000_000))
}
impl Compiler<'_> {
    pub(super) fn chord_input(
        &self,
        value: Option<&Value>,
        pointer: &str,
        complex: bool,
        threshold_ns: u64,
    ) -> Result<(KeyId, ModifierFilter, Option<Simultaneous>), Diagnostic> {
        if !complex || value.and_then(|value| value.get("simultaneous")).is_none() {
            if value
                .and_then(|value| value.get("simultaneous_options"))
                .is_some()
            {
                return Err(diagnostic(
                    &format!("{pointer}/simultaneous_options"),
                    "KW-CONFIG-001",
                    "simultaneous_options requires simultaneous",
                ));
            }
            let (key, modifiers) = self.input_key(value, pointer, complex)?;
            return Ok((key, modifiers, None));
        }
        let mut from = value
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "expected from object"))?;
        if from.contains_key("key_code") {
            return Err(diagnostic(
                &format!("{pointer}/key_code"),
                "KW-CONFIG-001",
                "choose either key_code or simultaneous",
            ));
        }
        let entries = from
            .remove("simultaneous")
            .ok_or_else(|| diagnostic(pointer, "KW-CONFIG-001", "missing simultaneous"))?;
        let entries = entries
            .as_array()
            .filter(|entries| !entries.is_empty() && entries.len() <= MAX_CHORD_KEYS)
            .ok_or_else(|| {
                diagnostic(
                    &format!("{pointer}/simultaneous"),
                    "KW-LIMIT-001",
                    "expected 1..=32 keyboard entries",
                )
            })?;
        let mut keys = Vec::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            let at = format!("{pointer}/simultaneous/{index}");
            let key = self.key(Some(entry), &at)?;
            if key.page != KEYBOARD_PAGE || keys.contains(&key) {
                return Err(diagnostic(
                    &at,
                    "KW-CONFIG-001",
                    "simultaneous requires distinct standard keyboard keys",
                ));
            }
            keys.push(key);
        }
        let mut chord = Simultaneous::new(keys);
        chord.threshold_ns = threshold_ns;
        if let Some(options) = from.remove("simultaneous_options") {
            let options = options.as_object().ok_or_else(|| {
                diagnostic(
                    &format!("{pointer}/simultaneous_options"),
                    "KW-CONFIG-001",
                    "expected options object",
                )
            })?;
            for (name, value) in options {
                let at = format!("{pointer}/simultaneous_options/{}", escape_pointer(name));
                match name.as_str() {
                    "description" => (),
                    "detect_key_down_uninterruptedly" => {
                        chord.detect_key_down_uninterruptedly = value
                            .as_bool()
                            .ok_or_else(|| diagnostic(&at, "KW-CONFIG-001", "expected boolean"))?
                    }
                    "key_down_order" => chord.key_down_order = order(value, &at)?,
                    "key_up_order" => chord.key_up_order = order(value, &at)?,
                    "key_up_when" => {
                        chord.key_up_when = match value.as_str() {
                            Some("any") => KeyUpWhen::Any,
                            Some("all") => KeyUpWhen::All,
                            _ => {
                                return Err(diagnostic(
                                    &at,
                                    "KW-CONFIG-001",
                                    "expected any or all",
                                ));
                            }
                        }
                    }
                    "to_after_key_up" => {
                        chord.after_key_up = self.actions(Some(value), &at, true)?
                    }
                    _ => {
                        return Err(diagnostic(
                            &at,
                            "KW-CAP-001",
                            "unsupported simultaneous option",
                        ));
                    }
                }
            }
        }
        from.insert(
            "key_code".into(),
            entries[0].get("key_code").cloned().ok_or_else(|| {
                diagnostic(pointer, "KW-CONFIG-001", "expected keyboard key_code")
            })?,
        );
        let (first, modifiers) = self.input_key(Some(&Value::Object(from)), pointer, true)?;
        Ok((first, modifiers, Some(chord)))
    }
}
fn order(value: &Value, pointer: &str) -> Result<KeyOrder, Diagnostic> {
    match value.as_str() {
        Some("insensitive") => Ok(KeyOrder::Insensitive),
        Some("strict") => Ok(KeyOrder::Strict),
        Some("strict_inverse") => Ok(KeyOrder::StrictInverse),
        _ => Err(diagnostic(
            pointer,
            "KW-CONFIG-001",
            "expected insensitive, strict or strict_inverse",
        )),
    }
}
