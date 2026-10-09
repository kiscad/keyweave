// Numeric facts reviewed 2026-10-09, Microsoft Scan Codes table.
// https://learn.microsoft.com/en-us/windows/win32/inputdev/about-keyboard-input#scan-codes
// CanonicalScan is not original HID evidence; aliases/special gates stay explicit.
use super::{Mapping, ScanId};
use keyweave_types::{KEYBOARD_PAGE, KeyId};
pub(super) const MAPPINGS: &[Mapping] = &[
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x04),
        scan: ScanId {
            code: 0x1e,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x05),
        scan: ScanId {
            code: 0x30,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x06),
        scan: ScanId {
            code: 0x2e,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x07),
        scan: ScanId {
            code: 0x20,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x08),
        scan: ScanId {
            code: 0x12,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x09),
        scan: ScanId {
            code: 0x21,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x0a),
        scan: ScanId {
            code: 0x22,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x0b),
        scan: ScanId {
            code: 0x23,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x0c),
        scan: ScanId {
            code: 0x17,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x0d),
        scan: ScanId {
            code: 0x24,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x0e),
        scan: ScanId {
            code: 0x25,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x0f),
        scan: ScanId {
            code: 0x26,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x10),
        scan: ScanId {
            code: 0x32,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x11),
        scan: ScanId {
            code: 0x31,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x12),
        scan: ScanId {
            code: 0x18,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x13),
        scan: ScanId {
            code: 0x19,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x14),
        scan: ScanId {
            code: 0x10,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x15),
        scan: ScanId {
            code: 0x13,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x16),
        scan: ScanId {
            code: 0x1f,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x17),
        scan: ScanId {
            code: 0x14,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x18),
        scan: ScanId {
            code: 0x16,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x19),
        scan: ScanId {
            code: 0x2f,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x1a),
        scan: ScanId {
            code: 0x11,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x1b),
        scan: ScanId {
            code: 0x2d,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x1c),
        scan: ScanId {
            code: 0x15,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x1d),
        scan: ScanId {
            code: 0x2c,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x1e),
        scan: ScanId {
            code: 0x02,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x1f),
        scan: ScanId {
            code: 0x03,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x20),
        scan: ScanId {
            code: 0x04,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x21),
        scan: ScanId {
            code: 0x05,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x22),
        scan: ScanId {
            code: 0x06,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x23),
        scan: ScanId {
            code: 0x07,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x24),
        scan: ScanId {
            code: 0x08,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x25),
        scan: ScanId {
            code: 0x09,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x26),
        scan: ScanId {
            code: 0x0a,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x27),
        scan: ScanId {
            code: 0x0b,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x28),
        scan: ScanId {
            code: 0x1c,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x29),
        scan: ScanId {
            code: 0x01,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x2a),
        scan: ScanId {
            code: 0x0e,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x2b),
        scan: ScanId {
            code: 0x0f,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x2c),
        scan: ScanId {
            code: 0x39,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x2d),
        scan: ScanId {
            code: 0x0c,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x2e),
        scan: ScanId {
            code: 0x0d,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x2f),
        scan: ScanId {
            code: 0x1a,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x30),
        scan: ScanId {
            code: 0x1b,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x31),
        scan: ScanId {
            code: 0x2b,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x32),
        scan: ScanId {
            code: 0x2b,
            extended: false,
        },
        canonical: false,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x33),
        scan: ScanId {
            code: 0x27,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x34),
        scan: ScanId {
            code: 0x28,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x35),
        scan: ScanId {
            code: 0x29,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x36),
        scan: ScanId {
            code: 0x33,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x37),
        scan: ScanId {
            code: 0x34,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x38),
        scan: ScanId {
            code: 0x35,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x39),
        scan: ScanId {
            code: 0x3a,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x3a),
        scan: ScanId {
            code: 0x3b,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x3b),
        scan: ScanId {
            code: 0x3c,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x3c),
        scan: ScanId {
            code: 0x3d,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x3d),
        scan: ScanId {
            code: 0x3e,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x3e),
        scan: ScanId {
            code: 0x3f,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x3f),
        scan: ScanId {
            code: 0x40,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x40),
        scan: ScanId {
            code: 0x41,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x41),
        scan: ScanId {
            code: 0x42,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x42),
        scan: ScanId {
            code: 0x43,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x43),
        scan: ScanId {
            code: 0x44,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x44),
        scan: ScanId {
            code: 0x57,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x45),
        scan: ScanId {
            code: 0x58,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x46),
        scan: ScanId {
            code: 0x37,
            extended: true,
        },
        canonical: true,
        special: true,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x47),
        scan: ScanId {
            code: 0x46,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x48),
        scan: ScanId {
            code: 0x45,
            extended: false,
        },
        canonical: true,
        special: true,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x49),
        scan: ScanId {
            code: 0x52,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x4a),
        scan: ScanId {
            code: 0x47,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x4b),
        scan: ScanId {
            code: 0x49,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x4c),
        scan: ScanId {
            code: 0x53,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x4d),
        scan: ScanId {
            code: 0x4f,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x4e),
        scan: ScanId {
            code: 0x51,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x4f),
        scan: ScanId {
            code: 0x4d,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x50),
        scan: ScanId {
            code: 0x4b,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x51),
        scan: ScanId {
            code: 0x50,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x52),
        scan: ScanId {
            code: 0x48,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x53),
        scan: ScanId {
            code: 0x45,
            extended: false,
        },
        canonical: true,
        special: true,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x54),
        scan: ScanId {
            code: 0x35,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x55),
        scan: ScanId {
            code: 0x37,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x56),
        scan: ScanId {
            code: 0x4a,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x57),
        scan: ScanId {
            code: 0x4e,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x58),
        scan: ScanId {
            code: 0x1c,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x59),
        scan: ScanId {
            code: 0x4f,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x5a),
        scan: ScanId {
            code: 0x50,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x5b),
        scan: ScanId {
            code: 0x51,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x5c),
        scan: ScanId {
            code: 0x4b,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x5d),
        scan: ScanId {
            code: 0x4c,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x5e),
        scan: ScanId {
            code: 0x4d,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x5f),
        scan: ScanId {
            code: 0x47,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x60),
        scan: ScanId {
            code: 0x48,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x61),
        scan: ScanId {
            code: 0x49,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x62),
        scan: ScanId {
            code: 0x52,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x63),
        scan: ScanId {
            code: 0x53,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x64),
        scan: ScanId {
            code: 0x56,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x65),
        scan: ScanId {
            code: 0x5d,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x66),
        scan: ScanId {
            code: 0x5e,
            extended: true,
        },
        canonical: true,
        special: true,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x67),
        scan: ScanId {
            code: 0x59,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x68),
        scan: ScanId {
            code: 0x64,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x69),
        scan: ScanId {
            code: 0x65,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x6a),
        scan: ScanId {
            code: 0x66,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x6b),
        scan: ScanId {
            code: 0x67,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x6c),
        scan: ScanId {
            code: 0x68,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x6d),
        scan: ScanId {
            code: 0x69,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x6e),
        scan: ScanId {
            code: 0x6a,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x6f),
        scan: ScanId {
            code: 0x6b,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x70),
        scan: ScanId {
            code: 0x6c,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x71),
        scan: ScanId {
            code: 0x6d,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x72),
        scan: ScanId {
            code: 0x6e,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x73),
        scan: ScanId {
            code: 0x76,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x85),
        scan: ScanId {
            code: 0x7e,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x87),
        scan: ScanId {
            code: 0x73,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x88),
        scan: ScanId {
            code: 0x70,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x89),
        scan: ScanId {
            code: 0x7d,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x8a),
        scan: ScanId {
            code: 0x79,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x8b),
        scan: ScanId {
            code: 0x7b,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x8c),
        scan: ScanId {
            code: 0x5c,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x90),
        scan: ScanId {
            code: 0x72,
            extended: false,
        },
        canonical: true,
        special: true,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x91),
        scan: ScanId {
            code: 0x71,
            extended: false,
        },
        canonical: true,
        special: true,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x92),
        scan: ScanId {
            code: 0x78,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x93),
        scan: ScanId {
            code: 0x77,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0x94),
        scan: ScanId {
            code: 0x76,
            extended: false,
        },
        canonical: false,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe0),
        scan: ScanId {
            code: 0x1d,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe1),
        scan: ScanId {
            code: 0x2a,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe2),
        scan: ScanId {
            code: 0x38,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe3),
        scan: ScanId {
            code: 0x5b,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe4),
        scan: ScanId {
            code: 0x1d,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe5),
        scan: ScanId {
            code: 0x36,
            extended: false,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe6),
        scan: ScanId {
            code: 0x38,
            extended: true,
        },
        canonical: true,
        special: false,
    },
    Mapping {
        key: KeyId::new(KEYBOARD_PAGE, 0xe7),
        scan: ScanId {
            code: 0x5c,
            extended: true,
        },
        canonical: true,
        special: false,
    },
];
