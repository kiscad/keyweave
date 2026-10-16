#![allow(clippy::unwrap_used)]
use keyweave_ui::{model, view::SettingsApp, visual::VisualDraft};
use serde_json::{Value, json};
const SOURCE: &str = r#"// comment
{"unknown":{"keep":[1,true]},"profiles":[{"name":"First","selected":true,"complex_modifications":{"rules":[]},"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]},{"from":{"key_code":"c","modifiers":{"mandatory":["shift"]}},"to":[{"key_code":"d"}]}]},{"name":"Second","selected":false,"custom":42}]}"#;
#[test]
fn visual_changes_preserve_unknown_fields_advanced_rules_and_other_profiles() {
    let original = keyweave_config::parse_jsonc(SOURCE).unwrap();
    let mut draft = VisualDraft::parse(SOURCE).unwrap();
    assert!(!draft.dirty());
    assert_eq!(draft.active(), 0);
    assert!(draft.row(0, 1).is_none());
    draft.set_row(0, 0, "caps_lock", "escape").unwrap();
    draft.rename(0, "工作").unwrap();
    draft.select(1).unwrap();
    draft.add_row(1, "button1", "button2").unwrap();
    let value: Value = serde_json::from_str(&draft.edited_json(SOURCE).unwrap()).unwrap();
    assert_eq!(value["unknown"], original["unknown"]);
    assert_eq!(
        value["profiles"][0]["complex_modifications"],
        original["profiles"][0]["complex_modifications"]
    );
    assert_eq!(
        value["profiles"][0]["simple_modifications"][1],
        original["profiles"][0]["simple_modifications"][1]
    );
    assert_eq!(
        value["profiles"][0]["simple_modifications"][0]["from"],
        json!({"key_code":"caps_lock"})
    );
    assert_eq!(value["profiles"][1]["custom"], 42);
    assert_eq!(value["profiles"][0]["selected"], false);
    assert_eq!(value["profiles"][1]["selected"], true);
    assert!(draft.dirty());
    assert_eq!(draft.add_profile().unwrap(), 2);
    assert_eq!(draft.active(), 1);
}
#[test]
fn invalid_edits_are_atomic_and_stale_documents_cannot_be_overwritten() {
    let mut draft = VisualDraft::parse(SOURCE).unwrap();
    let before = draft.edited_json(SOURCE).unwrap();
    assert!(draft.set_row(0, 1, "a", "b").is_err());
    assert!(draft.set_row(0, 0, "a", "not-a-key").is_err());
    assert!(draft.add_row(0, "a", "not-a-key").is_err());
    assert!(draft.select(999).is_err());
    assert!(draft.rename(0, " ").is_err());
    assert!(draft.rename(0, &"名".repeat(257)).is_err());
    assert!(draft.remove_row(999, 0).is_err());
    assert_eq!(draft.edited_json(SOURCE).unwrap(), before);
    assert!(!draft.dirty());
    assert!(draft.edited_json(&(SOURCE.to_owned() + " ")).is_err());
    draft.remove_row(0, 0).unwrap();
    assert_eq!(draft.row_count(0), 1);
    assert!(draft.row(0, 0).is_none());
}
#[test]
fn visual_bounds_and_strict_jsonc_reject_without_truncating() {
    assert!(VisualDraft::parse(r#"{"profiles":[],"profiles":[]}"#).is_err());
    assert!(VisualDraft::parse(r#"{"profiles":[{"simple_modifications":{}}]}"#).is_err());
    let profile = json!({"name":"p"});
    let source = json!({"profiles":vec![profile;128]}).to_string();
    let mut draft = VisualDraft::parse(&source).unwrap();
    assert!(draft.add_profile().is_err());
    assert!(!draft.dirty());
    let rows = vec![json!({"from":{"key_code":"a"},"to":[{"key_code":"b"}]}); 4096];
    let source = json!({"profiles":[{"simple_modifications":rows}]}).to_string();
    let mut draft = VisualDraft::parse(&source).unwrap();
    assert!(draft.add_row(0, "a", "b").is_err());
    assert_eq!(draft.row_count(0), 4096);
    draft.remove_row(0, 4095).unwrap();
    draft.add_row(0, "a", "c").unwrap();
    assert_eq!(draft.row(0, 4095).unwrap().to, "c");
}
#[test]
fn applying_visual_draft_keeps_saved_baseline_and_revokes_validation() {
    let mut app = SettingsApp::default();
    app.editor.replace(model::TEMPLATE.into());
    app.editor.start_validation().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.editor.pending() {
        assert!(std::time::Instant::now() < deadline);
        app.editor.poll();
        std::thread::yield_now();
    }
    assert!(app.editor.can_save());
    app.open_visual().unwrap();
    assert!(!app.unsaved());
    assert!(app.open_visual().is_err());
    app.visual
        .as_mut()
        .unwrap()
        .draft
        .add_row(0, "a", "b")
        .unwrap();
    assert!(app.unsaved());
    app.apply_visual().unwrap();
    assert!(app.editor.dirty());
    assert!(!app.editor.can_save());
    assert!(app.editor.report().is_none());
    assert!(app.visual.is_none());
    app.open_visual().unwrap();
    app.editor.text.push(' ');
    let text = app.editor.text.clone();
    assert!(app.apply_visual().is_err());
    assert_eq!(app.editor.text, text);
    assert!(app.visual.is_some());
}
#[test]
fn visual_panel_layout_renders_advanced_rows_without_runtime() {
    let mut app = SettingsApp::default();
    app.editor.replace(SOURCE.into());
    app.open_visual().unwrap();
    let context = eframe::egui::Context::default();
    let mut output = context.run_ui(Default::default(), |ui| app.contents(ui));
    assert!(
        !context
            .tessellate(output.shapes, output.pixels_per_point)
            .is_empty()
    );
    output.textures_delta.clear();
    assert!(!app.editor.dirty());
    assert!(!app.visual.as_ref().unwrap().draft.dirty());
}

#[test]
fn uncommitted_global_row_marks_settings_unsaved_and_cannot_be_lost_by_apply_or_profile_switch() {
    let mut app = SettingsApp::default();
    app.editor.replace(SOURCE.into());
    app.open_visual().unwrap();
    let pane = app.visual.as_mut().unwrap();
    pane.row_input(0).unwrap().to = "escape".into();
    assert!(pane.pending());
    assert!(!pane.draft.dirty());
    assert!(pane.browse(1).is_err());
    assert!(app.unsaved());
    assert!(app.apply_visual().is_err());
    assert_eq!(app.editor.text, SOURCE);
    let pane = app.visual.as_mut().unwrap();
    pane.commit_row(0).unwrap();
    assert!(!pane.pending());
    assert!(pane.draft.dirty());
    app.apply_visual().unwrap();
    let after: Value = serde_json::from_str(&app.editor.text).unwrap();
    let before = keyweave_config::parse_jsonc(SOURCE).unwrap();
    assert_eq!(
        after["profiles"][0]["simple_modifications"][0]["to"],
        json!([{"key_code":"escape"}])
    );
    assert_eq!(
        after["profiles"][0]["simple_modifications"][1],
        before["profiles"][0]["simple_modifications"][1]
    );
    assert_eq!(after["unknown"], before["unknown"]);
    assert_eq!(after["profiles"][1], before["profiles"][1]);
}
#[test]
fn invalid_or_stale_row_update_keeps_input_and_never_overwrites_the_current_raw_value() {
    use keyweave_ui::visual_view::VisualPane;
    let mut pane = VisualPane::new(SOURCE).unwrap();
    let before = pane.draft.edited_json(SOURCE).unwrap();
    pane.row_input(0).unwrap().to = "not-a-key".into();
    assert!(pane.commit_row(0).is_err());
    assert!(pane.pending());
    assert!(!pane.draft.dirty());
    assert_eq!(pane.draft.edited_json(SOURCE).unwrap(), before);
    assert!(pane.remove_mapping(0).is_err());
    pane.discard_row(0);
    assert!(!pane.pending());
    pane.row_input(0).unwrap().to = "escape".into();
    pane.draft.set_row(0, 0, "x", "y").unwrap();
    let changed = pane.draft.edited_json(SOURCE).unwrap();
    assert!(pane.commit_row(0).is_err());
    assert_eq!(pane.draft.edited_json(SOURCE).unwrap(), changed);
    assert_eq!(pane.row_input(0).unwrap().to, "escape");
    pane.draft.remove_row(0, 0).unwrap();
    assert!(pane.commit_row(0).is_err());
    assert!(pane.row_input(0).is_err());
    assert!(pane.pending());
    pane.discard_simple_inputs();
    assert!(!pane.pending());
    assert!(pane.browse(1).is_ok());
}
#[test]
fn name_and_new_mapping_buffers_require_completion_and_name_fence_rejects_external_change() {
    use keyweave_ui::visual_view::VisualPane;
    let mut pane = VisualPane::new(SOURCE).unwrap();
    *pane.name_input() = "工作".into();
    assert!(pane.pending());
    assert!(pane.browse(1).is_err());
    pane.draft.rename(0, "external").unwrap();
    assert!(pane.commit_name().is_err());
    assert_eq!(pane.name_input(), "工作");
    assert_eq!(pane.draft.profile_name(0), "external");
    pane.discard_simple_inputs();
    assert_eq!(pane.name_input(), "external");
    assert!(!pane.pending());
    *pane.name_input() = "New name".into();
    pane.commit_name().unwrap();
    assert!(!pane.pending());
    let (from, to) = pane.new_mapping_inputs();
    *from = "caps_lock".into();
    *to = "escape".into();
    assert!(pane.pending());
    assert!(pane.browse(1).is_err());
    pane.add_mapping().unwrap();
    assert!(!pane.pending());
    assert_eq!(pane.draft.row(0, 2).unwrap().to, "escape");
    pane.browse(1).unwrap();
    let (from, _) = pane.new_mapping_inputs();
    *from = "not-a-key".into();
    assert!(pane.add_mapping().is_err());
    assert!(pane.pending());
    assert_eq!(pane.draft.row_count(1), 0);
    pane.discard_simple_inputs();
    assert!(!pane.pending());
    pane.draft.rename(1, "external second").unwrap();
    assert_eq!(pane.name_input(), "external second");
    assert!(!pane.pending());
}
#[test]
fn completing_one_row_does_not_clear_another_and_structural_remove_requires_no_pending_inputs() {
    use keyweave_ui::visual_view::VisualPane;
    let source=json!({"profiles":[{"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]},{"from":{"key_code":"c"},"to":[{"key_code":"d"}]}]}]}).to_string();
    let mut pane = VisualPane::new(&source).unwrap();
    pane.row_input(0).unwrap().to = "x".into();
    pane.row_input(1).unwrap().to = "y".into();
    pane.commit_row(0).unwrap();
    assert!(pane.pending());
    assert_eq!(pane.row_input(1).unwrap().to, "y");
    assert!(pane.remove_mapping(0).is_err());
    assert_eq!(pane.draft.row_count(0), 2);
    pane.discard_row(1);
    assert!(!pane.pending());
    pane.remove_mapping(0).unwrap();
    assert_eq!(pane.row_input(0).unwrap().to, "d");
    pane.commit_row(0).unwrap();
    assert!(!pane.pending());
    assert_eq!(pane.draft.row_count(0), 1);
}
#[test]
fn headless_redraw_does_not_rebase_or_commit_pending_global_inputs() {
    use keyweave_ui::visual_view::VisualPane;
    let mut pane = VisualPane::new(SOURCE).unwrap();
    let before = pane.draft.edited_json(SOURCE).unwrap();
    pane.row_input(0).unwrap().to = "escape".into();
    let context = eframe::egui::Context::default();
    let mut notice = String::new();
    for _ in 0..2 {
        let mut output = context.run_ui(Default::default(), |ui| {
            pane.contents(ui, &mut notice);
        });
        assert!(
            !context
                .tessellate(output.shapes, output.pixels_per_point)
                .is_empty()
        );
        output.textures_delta.clear();
    }
    assert!(pane.pending());
    assert!(!pane.draft.dirty());
    assert_eq!(pane.draft.edited_json(SOURCE).unwrap(), before);
    assert_eq!(pane.row_input(0).unwrap().to, "escape");
    pane.commit_row(0).unwrap();
    assert!(!pane.pending());
}

#[test]
fn keyboard_focus_survives_the_pending_notice_appearing_between_frames() {
    use keyweave_ui::visual_view::VisualPane;
    let mut base = keyweave_config::parse_jsonc(SOURCE).unwrap();
    base["profiles"][0]["devices"] =
        json!([{"identifiers":{"vendor_id":10,"product_id":20},"simple_modifications":[]}]);
    let source = base.to_string();
    for target in 0..4 {
        let mut pane = VisualPane::new(&source).unwrap();
        if target == 3 {
            pane.devices.open(&pane.draft, 0, Some(0)).unwrap();
        }
        let context = eframe::egui::Context::default();
        let mut notice = String::new();
        let mut output = context.run_ui(Default::default(), |ui| {
            pane.contents(ui, &mut notice);
        });
        output.textures_delta.clear();
        let id = match target {
            0 => eframe::egui::Id::new(("visual-profile-name", 0usize)),
            1 => eframe::egui::Id::new(("visual-simple-to", 0usize, 0usize)),
            2 => eframe::egui::Id::new(("visual-new-from", 0usize)),
            _ => eframe::egui::Id::new(("device-vendor", (0usize, 0usize))),
        };
        context.memory_mut(|memory| memory.request_focus(id));
        for text in ["x", "y"] {
            let input = eframe::egui::RawInput {
                events: vec![eframe::egui::Event::Text(text.into())],
                ..Default::default()
            };
            let mut output = context.run_ui(input, |ui| {
                pane.contents(ui, &mut notice);
            });
            output.textures_delta.clear();
            assert_eq!(
                context.memory(|memory| memory.focused()),
                Some(id),
                "target {target}"
            );
        }
        let text = match target {
            0 => pane.name_input().clone(),
            1 => pane.row_input(0).unwrap().to.clone(),
            2 => pane.new_mapping_inputs().0.clone(),
            _ => pane.devices.editor().unwrap().selector.vendor.clone(),
        };
        assert!(text.ends_with("xy"), "target {target}: {text}");
        assert!(pane.pending());
        assert!(!pane.draft.dirty());
    }
}
