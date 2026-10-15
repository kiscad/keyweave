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
