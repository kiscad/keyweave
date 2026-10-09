#![allow(clippy::unwrap_used)]
use super::*;
use keyweave_types::MonoTime;
use std::collections::BTreeSet;

fn key(usage: u16) -> KeyId {
    KeyId::new(KEYBOARD_PAGE, usage)
}
fn tag() -> InjectionTag {
    InjectionTag::new(0x1234).unwrap()
}
fn record(scan: u32, extended: bool, up: bool) -> HookRecord {
    HookRecord {
        message: if up { 0x101 } else { 0x100 },
        vk_code: 0x41,
        scan_code: scan,
        flags: u32::from(extended) | if up { UP } else { 0 },
        extra_info: 0,
    }
}
fn identity(usage: u16) -> ResolvedKey {
    ResolvedKey {
        key: key(usage),
        known_aliases: false,
    }
}

#[test]
fn ordinary_table_has_unique_canonical_scans_and_roundtrips() {
    let mut keys = BTreeSet::new();
    let mut scans = BTreeSet::new();
    assert_eq!(mappings().len(), 132);
    for entry in mappings() {
        assert!(keys.insert((entry.key.page, entry.key.usage)));
        if !entry.canonical || entry.special {
            assert!(output_scan(entry.key).is_err());
            continue;
        }
        assert!(scans.insert((entry.scan.code(), entry.scan.extended())));
        assert_eq!(output_scan(entry.key).unwrap(), entry.scan);
        let resolved = resolve(
            record(u32::from(entry.scan.code()), entry.scan.extended(), false),
            IdentityPolicy::CanonicalScan,
        )
        .unwrap();
        assert_eq!(resolved.key, entry.key);
    }
}

#[test]
fn extended_bit_preserves_modifiers_keypad_and_navigation() {
    for (scan, extended, usage) in [
        (0x1d, false, 0xe0),
        (0x1d, true, 0xe4),
        (0x38, false, 0xe2),
        (0x38, true, 0xe6),
        (0x2a, false, 0xe1),
        (0x36, false, 0xe5),
        (0x1c, false, 0x28),
        (0x1c, true, 0x58),
        (0x47, false, 0x5f),
        (0x47, true, 0x4a),
        (0x53, false, 0x63),
        (0x53, true, 0x4c),
        (0x5c, false, 0x8c),
        (0x5c, true, 0xe7),
    ] {
        assert_eq!(
            resolve(record(scan, extended, false), IdentityPolicy::StrictUnique).unwrap(),
            identity(usage)
        );
    }
    assert_eq!(
        resolve(record(0x36, true, false), IdentityPolicy::CanonicalScan),
        Err(KeyError::Native)
    );
}

#[test]
fn saved_binding_survives_vk_changes_repeat_and_system_messages() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalScan);
    assert_eq!(
        normalizer
            .normalize(record(0x1e, false, false), tag())
            .unwrap(),
        Normalized::Key {
            identity: identity(4),
            edge: KeyEdge::Down
        }
    );
    let mut changed = record(0x1e, false, false);
    changed.message = 0x104;
    changed.flags = ALT_DOWN;
    changed.vk_code = 0x51;
    assert_eq!(
        normalizer.normalize(changed, tag()).unwrap(),
        Normalized::Key {
            identity: identity(4),
            edge: KeyEdge::Repeat
        }
    );
    assert_eq!(normalizer.pressed(), 1);
    changed.message = 0x105;
    changed.flags |= UP;
    assert_eq!(
        normalizer.normalize(changed, tag()).unwrap(),
        Normalized::Key {
            identity: identity(4),
            edge: KeyEdge::Up
        }
    );
    assert_eq!(normalizer.pressed(), 0);
    assert_eq!(
        normalizer.normalize(changed, tag()).unwrap(),
        Normalized::OrphanUp
    );
}

#[test]
fn injection_filter_precedes_unicode_decoding_and_does_not_release_physical_key() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalScan);
    normalizer
        .normalize(record(0x1e, false, false), tag())
        .unwrap();
    let mut injected = record(0x1e, false, true);
    injected.flags |= INJECTED;
    injected.extra_info = tag().value();
    assert_eq!(
        normalizer.normalize(injected, tag()).unwrap(),
        Normalized::Injected(Origin::OwnInjected)
    );
    assert_eq!(normalizer.pressed(), 1);
    injected.extra_info += 1;
    injected.scan_code = 0xd800;
    injected.message = 0;
    assert_eq!(
        normalizer.normalize(injected, tag()).unwrap(),
        Normalized::Injected(Origin::OtherInjected)
    );
    injected.flags = LOWER_INJECTED;
    assert_eq!(
        normalizer.normalize(injected, tag()).unwrap(),
        Normalized::Injected(Origin::OtherInjected)
    );
    let mut physical = record(0x1e, false, true);
    physical.extra_info = tag().value();
    assert_eq!(physical.origin(tag()), Origin::Physical);
    assert!(matches!(
        normalizer.normalize(physical, tag()).unwrap(),
        Normalized::Key {
            edge: KeyEdge::Up,
            ..
        }
    ));
    assert_eq!(normalizer.pressed(), 0);
}

