//! OS classification metadata, not HID role inference or capture readiness.
#[cfg(target_os = "linux")]
mod native;
#[cfg(target_os = "linux")]
pub(crate) use native::observe;
#[cfg(target_os = "linux")]
pub use native::provider_available;
/// Static udev classification of this opened device instance. Rule changes
/// require reopening the input; unavailable records remain None as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceTypes {
    pub is_keyboard: bool,
    pub is_pointing_device: bool,
}
#[cfg(target_os = "linux")]
pub(crate) const TYPE_NAMES: [&str; 11] = [
    "ID_INPUT_KEYBOARD",
    "ID_INPUT_MOUSE",
    "ID_INPUT_TOUCHPAD",
    "ID_INPUT_TOUCHSCREEN",
    "ID_INPUT_TABLET",
    "ID_INPUT_TABLET_PAD",
    "ID_INPUT_POINTINGSTICK",
    "ID_INPUT_KEY",
    "ID_INPUT_JOYSTICK",
    "ID_INPUT_ACCELEROMETER",
    "ID_INPUT_SWITCH",
];
// Only well-defined bool tags are queried; e.g. TOUCHPAD_INTEGRATION is not boolean.
#[cfg(any(target_os = "linux", test))]
pub(crate) const BOOL_TYPES: usize = 11;
#[cfg(any(target_os = "linux", test))]
pub(crate) fn classify(
    initialized: bool,
    input: Option<bool>,
    tags: &[Option<bool>; BOOL_TYPES],
) -> Option<DeviceTypes> {
    if !initialized || input != Some(true) || !tags.contains(&Some(true)) {
        return None;
    }
    Some(DeviceTypes {
        is_keyboard: tags[0] == Some(true),
        is_pointing_device: tags[1..7].contains(&Some(true)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classification_requires_complete_initialized_input_record_and_positive_type() {
        let mut tags = [None; BOOL_TYPES];
        assert_eq!(classify(true, Some(true), &tags), None);
        tags[0] = Some(true);
        assert_eq!(classify(false, Some(true), &tags), None);
        assert_eq!(classify(true, None, &tags), None);
        assert_eq!(classify(true, Some(false), &tags), None);
        assert_eq!(
            classify(true, Some(true), &tags),
            Some(DeviceTypes {
                is_keyboard: true,
                is_pointing_device: false
            })
        );
        tags[0] = Some(false);
        tags[7] = Some(true);
        assert_eq!(
            classify(true, Some(true), &tags),
            Some(DeviceTypes {
                is_keyboard: false,
                is_pointing_device: false
            })
        );
    }
    #[test]
    fn pointer_categories_are_combined_without_conflating_keys_or_gamepads() {
        for i in 0..BOOL_TYPES {
            let mut tags = [None; BOOL_TYPES];
            tags[i] = Some(true);
            assert_eq!(
                classify(true, Some(true), &tags),
                Some(DeviceTypes {
                    is_keyboard: i == 0,
                    is_pointing_device: (1..7).contains(&i)
                })
            );
        }
        let mut tags = [None; BOOL_TYPES];
        tags[0] = Some(true);
        tags[1] = Some(true);
        assert_eq!(
            classify(true, Some(true), &tags),
            Some(DeviceTypes {
                is_keyboard: true,
                is_pointing_device: true
            })
        );
    }
}
