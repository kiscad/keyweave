#![allow(clippy::unwrap_used)]
use super::*;
fn metadata() -> Metadata {
    Metadata {
        bus: 3,
        vendor_id: 1,
        product_id: 2,
        version: 1,
        device_number: (13, 64),
        name: Some("synthetic".into()),
        name_was_lossy: false,
        types: None,
        advertised_key_count: 1,
        keyboard_capability_issues: vec![],
        keyboard_issues_truncated: false,
        relative_capability_issues: vec![],
        relative_issues_truncated: false,
    }
}
#[test]
fn canonical_candidates_sort_numerically_and_partial_failures_remain_separate() {
    let names = [
        "event10",
        "mouse0",
        "event2",
        "event01",
        "event-1",
        "event4294967296",
        "event",
        "by-id",
    ]
    .map(|s| Ok(OsString::from(s)));
    let mut called = vec![];
    let result = scan_with(
        names,
        |path| {
            called.push(path.to_owned());
            if path.ends_with("event2") {
                Err(io::Error::from_raw_os_error(13))
            } else {
                Ok(metadata())
            }
        },
        || Duration::ZERO,
    );
    assert!(result.scan_complete);
    assert!(!result.probes_complete);
    assert_eq!(
        called,
        vec![
            PathBuf::from("/dev/input/event2"),
            PathBuf::from("/dev/input/event10")
        ]
    );
    assert!(
        matches!(&result.entries[0],Entry::Unavailable{failure,..}if failure.os_code==Some(13))
    );
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(
        json["entries"][1]["metadata"]["types"],
        serde_json::Value::Null
    );
    assert_eq!(json["scope"], "canonical_evdev_nodes");
}
#[test]
fn limits_duplicates_and_directory_errors_never_claim_complete_or_probe_twice() {
    let names = (0..257).map(|n| Ok(OsString::from(format!("event{n}"))));
    let mut count = 0;
    let result = scan_with(
        names,
        |_| {
            count += 1;
            Ok(metadata())
        },
        || Duration::ZERO,
    );
    assert!(!result.scan_complete);
    assert_eq!(count, 256);
    let result = scan_with(
        (0..1025).map(|_| Ok(OsString::from("mouse0"))),
        |_| panic!("not event nodes"),
        || Duration::ZERO,
    );
    assert!(!result.scan_complete);
    assert_eq!(result.directory_entries_seen, 1024);
    let mut count = 0;
    let result = scan_with(
        [
            Ok(OsString::from("event2")),
            Err(io::Error::from_raw_os_error(5)),
            Ok(OsString::from("event2")),
        ],
        |_| {
            count += 1;
            Ok(metadata())
        },
        || Duration::ZERO,
    );
    assert_eq!(count, 1);
    assert!(!result.scan_complete);
    assert!(result.issues.iter().any(|v| v == "duplicate_candidate"));
}
#[test]
fn deadline_regression_overflow_and_slow_probe_do_not_extend_budget() {
    let mut ticks = [Duration::ZERO, Duration::from_secs(2)].into_iter();
    let result = scan_with(
        [Ok(OsString::from("event1"))],
        |_| panic!("expired"),
        || ticks.next().unwrap(),
    );
    assert!(!result.scan_complete);
    let result = scan_with([], |_| panic!("overflow"), || Duration::MAX);
    assert!(!result.scan_complete && !result.probes_complete);
    let mut ticks = [Duration::from_secs(1), Duration::ZERO].into_iter();
    let result = scan_with(
        [Ok(OsString::from("event1"))],
        |_| panic!("backwards"),
        || ticks.next().unwrap(),
    );
    assert!(!result.scan_complete);
    let mut ticks = [
        Duration::ZERO,
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(2),
    ]
    .into_iter();
    let result = scan_with(
        [Ok(OsString::from("event1"))],
        |_| Ok(metadata()),
        || ticks.next().unwrap(),
    );
    assert_eq!(result.entries.len(), 1);
    assert!(!result.probes_complete);
}

#[test]
fn metadata_output_escapes_names_bounds_diagnostics_and_keeps_unknown_classification() {
    use crate::{
        device::DeviceInspection,
        events::KEY_MAX,
        probe::{Bits, Capabilities, DeviceId, Probe},
    };
    let mut events = Bits::empty(32).unwrap();
    events.set(1).unwrap();
    let mut keys = Bits::empty(usize::from(KEY_MAX) + 1).unwrap();
    for key in 0..=KEY_MAX {
        keys.set(key).unwrap();
    }
    let observation = DeviceInspection {
        probe: Probe {
            id: DeviceId {
                bus: 3,
                vendor: 1,
                product: 2,
                version: 1,
            },
            name: Some(vec![0x1b, b'[', b'1', 0xff]),
            input_syspath: "/synthetic/input0".into(),
            capabilities: Capabilities {
                events,
                keys,
                relative: Bits::empty(16).unwrap(),
                misc: Bits::empty(8).unwrap(),
                leds: Bits::empty(16).unwrap(),
                properties: Bits::empty(32).unwrap(),
            },
        },
        device_types: None,
        device_number: (13, 64),
    };
    let data = Metadata::from(observation);
    assert!(data.name_was_lossy);
    assert!(data.types.is_none());
    assert!(data.keyboard_issues_truncated);
    assert_eq!(data.keyboard_capability_issues.len(), 16);
    let text = serde_json::to_string(&data).unwrap();
    assert!(!text.as_bytes().contains(&0x1b));
    assert!(text.contains("\\u001b"));
    let mut times = [
        Duration::ZERO,
        Duration::from_millis(1),
        Duration::ZERO,
        Duration::from_millis(2),
    ]
    .into_iter();
    let result = scan_with(
        [Ok(OsString::from("event1")), Ok(OsString::from("event2"))],
        |_| panic!("clock failure must latch"),
        || times.next().unwrap(),
    );
    assert!(!result.scan_complete && !result.probes_complete);
    assert!(result.entries.is_empty());
}
