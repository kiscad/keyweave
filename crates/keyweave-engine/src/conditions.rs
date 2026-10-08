use keyweave_types::*;

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
        if !context.fresh(now) {
            return Truth::Unknown;
        }
        let Some(devices) = &context.devices else {
            return Truth::Unknown;
        };
        match self {
            Self::Device {
                identifiers,
                unless,
                exists,
            } => {
                let value = if *exists {
                    devices.iter().fold(Truth::False, |value, device| {
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
