use serde::{Deserialize, Serialize};

pub const MAX_VARIABLES: usize = 256;
pub const MAX_VARIABLE_NAME_BYTES: usize = 256;
pub const MAX_VARIABLE_STRING_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VariableValue {
    Integer(i64),
    Boolean(bool),
    String(String),
}
impl Default for VariableValue {
    fn default() -> Self {
        Self::Integer(0)
    }
}
impl VariableValue {
    pub fn bounded(&self) -> bool {
        !matches!(self, Self::String(value) if value.len() > MAX_VARIABLE_STRING_BYTES)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariableOperation {
    Set {
        value: Option<VariableValue>,
        key_up_value: Option<VariableValue>,
    },
    Unset,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableAction {
    pub name: String,
    pub operation: VariableOperation,
}
pub fn valid_variable_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_VARIABLE_NAME_BYTES
}
