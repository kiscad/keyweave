//! Device-specific Simple mappings precede global Simple mappings.
use super::*;
pub(super) fn identifier(value: &Value, location: &str) -> Result<DeviceProperties, Diagnostic> {
    let mut object = value.as_object().cloned().ok_or_else(|| {
        diagnostic(
            location,
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
    let properties: DeviceProperties = serde_json::from_value(Value::Object(object))
        .map_err(|_| diagnostic(location, "KW-CONFIG-001", "invalid device identifier"))?;
    Ok(properties)
}

pub(super) fn compile(
    compiler: &mut Compiler<'_>,
    value: &Value,
    pointer: &str,
) -> Result<Vec<Mapping>, ConfigError> {
    let devices = array(value, pointer)?;
    if devices.len() > 256 {
        return Err(structure(
            pointer,
            "at most 256 device entries are supported",
        ));
    }
    let mut result = Vec::new();
    for (index, value) in devices.iter().enumerate() {
        let location = format!("{pointer}/{index}");
        let device = value
            .as_object()
            .ok_or_else(|| structure(&location, "expected device object"))?;
        let before = compiler.disabled;
        compiler.reject_unknown_fields(
            device,
            &location,
            &[
                "identifiers",
                "simple_modifications",
                "fn_function_keys",
                "ignore",
                "disable_built_in_keyboard_if_exists",
                "treat_as_built_in_keyboard",
            ],
        );
        for flag in [
            "ignore",
            "disable_built_in_keyboard_if_exists",
            "treat_as_built_in_keyboard",
        ] {
            if let Some(value) = device.get(flag) {
                if !value.is_boolean() {
                    return Err(structure(&format!("{location}/{flag}"), "expected boolean"));
                }
                if value == &Value::Bool(true) {
                    compiler.reject(
                        &format!("{location}/{flag}"),
                        "KW-CAP-001",
                        "device capture/hardware setting is not implemented",
                    );
                }
            }
        }
        if let Some(value) = device.get("fn_function_keys")
            && !array(value, &format!("{location}/fn_function_keys"))?.is_empty()
        {
            compiler.reject(
                &format!("{location}/fn_function_keys"),
                "KW-CAP-001",
                "device Fn mappings are not implemented",
            );
        }
        let Some(identifiers) = device.get("identifiers") else {
            compiler.reject(
                &format!("{location}/identifiers"),
                "KW-CONFIG-001",
                "missing device identifiers",
            );
            continue;
        };
        let properties = match identifier(identifiers, &format!("{location}/identifiers")) {
            Ok(p) => p,
            Err(error) => {
                compiler.diagnostics.push(error);
                compiler.disabled += 1;
                continue;
            }
        };
        // Match upstream make_device_if_condition(profile.device): it normalizes
        // missing IDs to zero and type flags to false, and deliberately ignores
        // location/address/other type fields in this particular selector.
        let filter = DeviceProperties {
            vendor_id: Some(properties.vendor_id.unwrap_or(0)),
            product_id: Some(properties.product_id.unwrap_or(0)),
            is_keyboard: Some(properties.is_keyboard.unwrap_or(false)),
            is_pointing_device: Some(properties.is_pointing_device.unwrap_or(false)),
            ..Default::default()
        };
        if !compiler.capabilities.reliable_device_identity {
            compiler.reject(
                &location,
                "KW-CAP-001",
                "backend cannot supply reliable per-event device identity",
            );
        }
        let mut mappings = Vec::new();
        if let Some(value) = device.get("simple_modifications") {
            for (i, item) in array(value, &format!("{location}/simple_modifications"))?
                .iter()
                .enumerate()
            {
                let pointer = format!("{location}/simple_modifications/{i}");
                compiler.count_rule(&pointer)?;
                match compiler.mapping(item, &pointer, false) {
                    Ok(mut mapping) => {
                        mapping.extras.conditions.push(Condition::Device {
                            identifiers: vec![filter.clone()],
                            unless: false,
                            exists: false,
                        });
                        mappings.push(mapping);
                    }
                    Err(error) => {
                        compiler.diagnostics.push(error);
                        compiler.disabled += 1;
                    }
                }
            }
        }
        if compiler.disabled == before {
            if !mappings.is_empty() {
                compiler.diagnostics.push(Diagnostic {
                    code:"KW-DEVICE-001".into(), severity:Severity::Warning,
                    pointer:format!("{location}/identifiers"),
                    message:"Device-specific mappings require fresh trusted vendor/product and keyboard/pointing type facts. Missing or expired facts fall back to global Simple mappings; offline compilation does not establish provider readiness.".into(),
                });
            }
            result.extend(mappings);
        }
    }
    Ok(result)
}
