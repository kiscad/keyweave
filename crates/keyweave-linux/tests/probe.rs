#![allow(clippy::unwrap_used)]
use keyweave_linux::{keys::native_codes, probe::*};
fn keyboard() -> Capabilities {
    let mut events = Bits::empty(32).unwrap();
    for code in [0, 1, 4, 17, 20] {
        events.set(code).unwrap();
    }
    let mut keys = Bits::empty(768).unwrap();
    keys.set(30).unwrap();
    keys.set(29).unwrap();
    let mut misc = Bits::empty(8).unwrap();
    misc.set(4).unwrap();
    let mut leds = Bits::empty(16).unwrap();
    for code in 0..3 {
        leds.set(code).unwrap();
    }
    Capabilities {
        events,
        keys,
        misc,
        leds,
        properties: Bits::empty(32).unwrap(),
    }
}
#[test]
fn words_preserve_native_byte_order_boundaries_and_full_key_range() {
    let mut bytes = vec![0; 96];
    bytes[..8].copy_from_slice(&((1u64 << 63) | 1).to_ne_bytes());
    bytes[8..16].copy_from_slice(&1u64.to_ne_bytes());
    bytes[88..96].copy_from_slice(&(1u64 << 63).to_ne_bytes());
    let bits = Bits::from_native_words(&bytes, 768).unwrap();
    assert_eq!(bits.codes().collect::<Vec<_>>(), [0, 63, 64, 767]);
    assert!(!bits.contains(768));
    assert_eq!(bits.count(), 768);
}
#[test]
fn invalid_mask_sizes_padding_and_code_bounds_are_explicit_errors() {
    assert!(Bits::empty(0).is_err());
    assert!(Bits::empty(769).is_err());
    assert!(Bits::from_native_words(&[0; 4], 32).is_err());
    assert!(Bits::from_native_words(&(1u64 << 32).to_ne_bytes(), 32).is_err());
    let mut bits = Bits::empty(32).unwrap();
    assert!(bits.set(32).is_err());
    assert!(bits.is_empty());
}
#[test]
fn ordinary_keyboard_capabilities_have_no_class_issues() {
    assert!(keyboard().keyboard_issues().is_empty());
}
#[test]
fn every_supported_native_key_can_be_advertised_without_false_rejection() {
    let mut caps = keyboard();
    for code in native_codes() {
        caps.keys.set(code).unwrap();
    }
    assert!(caps.keyboard_issues().is_empty());
}
#[test]
fn mixed_pointer_touch_switch_or_force_feedback_class_is_rejected() {
    for event in [2, 3, 5, 18, 21, 22, 23] {
        let mut caps = keyboard();
        caps.events.set(event).unwrap();
        assert!(
            caps.keyboard_issues()
                .contains(&AdmissionIssue::EventClass(event))
        );
    }
}
#[test]
fn unknown_key_metadata_led_and_touch_property_are_not_silently_dropped() {
    let mut caps = keyboard();
    caps.keys.set(240).unwrap();
    caps.misc.set(0).unwrap();
    caps.leds.set(4).unwrap();
    caps.properties.set(1).unwrap();
    let issues = caps.keyboard_issues();
    assert!(issues.contains(&AdmissionIssue::Key(240)));
    assert!(issues.contains(&AdmissionIssue::Misc(0)));
    assert!(issues.contains(&AdmissionIssue::Led(4)));
    assert!(issues.contains(&AdmissionIssue::Property(1)));
}
#[test]
fn missing_keys_are_not_assumed_to_be_a_safe_keyboard() {
    let mut caps = keyboard();
    caps.keys = Bits::empty(768).unwrap();
    assert!(caps.keyboard_issues().contains(&AdmissionIssue::NoKeys));
}
#[cfg(target_os = "linux")]
#[test]
fn explicit_node_syntax_excludes_other_devices_and_path_traversal() {
    use keyweave_linux::device::is_event_node;
    use std::path::Path;
    assert!(is_event_node(Path::new("/dev/input/event0")));
    for path in [
        "/dev/null",
        "/dev/input/event",
        "/dev/input/event-1",
        "/dev/input/event0/../event1",
        "/tmp/event0",
        "event0",
    ] {
        assert!(!is_event_node(Path::new(path)), "{path}");
    }
}
#[cfg(target_os = "linux")]
#[test]
fn opening_null_is_rejected_before_any_evdev_input_read() {
    use keyweave_linux::device::EvdevDevice;
    use std::path::Path;
    assert_eq!(
        EvdevDevice::open(Path::new("/dev/null"))
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
}
