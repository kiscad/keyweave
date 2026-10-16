#![allow(clippy::unwrap_used)]
use keyweave_ui::{
    devices::{DeviceEdit, Row},
    devices_view::DevicesPane,
    model::{self, Target},
    view::SettingsApp,
    visual::VisualDraft,
};
use serde_json::{Value, json};
const SOURCE: &str = r#"// retained values
{"custom_root":true,"profiles":[{"selected":true,"devices":[{"identifiers":{"vendor_id":10,"product_id":20,"is_keyboard":true,"device_address":"keep","custom_identifier":[1,2]},"ignore":true,"custom_device":{"keep":1},"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]},{"from":{"key_code":"c","modifiers":{"mandatory":["shift"]}},"to":[{"key_code":"d"}]}]}],"simple_modifications":[{"from":{"key_code":"x"},"to":[{"key_code":"y"}]}],"complex_modifications":{"rules":[]}},{"name":"other","devices":[]}]}"#;
const CLEAN: &str = r#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}],"devices":[{"identifiers":{"vendor_id":10,"product_id":20,"is_keyboard":true,"is_pointing_device":false},"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"c"}]}]}]}]}"#;
fn value(draft: &VisualDraft, source: &str) -> Value {
    serde_json::from_str(&draft.edited_json(source).unwrap()).unwrap()
}
#[test]
fn device_commit_preserves_unknown_hardware_advanced_and_unrelated_profile_values() {
    let mut draft = VisualDraft::parse(SOURCE).unwrap();
    let before = value(&draft, SOURCE);
    let mut edit = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
    assert!(!edit.pending());
    edit.commit(&mut draft).unwrap();
    assert!(!draft.dirty());
    edit.selector.vendor = "0xFFFFFFFF".into();
    edit.selector.pointing = true;
    if let Row::Editable(row) = &mut edit.rows[0] {
        row.to = "escape".into();
    } else {
        panic!("editable row");
    }
    assert!(edit.remove_row(1).is_err());
    assert!(edit.pending());
    assert!(!draft.dirty());
    edit.commit(&mut draft).unwrap();
    let after = value(&draft, SOURCE);
    let a = &after["profiles"][0]["devices"][0];
    let b = &before["profiles"][0]["devices"][0];
    assert_eq!(a["identifiers"]["vendor_id"], u32::MAX);
    assert_eq!(a["identifiers"]["is_pointing_device"], true);
    for name in ["device_address", "custom_identifier"] {
        assert_eq!(a["identifiers"][name], b["identifiers"][name]);
    }
    for name in ["ignore", "custom_device"] {
        assert_eq!(a[name], b[name]);
    }
    assert_eq!(a["simple_modifications"][1], b["simple_modifications"][1]);
    assert_eq!(
        a["simple_modifications"][0]["to"],
        json!([{"key_code":"escape"}])
    );
    assert_eq!(after["profiles"][1], before["profiles"][1]);
    assert_eq!(
        after["profiles"][0]["simple_modifications"],
        before["profiles"][0]["simple_modifications"]
    );
    assert_eq!(after["custom_root"], true);
}
#[test]
fn invalid_rows_ids_and_stale_edit_delete_or_append_are_atomic() {
    let mut draft = VisualDraft::parse(CLEAN).unwrap();
    let before = draft.edited_json(CLEAN).unwrap();
    for bad in ["", "-1", "+2", "0x", "0x100000000", "4294967296", "1e2"] {
        let mut edit = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
        edit.selector.vendor = bad.into();
        assert!(edit.commit(&mut draft).is_err());
        assert_eq!(draft.edited_json(CLEAN).unwrap(), before);
        assert!(!draft.dirty());
    }
    let mut edit = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
    if let Row::Editable(row) = &mut edit.rows[0] {
        row.to = "not-a-key".into();
    }
    assert!(edit.commit(&mut draft).is_err());
    assert_eq!(draft.edited_json(CLEAN).unwrap(), before);
    let stale = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
    let old = draft.device(0, 0).unwrap().clone();
    let mut current = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
    current.selector.product = "0x30".into();
    current.commit(&mut draft).unwrap();
    let changed = draft.edited_json(CLEAN).unwrap();
    assert!(stale.commit(&mut draft).is_err());
    assert!(draft.delete_device(0, 0, &old).is_err());
    assert_eq!(draft.edited_json(CLEAN).unwrap(), changed);
    let first = DeviceEdit::open(&draft, 0, None).unwrap();
    let second = DeviceEdit::open(&draft, 0, None).unwrap();
    first.commit(&mut draft).unwrap();
    let changed = draft.edited_json(CLEAN).unwrap();
    assert!(second.commit(&mut draft).is_err());
    assert_eq!(draft.edited_json(CLEAN).unwrap(), changed);
    let selected = draft.device(0, 0).unwrap().clone();
    draft.delete_device(0, 0, &selected).unwrap();
    assert_eq!(draft.device_count(0), 1);
}
#[test]
fn shared_simple_and_device_limits_reject_without_truncation_and_release_budget_after_remove() {
    let row = json!({"from":{"key_code":"a"},"to":[{"key_code":"b"}]});
    let source=json!({"profiles":[{"simple_modifications":vec![row.clone();4095],"devices":[{"identifiers":{},"simple_modifications":[row.clone()]}]}]}).to_string();
    let mut draft = VisualDraft::parse(&source).unwrap();
    let mut edit = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
    edit.add_row().unwrap();
    assert!(edit.commit(&mut draft).is_err());
    assert!(!draft.dirty());
    assert!(draft.add_row(0, "a", "c").is_err());
    let mut edit = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
    edit.remove_row(0).unwrap();
    edit.commit(&mut draft).unwrap();
    draft.add_row(0, "a", "c").unwrap();
    assert_eq!(draft.row_count(0), 4096);
    let source = json!({"profiles":[{"devices":vec![json!({"identifiers":{}});256]}]}).to_string();
    let draft = VisualDraft::parse(&source).unwrap();
    assert!(DeviceEdit::open(&draft, 0, None).is_err());
    assert_eq!(draft.device_count(0), 256);
    for devices in [
        json!({}),
        json!([false]),
        json!([{"identifiers":{},"simple_modifications":{}}]),
        json!(vec![json!({"identifiers":{}}); 257]),
    ] {
        assert!(
            VisualDraft::parse(&json!({"profiles":[{"devices":devices}]}).to_string()).is_err()
        );
    }
}
#[test]
fn pending_device_buffers_participate_in_settings_unsaved_and_apply_source_fence() {
    let mut app = SettingsApp::default();
    app.editor.replace(CLEAN.into());
    app.editor.target = Target::LinuxKeyboard;
    app.editor.start_validation().unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.editor.pending() {
        app.editor.poll();
        assert!(std::time::Instant::now() < until);
        std::thread::yield_now();
    }
    assert!(app.editor.can_save());
    app.open_visual().unwrap();
    let pane = app.visual.as_mut().unwrap();
    pane.devices.open(&pane.draft, 0, Some(0)).unwrap();
    pane.devices.editor().unwrap().selector.product = "0x55".into();
    assert!(pane.pending() && !pane.draft.dirty());
    assert!(app.unsaved());
    assert!(app.apply_visual().is_err());
    let pane = app.visual.as_mut().unwrap();
    pane.devices.commit(&mut pane.draft).unwrap();
    assert!(!pane.pending() && pane.draft.dirty());
    app.apply_visual().unwrap();
    assert!(app.editor.dirty());
    assert!(!app.editor.can_save());
    assert!(app.editor.report().is_none());
    app.open_visual().unwrap();
    let pane = app.visual.as_mut().unwrap();
    pane.devices.open(&pane.draft, 0, None).unwrap();
    assert!(pane.pending());
    pane.devices.discard();
    assert!(!pane.pending());
    app.editor.text.push(' ');
    assert!(app.apply_visual().is_err());
}
#[test]
fn edited_device_config_uses_real_offline_compiler_and_retains_platform_capability_gates() {
    let mut draft = VisualDraft::parse(CLEAN).unwrap();
    let mut edit = DeviceEdit::open(&draft, 0, Some(0)).unwrap();
    edit.selector.product = "0x30".into();
    if let Row::Editable(row) = &mut edit.rows[0] {
        row.to = "escape".into();
    }
    edit.commit(&mut draft).unwrap();
    let source = draft.edited_json(CLEAN).unwrap();
    for target in Target::ALL {
        for staged in [false, true] {
            if target.mixed() && !staged {
                continue;
            }
            let report = model::validate(&source, target, staged);
            if matches!(target, Target::LinuxKeyboard | Target::LinuxMixed) {
                assert!(report.accepted, "{:?}", report.diagnostics);
                assert!(report.diagnostics.iter().any(|d| d.code == "KW-DEVICE-001"));
            } else {
                assert!(!report.accepted);
                assert!(
                    report
                        .diagnostics
                        .iter()
                        .any(|d| d.message.contains("reliable per-event device identity"))
                );
            }
        }
    }
}
#[test]
fn configured_device_panel_renders_advanced_rows_headlessly_without_mutating_draft() {
    let mut draft = VisualDraft::parse(SOURCE).unwrap();
    let mut pane = DevicesPane::default();
    pane.open(&draft, 0, Some(0)).unwrap();
    let mut notice = String::new();
    let context = eframe::egui::Context::default();
    let mut output = context.run_ui(Default::default(), |ui| {
        pane.contents(ui, &mut draft, 0, &mut notice)
    });
    assert!(
        !context
            .tessellate(output.shapes, output.pixels_per_point)
            .is_empty()
    );
    output.textures_delta.clear();
    assert!(!pane.pending());
    assert!(!draft.dirty());
    assert!(notice.is_empty());
}