#[test]
fn invalid_physical_records_leave_saved_binding_intact() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalScan);
    normalizer
        .normalize(record(0x1e, false, false), tag())
        .unwrap();
    let valid = record(0x1e, false, true);
    for invalid in [
        HookRecord {
            message: 0x100,
            ..valid
        },
        HookRecord {
            flags: UP | 4,
            ..valid
        },
        HookRecord {
            vk_code: 0,
            ..valid
        },
        HookRecord {
            vk_code: 255,
            ..valid
        },
        HookRecord {
            scan_code: 0,
            ..valid
        },
        HookRecord {
            scan_code: 0xe01e,
            ..valid
        },
    ] {
        assert_eq!(normalizer.normalize(invalid, tag()), Err(KeyError::Record));
        assert_eq!(normalizer.pressed(), 1);
    }
    assert!(matches!(
        normalizer.normalize(valid, tag()).unwrap(),
        Normalized::Key {
            edge: KeyEdge::Up,
            ..
        }
    ));
}

#[test]
fn ambiguous_input_is_explicit_and_alias_output_is_refused() {
    for (scan, canonical, alias) in [(0x2b, 0x31, 0x32), (0x76, 0x73, 0x94)] {
        let native = record(scan, false, false);
        assert_eq!(
            resolve(native, IdentityPolicy::CanonicalScan).unwrap(),
            ResolvedKey {
                key: key(canonical),
                known_aliases: true
            }
        );
        assert_eq!(
            resolve(native, IdentityPolicy::StrictUnique),
            Err(KeyError::Ambiguous)
        );
        assert_eq!(output_scan(key(alias)), Err(KeyError::Ambiguous));
        let mut normalizer = KeyNormalizer::new(IdentityPolicy::StrictUnique);
        assert_eq!(
            normalizer.normalize(native, tag()),
            Err(KeyError::Ambiguous)
        );
        assert_eq!(normalizer.pressed(), 0);
    }
}

#[test]
fn special_sequences_are_gated_before_mutating_held_state() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalScan);
    for (scan, extended) in [
        (0x45, false),
        (0x45, true),
        (0x37, true),
        (0x54, false),
        (0x5e, true),
        (0x71, false),
        (0x72, false),
    ] {
        assert_eq!(
            normalizer.normalize(record(scan, extended, false), tag()),
            Err(KeyError::SpecialSequence)
        );
    }
    for vk_code in [0x03, 0x13, 0x2c] {
        assert_eq!(
            normalizer.normalize(
                HookRecord {
                    vk_code,
                    ..record(0x1e, false, false)
                },
                tag()
            ),
            Err(KeyError::SpecialSequence)
        );
    }
    for usage in [0x46, 0x48, 0x53, 0x66, 0x90, 0x91] {
        assert_eq!(output_scan(key(usage)), Err(KeyError::SpecialSequence));
    }
    assert_eq!(normalizer.pressed(), 0);
}

#[test]
fn reset_forgets_state_without_synthesizing_edges() {
    let mut normalizer = KeyNormalizer::new(IdentityPolicy::CanonicalScan);
    normalizer
        .normalize(record(0x1d, false, false), tag())
        .unwrap();
    normalizer
        .normalize(record(0x1d, true, false), tag())
        .unwrap();
    assert_eq!(normalizer.pressed(), 2);
    normalizer.reset();
    assert_eq!(normalizer.pressed(), 0);
    assert_eq!(
        normalizer
            .normalize(record(0x1d, true, true), tag())
            .unwrap(),
        Normalized::OrphanUp
    );
}

#[test]
fn packets_preserve_scan_flags_marker_and_repeat_semantics() {
    let plain = encode(key(4), KeyEdge::Down, tag()).unwrap();
    assert_eq!(
        plain,
        KeyboardPacket {
            vk: 0,
            scan: 0x1e,
            flags: 8,
            time: 0,
            extra_info: 0x1234
        }
    );
    assert_eq!(encode(key(4), KeyEdge::Repeat, tag()).unwrap(), plain);
    assert_eq!(
        encode(key(0xe4), KeyEdge::Up, tag()).unwrap(),
        KeyboardPacket {
            vk: 0,
            scan: 0x1d,
            flags: 11,
            time: 0,
            extra_info: 0x1234
        }
    );
    assert_eq!(
        encode(KeyId::new(0x0c, 0xe9), KeyEdge::Down, tag()),
        Err(KeyError::Hid)
    );
    assert_eq!(
        encode(key(0xffff), KeyEdge::Down, tag()),
        Err(KeyError::Hid)
    );
    assert_eq!(InjectionTag::new(0), Err(KeyError::Marker));
}

#[test]
fn batch_is_ordered_bounded_and_wholly_validated_before_return() {
    let edge = OutputIntent {
        at_ns: MonoTime(7),
        key: key(4),
        edge: KeyEdge::Down,
    };
    let mut batch = vec![edge.clone(); MAX_BATCH];
    assert_eq!(encode_batch(&batch, tag()).unwrap().len(), MAX_BATCH);
    batch.push(edge.clone());
    assert_eq!(encode_batch(&batch, tag()), Err(KeyError::Capacity));
    batch = vec![
        edge.clone(),
        OutputIntent {
            key: key(0x32),
            ..edge.clone()
        },
    ];
    assert_eq!(encode_batch(&batch, tag()), Err(KeyError::Ambiguous));
    batch[1] = OutputIntent {
        key: key(0xe4),
        edge: KeyEdge::Up,
        ..edge
    };
    let packets = encode_batch(&batch, tag()).unwrap();
    assert_eq!(packets[0].scan, 0x1e);
    assert_eq!((packets[1].scan, packets[1].flags), (0x1d, 11));
    assert!(encode_batch(&[], tag()).unwrap().is_empty());
}
