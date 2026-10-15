use keyweave_types::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Pattern(Arc<regex::bytes::Regex>);
impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}
impl Eq for Pattern {}
impl Pattern {
    pub fn compile(pattern: &str) -> Result<Self, String> {
        if pattern.len() > 1024 || pattern.contains(['\r', '\n']) {
            return Err("pattern must be single-line UTF-8 and at most 1024 bytes".into());
        }
        if pattern.contains("&&") || pattern.contains("~~") || pattern.contains("--") {
            return Err("regex character-class set operations are unsupported".into());
        }
        let mut in_class = false;
        let mut chars = pattern.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let escaped = chars.next().ok_or("incomplete regex escape")?;
                if !escaped.is_ascii() {
                    return Err("non-ASCII regex escapes are unsupported; use literal UTF-8".into());
                }
                if in_class && escaped.is_ascii_alphanumeric() {
                    return Err("shorthand escapes inside character classes are unsupported".into());
                }
                if escaped.is_ascii_alphanumeric() && !"dDsSwWbBnrtf".contains(escaped) {
                    return Err("unsupported ECMAScript escape/backreference".into());
                }
            } else if ch == '[' {
                if in_class {
                    return Err("nested character classes are unsupported".into());
                }
                in_class = true;
            } else if ch == ']' {
                in_class = false;
            } else if !in_class
                && matches!(ch, '*' | '+' | '?')
                && chars.peek().is_some_and(|ch| matches!(ch, '*' | '+'))
            {
                return Err("consecutive or possessive quantifiers are unsupported".into());
            } else if ch == '(' && chars.peek() == Some(&'?') {
                chars.next();
                if chars.next() != Some(':') {
                    return Err(
                        "lookaround, inline flags and special groups are unsupported".into(),
                    );
                }
            }
        }
        // C++ std::regex<char> searches UTF-8 as bytes. Keep ASCII shorthands,
        // then encode every non-ASCII literal byte internally without grouping:
        // quantifiers/classes must retain the original byte-level scope.
        let mut normalized = String::new();
        let mut chars = pattern.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                let escaped = chars.next().ok_or("incomplete regex escape")?;
                match escaped {
                    'w' => normalized.push_str("[A-Za-z0-9_]"),
                    'W' => normalized.push_str("[^A-Za-z0-9_]"),
                    'd' => normalized.push_str("[0-9]"),
                    'D' => normalized.push_str("[^0-9]"),
                    's' => normalized.push_str("[ \\t\\n\\r\\x0B\\x0C]"),
                    'S' => normalized.push_str("[^ \\t\\n\\r\\x0B\\x0C]"),
                    'b' => normalized.push_str("(?-u:\\b)"),
                    'B' => normalized.push_str("(?-u:\\B)"),
                    _ => {
                        normalized.push('\\');
                        normalized.push(escaped);
                    }
                }
            } else {
                normalized.push(ch);
            }
        }
        let mut byte_pattern = String::with_capacity(normalized.len());
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for byte in normalized.bytes() {
            if byte.is_ascii() {
                byte_pattern.push(char::from(byte));
            } else {
                byte_pattern.push_str("\\x");
                byte_pattern.push(char::from(HEX[usize::from(byte >> 4)]));
                byte_pattern.push(char::from(HEX[usize::from(byte & 15)]));
            }
        }
        regex::bytes::RegexBuilder::new(&byte_pattern)
            .unicode(false)
            .size_limit(32 * 1024)
            .dfa_size_limit(16 * 1024)
            .build()
            .map(|regex| Self(Arc::new(regex)))
            .map_err(|error| error.to_string())
    }
    fn test(&self, value: &Option<String>) -> Truth {
        match value {
            Some(value) if !value.contains(['\r', '\n']) => {
                if self.0.is_match(value.as_bytes()) {
                    Truth::True
                } else {
                    Truth::False
                }
            }
            _ => Truth::Unknown,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputSourceFilter {
    pub language: Option<Pattern>,
    pub input_source_id: Option<Pattern>,
    pub input_mode_id: Option<Pattern>,
}
fn pattern_option(pattern: &Option<Pattern>, value: &Option<String>) -> Truth {
    pattern
        .as_ref()
        .map_or(Truth::True, |pattern| pattern.test(value))
}
fn pattern_list(patterns: &[Pattern], value: &Option<String>) -> Truth {
    patterns.iter().fold(Truth::False, |result, pattern| {
        result.or(pattern.test(value))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    Unknown,
}
impl Truth {
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::True,
        }
    }
    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::False,
        }
    }
    fn negated(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    Variable {
        name: String,
        value: VariableValue,
        unless: bool,
    },
    Application {
        bundle_identifiers: Vec<Pattern>,
        file_paths: Vec<Pattern>,
        unless: bool,
    },
    InputSource {
        sources: Vec<InputSourceFilter>,
        unless: bool,
    },
    Device {
        identifiers: Vec<DeviceProperties>,
        unless: bool,
        exists: bool,
    },
}
impl Condition {
    pub fn evaluate(
        &self,
        context: &ContextSnapshot,
        device: DeviceInstanceId,
        now: MonoTime,
    ) -> Truth {
        if matches!(self, Self::Variable { .. }) {
            return Truth::Unknown;
        }
        if !context.fresh(now) {
            return Truth::Unknown;
        }
        match self {
            Self::Variable { .. } => Truth::Unknown,
            Self::Application {
                bundle_identifiers,
                file_paths,
                unless,
            } => {
                let Some(application) = &context.application else {
                    return Truth::Unknown;
                };
                let result = pattern_list(bundle_identifiers, &application.bundle_identifier)
                    .or(pattern_list(file_paths, &application.file_path));
                if *unless { result.negated() } else { result }
            }
            Self::InputSource { sources, unless } => {
                let Some(source) = &context.input_source else {
                    return Truth::Unknown;
                };
                let result = sources.iter().fold(Truth::False, |result, filter| {
                    result.or(pattern_option(&filter.language, &source.language)
                        .and(pattern_option(
                            &filter.input_source_id,
                            &source.input_source_id,
                        ))
                        .and(pattern_option(&filter.input_mode_id, &source.input_mode_id)))
                });
                if *unless { result.negated() } else { result }
            }
            Self::Device {
                identifiers,
                unless,
                exists,
            } => {
                let Some(devices) = &context.devices else {
                    return Truth::Unknown;
                };
                let value = if *exists {
                    let initial = if context.device_inventory_scope.is_complete() {
                        Truth::False
                    } else {
                        Truth::Unknown
                    };
                    devices.iter().fold(initial, |value, device| {
                        value.or(matches_any(identifiers, &device.properties))
                    })
                } else if let Some(device) = devices.iter().find(|entry| entry.device == device) {
                    matches_any(identifiers, &device.properties)
                } else {
                    Truth::Unknown
                };
                if *unless { value.negated() } else { value }
            }
        }
    }
}

fn compare<T: PartialEq>(expected: &Option<T>, actual: &Option<T>) -> Truth {
    match (expected, actual) {
        (None, _) => Truth::True,
        (Some(_), None) => Truth::Unknown,
        (Some(expected), Some(actual)) if expected == actual => Truth::True,
        _ => Truth::False,
    }
}
fn matches_any(identifiers: &[DeviceProperties], actual: &DeviceProperties) -> Truth {
    identifiers.iter().fold(Truth::False, |value, expected| {
        let fields = [
            compare(&expected.vendor_id, &actual.vendor_id),
            compare(&expected.product_id, &actual.product_id),
            compare(&expected.location_id, &actual.location_id),
            compare(&expected.device_address, &actual.device_address),
            compare(&expected.is_keyboard, &actual.is_keyboard),
            compare(&expected.is_pointing_device, &actual.is_pointing_device),
            compare(&expected.is_game_pad, &actual.is_game_pad),
            compare(&expected.is_consumer, &actual.is_consumer),
            compare(&expected.is_built_in_keyboard, &actual.is_built_in_keyboard),
        ];
        value.or(fields.into_iter().fold(Truth::True, Truth::and))
    })
}
