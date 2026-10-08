#![allow(clippy::unwrap_used)]
use keyweave_linux::{
    events::{EV_KEY, Event},
    keys::*,
};
use keyweave_types::*;
use std::collections::BTreeSet;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn event(code: u16, value: i32) -> Event {
    Event::output(EV_KEY, code, value)
}
#[test]
fn every_canonical_entry_round_trips_and_all_aliases_require_scan_on_input() {
    let mut hid = BTreeSet::new();
    let mut native = BTreeSet::new();
    for row in mappings() {
        assert!(hid.insert(row.key));
        let exact = resolve_input(row.code, Some(row.key), IdentityPolicy::RequireHidScan).unwrap();
        assert_eq!(exact.key, row.key);
        assert_eq!(exact.evidence, IdentityEvidence::VerifiedHidScan);
        if row.canonical {
            assert!(native.insert(row.code));
            assert_eq!(output_code(row.key), Ok(row.code));
            let inferred = resolve_input(row.code, None, IdentityPolicy::CanonicalEvdev).unwrap();
            assert_eq!(inferred.key, row.key);
            assert_eq!(inferred.evidence, IdentityEvidence::EvdevCanonical);
        } else {
            assert_eq!(output_code(row.key), Err(KeyError::OutputAlias));
        }
    }
    assert_eq!(mappings().len(), 159);
    assert_eq!(native_codes().collect::<Vec<_>>().len(), 154);
}
#[test]
fn left_right_modifiers_numpad_and_delete_positions_remain_distinct() {
    for (name, code) in [
        ("left_control", 29),
        ("right_control", 97),
        ("left_shift", 42),
        ("right_shift", 54),
        ("left_option", 56),
        ("right_option", 100),
        ("left_command", 125),
        ("right_command", 126),
        ("return_or_enter", 28),
        ("keypad_enter", 96),
        ("delete_or_backspace", 14),
        ("delete_forward", 111),
        ("1", 2),
        ("keypad_1", 79),
        ("f1", 59),
        ("f24", 194),
    ] {
        assert_eq!(output_code(key(name)), Ok(code));
    }
}
#[test]
fn keyboard_and_consumer_audio_are_not_silently_merged_as_hid_output() {
    let keyboard = key("mute");
    let consumer = key("consumer:mute");
    assert_ne!(keyboard, consumer);
    assert_eq!(
        resolve_input(113, None, IdentityPolicy::CanonicalEvdev)
            .unwrap()
            .key,
        consumer
    );
    assert!(
        resolve_input(113, None, IdentityPolicy::CanonicalEvdev)
            .unwrap()
            .has_known_aliases
    );
    assert_eq!(
        resolve_input(113, Some(keyboard), IdentityPolicy::RequireHidScan)
            .unwrap()
            .key,
        keyboard
    );
    assert_eq!(output_code(keyboard), Err(KeyError::OutputAlias));
    assert_eq!(output_code(consumer), Ok(113));
}
#[test]
fn non_us_pound_requires_hid_evidence_and_output_is_explicitly_unrepresentable() {
    assert_eq!(
        resolve_input(43, None, IdentityPolicy::CanonicalEvdev)
            .unwrap()
            .key,
        key("backslash")
    );
    assert_eq!(
        resolve_input(
            43,
            Some(key("non_us_pound")),
            IdentityPolicy::RequireHidScan
        )
        .unwrap()
        .key,
        key("non_us_pound")
    );
    assert_eq!(output_code(key("non_us_pound")), Err(KeyError::OutputAlias));
    assert_eq!(output_code(key("non_us_backslash")), Ok(86));
}
#[test]
fn strict_policy_requires_verified_scan_even_for_an_apparently_unique_evdev_code() {
    assert_eq!(
        resolve_input(30, None, IdentityPolicy::RequireHidScan),
        Err(KeyError::ScanRequired)
    );
    assert_eq!(
        resolve_input(30, Some(key("b")), IdentityPolicy::CanonicalEvdev),
        Err(KeyError::ScanMismatch)
    );
    assert_eq!(
        resolve_input(0, None, IdentityPolicy::CanonicalEvdev),
        Err(KeyError::UnsupportedNative)
    );
    assert_eq!(
        resolve_input(240, None, IdentityPolicy::CanonicalEvdev),
        Err(KeyError::UnsupportedNative)
    );
    assert_eq!(
        output_code(KeyId::new(0xff00, 1)),
        Err(KeyError::UnsupportedHid)
    );
}
#[test]
fn repeat_duplicate_down_and_up_use_saved_identity_despite_changed_or_missing_scan() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::RequireHidScan);
    let down = normalizer
        .process(event(113, 1), Some(key("mute")))
        .unwrap();
    let identity = match down {
        NormalizedKey::Event { identity, .. } => identity,
        _ => panic!("missing down"),
    };
    assert_eq!(
        normalizer
            .process(event(113, 1), Some(key("consumer:mute")))
            .unwrap(),
        NormalizedKey::Event {
            identity,
            edge: KeyEdge::Down
        }
    );
    assert_eq!(
        normalizer.process(event(113, 2), None).unwrap(),
        NormalizedKey::Event {
            identity,
            edge: KeyEdge::Repeat
        }
    );
    assert_eq!(
        normalizer.process(event(113, 0), None).unwrap(),
        NormalizedKey::Event {
            identity,
            edge: KeyEdge::Up
        }
    );
    assert_eq!(normalizer.pressed_count(), 0);
}
#[test]
fn unknown_up_repeat_and_reset_do_not_infer_a_down_or_release_another_native_source() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalEvdev);
    for (value, edge) in [(0, KeyEdge::Up), (2, KeyEdge::Repeat)] {
        assert_eq!(
            normalizer.process(event(30, value), None).unwrap(),
            NormalizedKey::Ignored { code: 30, edge }
        );
    }
    normalizer.process(event(30, 1), None).unwrap();
    normalizer.process(event(48, 0), None).unwrap();
    assert_eq!(normalizer.pressed_count(), 1);
    normalizer.reset();
    assert_eq!(normalizer.pressed_count(), 0);
    assert!(matches!(
        normalizer.process(event(30, 0), None).unwrap(),
        NormalizedKey::Ignored { .. }
    ));
}
#[test]
fn bad_event_and_scan_errors_preserve_existing_native_bindings() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalEvdev);
    normalizer.process(event(30, 1), None).unwrap();
    assert_eq!(
        normalizer.process(event(48, 1), Some(key("a"))),
        Err(KeyError::ScanMismatch)
    );
    assert_eq!(
        normalizer.process(event(30, 3), None),
        Err(KeyError::InvalidEvent)
    );
    assert_eq!(
        normalizer.process(Event::output(4, 4, 30), None),
        Err(KeyError::InvalidEvent)
    );
    assert_eq!(normalizer.pressed_count(), 1);
}
#[test]
fn native_normalizer_is_scoped_to_device_generation() {
    let mut old = KeyNormalizer::new(IdentityPolicy::CanonicalEvdev);
    let mut next = KeyNormalizer::new(IdentityPolicy::CanonicalEvdev);
    old.process(event(30, 1), None).unwrap();
    assert!(matches!(
        next.process(event(30, 0), None).unwrap(),
        NormalizedKey::Ignored { .. }
    ));
    assert_eq!(old.pressed_count(), 1);
}
#[test]
fn all_native_codes_can_be_pressed_without_unbounded_identity_growth() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalEvdev);
    for code in native_codes() {
        normalizer.process(event(code, 1), None).unwrap();
    }
    assert_eq!(normalizer.pressed_count(), 154);
    for code in native_codes() {
        normalizer.process(event(code, 0), None).unwrap();
    }
    assert_eq!(normalizer.pressed_count(), 0);
}
