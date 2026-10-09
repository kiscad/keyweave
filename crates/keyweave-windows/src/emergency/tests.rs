#![allow(clippy::unwrap_used)]
use super::*;
fn key(usage: u16) -> KeyId {
    KeyId::new(KEYBOARD_PAGE, usage)
}
fn chord() -> EmergencyChord {
    let mut chord = EmergencyChord::new();
    for usage in [0xe0, 0xe2, 0xe1, 0x45] {
        chord
            .observe(key(usage), KeyEdge::Down, MonoTime(10))
            .unwrap();
    }
    chord
}
#[test]
fn threshold_is_exact_and_fires_once_per_terminal_session() {
    let mut chord = chord();
    assert!(!chord.advance(MonoTime(HOLD_NS + 9)).unwrap());
    assert!(chord.advance(MonoTime(HOLD_NS + 10)).unwrap());
    assert!(!chord.advance(MonoTime(HOLD_NS + 11)).unwrap());
    chord
        .observe(key(0x45), KeyEdge::Up, MonoTime(HOLD_NS + 12))
        .unwrap();
    chord
        .observe(key(0x45), KeyEdge::Down, MonoTime(HOLD_NS + 13))
        .unwrap();
    assert_eq!(chord.next_deadline(), None);
}
#[test]
fn all_left_right_choices_and_overlapping_modifiers_keep_original_deadline() {
    for ctrl in [0xe0, 0xe4] {
        for alt in [0xe2, 0xe6] {
            for shift in [0xe1, 0xe5] {
                let mut chord = EmergencyChord::new();
                for usage in [ctrl, alt, shift, 0x45] {
                    chord
                        .observe(key(usage), KeyEdge::Down, MonoTime(0))
                        .unwrap();
                }
                assert!(chord.advance(MonoTime(HOLD_NS)).unwrap());
            }
        }
    }
    let mut chord = chord();
    chord
        .observe(key(0xe4), KeyEdge::Down, MonoTime(20))
        .unwrap();
    chord.observe(key(0xe0), KeyEdge::Up, MonoTime(30)).unwrap();
    assert_eq!(chord.next_deadline(), Some(MonoTime(10 + HOLD_NS)));
}
#[test]
fn repeated_down_and_repeat_do_not_refresh_or_establish_missing_group() {
    let mut chord = chord();
    for (usage, edge) in [
        (0x45, KeyEdge::Repeat),
        (0xe0, KeyEdge::Down),
        (4, KeyEdge::Down),
        (4, KeyEdge::Up),
    ] {
        chord.observe(key(usage), edge, MonoTime(20)).unwrap();
    }
    assert_eq!(chord.next_deadline(), Some(MonoTime(HOLD_NS + 10)));
    for missing in [0xe0, 0xe2, 0xe1, 0x45] {
        let mut chord = EmergencyChord::new();
        for usage in [0xe0, 0xe2, 0xe1, 0x45] {
            chord
                .observe(
                    key(usage),
                    if usage == missing {
                        KeyEdge::Repeat
                    } else {
                        KeyEdge::Down
                    },
                    MonoTime(0),
                )
                .unwrap();
        }
        assert_eq!(chord.next_deadline(), None);
    }
}
#[test]
fn early_release_cancels_and_repress_requires_full_interval() {
    let mut chord = chord();
    chord
        .observe(key(0x45), KeyEdge::Up, MonoTime(HOLD_NS))
        .unwrap();
    assert!(!chord.advance(MonoTime(HOLD_NS + 10)).unwrap());
    chord
        .observe(key(0x45), KeyEdge::Down, MonoTime(HOLD_NS + 20))
        .unwrap();
    assert_eq!(chord.next_deadline(), Some(MonoTime(2 * HOLD_NS + 20)));
}
#[test]
fn clock_errors_are_atomic_and_other_usage_pages_do_not_form_chord() {
    let mut chord = chord();
    let before = chord.clone();
    assert_eq!(
        chord.observe(key(0x45), KeyEdge::Up, MonoTime(9)),
        Err(EmergencyError::ClockBackwards)
    );
    assert_eq!(chord, before);
    assert_eq!(
        chord.advance(MonoTime(9)),
        Err(EmergencyError::ClockBackwards)
    );
    assert_eq!(chord, before);
    let mut chord = EmergencyChord::new();
    for usage in [0xe0, 0xe2, 0xe1] {
        chord
            .observe(key(usage), KeyEdge::Down, MonoTime(u64::MAX))
            .unwrap();
    }
    let before = chord.clone();
    assert_eq!(
        chord.observe(key(0x45), KeyEdge::Down, MonoTime(u64::MAX)),
        Err(EmergencyError::TimeOverflow)
    );
    assert_eq!(chord, before);
    chord
        .observe(KeyId::new(0x0c, 0x45), KeyEdge::Down, MonoTime(u64::MAX))
        .unwrap();
    assert_eq!(chord.next_deadline(), None);
}
#[test]
fn hid_constants_are_resolved_by_the_actual_scan_codec() {
    use crate::keys::{HookRecord, IdentityPolicy, InjectionTag, Origin};
    for (usage, scan, flags) in [
        (0xe0, 0x1d, 0),
        (0xe4, 0x1d, 1),
        (0xe2, 0x38, 0),
        (0xe6, 0x38, 1),
        (0xe1, 0x2a, 0),
        (0xe5, 0x36, 0),
        (0x45, 0x58, 0),
    ] {
        let record = HookRecord {
            message: 0x100,
            vk_code: 0x41,
            scan_code: scan,
            flags,
            extra_info: 0,
        };
        assert_eq!(
            record.origin(InjectionTag::new(1).unwrap()),
            Origin::Physical
        );
        assert_eq!(
            crate::keys::resolve(record, IdentityPolicy::CanonicalScan)
                .unwrap()
                .key,
            key(usage)
        );
    }
}
