#![allow(clippy::unwrap_used)]
use super::*;
use crate::{devices_view::DevicesPane, visual::VisualDraft};
use keyweave_linux::inventory::{Failure, Metadata, TypeFacts};
use std::sync::mpsc;
fn pane() -> InventoryPane {
    InventoryPane::with_gate(Arc::new(AtomicBool::new(false)))
}
fn snapshot() -> Snapshot {
    Snapshot {
        schema_version: 1,
        scope: "canonical_evdev_nodes",
        scan_complete: false,
        probes_complete: false,
        directory_entries_seen: 2,
        issues: vec!["partial diagnostic".into()],
        entries: vec![
            Entry::Observed {
                path: "/dev/input/event2".into(),
                metadata: Metadata {
                    bus: 3,
                    vendor_id: 123,
                    product_id: 456,
                    version: 1,
                    device_number: (13, 66),
                    name: Some("Keyboard\u{1b}\"".into()),
                    name_was_lossy: true,
                    types: Some(TypeFacts {
                        is_keyboard: true,
                        is_pointing_device: false,
                    }),
                    advertised_key_count: 104,
                    keyboard_capability_issues: vec![],
                    keyboard_issues_truncated: false,
                    relative_capability_issues: vec![],
                    relative_issues_truncated: false,
                },
            },
            Entry::Unavailable {
                path: "/dev/input/event3".into(),
                failure: Failure {
                    kind: "PermissionDenied".into(),
                    os_code: Some(13),
                },
            },
        ],
    }
}
fn loaded(now: Instant) -> InventoryPane {
    let mut pane = pane();
    pane.observation = Some(Observation {
        started: now,
        snapshot: snapshot(),
    });
    pane
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "test worker did not finish");
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn selector_requires_current_known_observation_and_does_not_guess_types() {
    let now = Instant::now();
    let mut pane = loaded(now);
    let selector = pane.selector(0, now).unwrap();
    assert_eq!(selector.vendor, "123");
    assert_eq!(selector.product, "456");
    assert!(selector.keyboard);
    assert!(!selector.pointing);
    // Partial inventory permits known individual metadata, not failed nodes.
    assert!(pane.selector(1, now).is_err());
    assert!(pane.selector(2, now).is_err());
    assert!(
        pane.selector(0, now + MAX_AGE + Duration::from_nanos(1))
            .is_err()
    );
    assert!(pane.selector(0, now - Duration::from_nanos(1)).is_err());
    if let Entry::Observed { metadata, .. } =
        &mut pane.observation.as_mut().unwrap().snapshot.entries[0]
    {
        metadata.types = None;
    }
    assert!(pane.selector(0, now).unwrap_err().contains("Unknown"));
}
#[test]
fn refresh_invalidates_old_results_and_poll_waits_without_joining_active_scan() {
    let now = Instant::now();
    let mut pane = loaded(now);
    let (entered, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    pane.start(now, move || {
        entered.send(()).unwrap();
        blocked.recv().unwrap();
        snapshot()
    })
    .unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(pane.observation.is_none());
    pane.poll(); // Must return while scan is deliberately blocked.
    assert!(pane.job.is_some());
    assert!(pane.selector(0, now).is_err());
    assert!(pane.start(now, snapshot).is_err());
    release.send(()).unwrap();
    until(|| {
        pane.poll();
        pane.job.is_none()
    });
    assert_eq!(pane.selector(0, now).unwrap().vendor, "123");
}
#[test]
fn dropped_pane_retains_process_scan_gate_until_detached_task_finishes() {
    let gate = Arc::new(AtomicBool::new(false));
    let mut first = InventoryPane::with_gate(gate.clone());
    let mut second = InventoryPane::with_gate(gate.clone());
    second.observation = Some(Observation {
        started: Instant::now(),
        snapshot: snapshot(),
    });
    let (release, blocked) = mpsc::channel();
    first
        .start(Instant::now(), move || {
            blocked.recv().unwrap();
            snapshot()
        })
        .unwrap();
    drop(first);
    assert!(second.start(Instant::now(), snapshot).is_err());
    assert!(second.observation.is_none());
    release.send(()).unwrap();
    until(|| !gate.load(Ordering::Acquire));
    second.start(Instant::now(), snapshot).unwrap();
    until(|| {
        second.poll();
        second.job.is_none()
    });
    assert!(second.observation.is_some());
}
#[test]
fn failed_task_cannot_reveal_old_metadata_and_releases_scan_gate() {
    let now = Instant::now();
    let mut pane = loaded(now);
    pane.start(now, || panic!("synthetic scan failure"))
        .unwrap();
    until(|| {
        pane.poll();
        pane.job.is_none()
    });
    assert!(pane.error.is_some());
    assert!(pane.selector(0, now).is_err());
    assert!(!pane.gate.load(Ordering::Acquire));
    pane.start(now, snapshot).unwrap();
    until(|| {
        pane.poll();
        pane.job.is_none()
    });
    assert!(pane.error.is_none());
}
#[test]
fn selected_metadata_only_creates_pending_draft_and_preserves_append_fence() {
    let mut draft =
        VisualDraft::parse(r#"{"profiles":[{"name":"p","selected":true,"devices":[]}]}"#).unwrap();
    let now = Instant::now();
    let observation = loaded(now);
    let mut devices = DevicesPane::default();
    devices
        .generate_selector(&draft, 0, observation.selector(0, now).unwrap())
        .unwrap();
    assert!(devices.pending());
    assert_eq!(draft.device_count(0), 0);
    assert!(
        devices
            .generate_selector(&draft, 0, observation.selector(0, now).unwrap())
            .is_err()
    );
    assert_eq!(devices.editor().unwrap().selector.vendor, "123");
    devices.commit(&mut draft).unwrap();
    let value = draft.device(0, 0).unwrap();
    assert_eq!(value["identifiers"]["vendor_id"], 123);
    assert_eq!(value["identifiers"]["product_id"], 456);
    assert_eq!(value["identifiers"]["is_keyboard"], true);
    assert!(value.get("path").is_none());
    assert!(value.get("name").is_none());
    devices
        .generate_selector(&draft, 0, observation.selector(0, now).unwrap())
        .unwrap();
    let mut competing = DevicesPane::default();
    competing.open(&draft, 0, None).unwrap();
    competing.commit(&mut draft).unwrap();
    assert!(devices.commit(&mut draft).is_err());
    assert!(devices.pending());
    assert_eq!(draft.device_count(0), 2);
}
#[test]
fn headless_discovery_shows_partial_results_without_starting_a_scan() {
    let ctx = egui::Context::default();
    let mut pane = loaded(Instant::now());
    let mut notice = String::new();
    let mut output = ctx.run_ui(Default::default(), |ui| {
        assert!(pane.contents(ui, true, &mut notice).is_none());
    });
    assert!(!output.shapes.is_empty());
    output.textures_delta.clear();
    assert!(pane.job.is_none());
    assert!(!pane.gate.load(Ordering::Acquire));
    assert!(notice.is_empty());
}
