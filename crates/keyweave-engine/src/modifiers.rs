use keyweave_types::{KEYBOARD_PAGE, KeyId};
use std::collections::BTreeSet;

/// Order follows upstream modifier_definition::modifier, independent of JSON order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Modifier {
    Any,
    Command,
    Control,
    LeftCommand,
    LeftControl,
    LeftOption,
    LeftShift,
    Option,
    RightCommand,
    RightControl,
    RightOption,
    RightShift,
    Shift,
}
impl Modifier {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "any" => Self::Any,
            "command" | "gui" => Self::Command,
            "control" => Self::Control,
            "option" | "alt" => Self::Option,
            "shift" => Self::Shift,
            "left_command" | "left_gui" => Self::LeftCommand,
            "right_command" | "right_gui" => Self::RightCommand,
            "left_control" => Self::LeftControl,
            "right_control" => Self::RightControl,
            "left_option" | "left_alt" => Self::LeftOption,
            "right_option" | "right_alt" => Self::RightOption,
            "left_shift" => Self::LeftShift,
            "right_shift" => Self::RightShift,
            _ => return None,
        })
    }
    pub fn keys(self) -> Vec<KeyId> {
        let usages: &[u16] = match self {
            Self::Any => &[],
            Self::Command => &[0xe3, 0xe7],
            Self::Control => &[0xe0, 0xe4],
            Self::Option => &[0xe2, 0xe6],
            Self::Shift => &[0xe1, 0xe5],
            Self::LeftCommand => &[0xe3],
            Self::RightCommand => &[0xe7],
            Self::LeftControl => &[0xe0],
            Self::RightControl => &[0xe4],
            Self::LeftOption => &[0xe2],
            Self::RightOption => &[0xe6],
            Self::LeftShift => &[0xe1],
            Self::RightShift => &[0xe5],
        };
        usages
            .iter()
            .map(|usage| KeyId {
                page: KEYBOARD_PAGE,
                usage: *usage,
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModifierFilter {
    pub mandatory: BTreeSet<Modifier>,
    pub optional: BTreeSet<Modifier>,
}
impl ModifierFilter {
    pub fn consumed(
        &self,
        pressed: &BTreeSet<KeyId>,
        allow_extra: bool,
    ) -> Option<BTreeSet<KeyId>> {
        if self.mandatory.contains(&Modifier::Any) {
            return Some(pressed.clone());
        }
        let mut consumed = BTreeSet::new();
        let mut allowed = BTreeSet::new();
        for modifier in &self.mandatory {
            let keys = modifier.keys();
            consumed.insert(keys.iter().copied().find(|key| pressed.contains(key))?);
            allowed.extend(keys);
        }
        if allow_extra || self.optional.contains(&Modifier::Any) {
            return Some(consumed);
        }
        for modifier in &self.optional {
            allowed.extend(modifier.keys());
        }
        pressed.is_subset(&allowed).then_some(consumed)
    }
}
