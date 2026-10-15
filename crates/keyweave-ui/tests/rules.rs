#![allow(clippy::unwrap_used)]
use keyweave_ui::{
    model,
    rules::{MAX_COMPLEX_RULES, RuleAsset},
    rules_view::RulesPane,
    view::SettingsApp,
    visual::VisualDraft,
};
use serde_json::{Value, json};
fn rule(name: &str) -> Value {
    json!({"description":name,"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"b"}]}]})
}
fn source() -> String {
    json!({"root_metadata":1,"profiles":[{"selected":true,"complex_modifications":{"parameters":{"basic.to_if_alone_timeout_milliseconds":300},"custom":"keep","rules":[rule("old")] }},{"name":"untouched","custom":[1,2]}]}).to_string()
}
#[test]
fn selected_asset_rules_append_losslessly_preserving_old_profiles_and_parameters() {
    let first = rule("first");
    let mut unsupported = rule("unsupported");
    unsupported["unknown"] = json!({"must":"survive"});
    let input=json!({"title":"library","maintainers":["someone"],"wrapper_unknown":true,"rules":[first,rule("skip"),unsupported]}).to_string();
    let asset = RuleAsset::parse(&input).unwrap();
    assert_eq!(asset.title(), "library");
    assert_eq!(asset.raw()["wrapper_unknown"], true);
    let source = source();
    let mut draft = VisualDraft::parse(&source).unwrap();
    assert_eq!(
        draft.import_rules(0, &asset, &[true, false, true]).unwrap(),
        2
    );
    let value = keyweave_config::parse_jsonc(&draft.edited_json(&source).unwrap()).unwrap();
    let before = keyweave_config::parse_jsonc(&source).unwrap();
    assert_eq!(value["profiles"][1], before["profiles"][1]);
    assert_eq!(
        value["profiles"][0]["complex_modifications"]["parameters"],
        before["profiles"][0]["complex_modifications"]["parameters"]
    );
    assert_eq!(
        value["profiles"][0]["complex_modifications"]["custom"],
        "keep"
    );
    assert_eq!(
        draft.rules(0),
        &[
            rule("old"),
            asset.rules()[0].clone(),
            asset.rules()[2].clone()
        ]
    );
    assert!(value.get("wrapper_unknown").is_none());
    let report = model::validate(
        &draft.edited_json(&source).unwrap(),
        model::Target::LinuxKeyboard,
        true,
    );
    assert!(!report.accepted);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.pointer.contains("/complex_modifications/rules/2/unknown"))
    );
}
#[test]
fn single_rule_enabled_state_and_unsupported_eval_js_survive_without_execution() {
    let asset=RuleAsset::parse(r#"{"description":"disabled","enabled":false,"eval_js":"throw new Error('must not run')","metadata":{"keep":true}}"#).unwrap();
    let mut draft = VisualDraft::parse(model::TEMPLATE).unwrap();
    draft.import_rules(0, &asset, &[true]).unwrap();
    assert_eq!(draft.rules(0)[0], asset.raw().clone());
    assert!(
        model::validate(
            &draft.edited_json(model::TEMPLATE).unwrap(),
            model::Target::LinuxKeyboard,
            true
        )
        .accepted
    );
    draft.set_rule_enabled(0, 0, true).unwrap();
    let report = model::validate(
        &draft.edited_json(model::TEMPLATE).unwrap(),
        model::Target::LinuxKeyboard,
        true,
    );
    assert!(!report.accepted);
    assert_eq!(draft.rules(0)[0]["eval_js"], asset.raw()["eval_js"]);
}
#[test]
fn malformed_assets_and_selection_or_capacity_errors_leave_draft_unchanged() {
    for text in [
        r#"{"title":"bad","rules":[{}]}"#,
        r#"{"title":"bad","rules":[]}"#,
        r#"{"title":1,"rules":[]}"#,
        r#"{"description":"bad","manipulators":[]}"#,
        r#"{"title":"bad","title":"duplicate","rules":[]}"#,
        r#"{"title":"bad","rules":[{"manipulators":[{}]},null]}"#,
    ] {
        assert!(RuleAsset::parse(text).is_err(), "{text}");
    }
    let asset = RuleAsset::parse(&rule("one").to_string()).unwrap();
    let source = source();
    let mut draft = VisualDraft::parse(&source).unwrap();
    let before = draft.edited_json(&source).unwrap();
    assert!(draft.import_rules(0, &asset, &[]).is_err());
    assert!(draft.import_rules(99, &asset, &[true]).is_err());
    assert!(draft.import_rules(0, &asset, &[false]).is_err());
    assert!(draft.move_rule(0, 0, 1).is_err());
    assert!(draft.remove_rule(0, 1).is_err());
    assert!(draft.set_rule_enabled(99, 0, false).is_err());
    assert_eq!(draft.edited_json(&source).unwrap(), before);
    assert!(!draft.dirty());
    let full=json!({"profiles":[{"complex_modifications":{"rules":vec![rule("full");MAX_COMPLEX_RULES]}}]}).to_string();
    let mut draft = VisualDraft::parse(&full).unwrap();
    assert!(draft.import_rules(0, &asset, &[true]).is_err());
    assert_eq!(draft.rules(0).len(), MAX_COMPLEX_RULES);
    assert!(!draft.dirty());
    let over = json!({"title":"over","rules":vec![rule("full");MAX_COMPLEX_RULES+1]}).to_string();
    assert!(RuleAsset::parse(&over).is_err());
}
#[test]
fn rule_replacement_requires_exact_saved_value_and_reordering_preserves_objects() {
    let source = source();
    let mut draft = VisualDraft::parse(&source).unwrap();
    let original = draft.rules(0)[0].clone();
    assert!(draft.replace_rule(0, 0, &original, "{}").is_err());
    assert!(!draft.dirty());
    let replacement = format!("// rule comment\n{}", rule("new"));
    draft.replace_rule(0, 0, &original, &replacement).unwrap();
    assert!(draft.replace_rule(0, 0, &original, &replacement).is_err());
    let asset =
        RuleAsset::parse(&json!({"title":"more","rules":[rule("two"),rule("three")]}).to_string())
            .unwrap();
    draft.import_rules(0, &asset, &[true, true]).unwrap();
    draft.move_rule(0, 0, 2).unwrap();
    assert_eq!(draft.rules(0), &[rule("two"), rule("three"), rule("new")]);
    draft.remove_rule(0, 1).unwrap();
    assert_eq!(draft.rules(0), &[rule("two"), rule("new")]);
    draft.set_rule_enabled(0, 0, false).unwrap();
    assert_eq!(draft.rules(0)[0]["enabled"], false);
}
#[test]
fn uncommitted_rule_editor_blocks_apply_and_replacement_until_commit_or_discard() {
    let mut app = SettingsApp::default();
    app.editor.replace(source());
    app.open_visual().unwrap();
    let pane = app.visual.as_mut().unwrap();
    pane.rules.open_rule(&pane.draft, 0, 0).unwrap();
    assert!(!pane.pending());
    *pane.rules.edit_text().unwrap() = rule("edited").to_string();
    assert!(app.unsaved());
    assert!(app.apply_visual().is_err());
    let pane = app.visual.as_mut().unwrap();
    assert!(pane.rules.open_rule(&pane.draft, 0, 0).is_err());
    *pane.rules.edit_text().unwrap() = "{invalid".into();
    assert!(pane.rules.commit(&mut pane.draft).is_err());
    assert!(pane.pending());
    assert_eq!(pane.draft.rules(0)[0], rule("old"));
    *pane.rules.edit_text().unwrap() = rule("edited").to_string();
    pane.rules.commit(&mut pane.draft).unwrap();
    assert!(!pane.pending());
    assert_eq!(pane.draft.rules(0)[0], rule("edited"));
    app.apply_visual().unwrap();
    assert!(app.editor.dirty());
    assert!(!app.editor.can_save());
}
#[test]
fn complex_panel_headless_renders_editor_and_asset_without_mutating_document() {
    let mut app = SettingsApp::default();
    app.editor.replace(source());
    app.open_visual().unwrap();
    let pane = app.visual.as_mut().unwrap();
    pane.rules.open_rule(&pane.draft, 0, 0).unwrap();
    pane.rules
        .load_asset("fixture.json".into(), &rule("import").to_string())
        .unwrap();
    let ctx = eframe::egui::Context::default();
    let mut output = ctx.run_ui(Default::default(), |ui| app.contents(ui));
    assert!(
        !ctx.tessellate(output.shapes, output.pixels_per_point)
            .is_empty()
    );
    output.textures_delta.clear();
    assert!(!app.unsaved());
    // Loading an invalid replacement does not silently swap the valid preview.
    let mut pane = RulesPane::default();
    assert!(pane.load_asset("invalid".into(), "{}").is_err());
}
#[test]
fn pinned_upstream_single_and_library_assets_import_as_exact_original_rules() {
    for text in [
        include_str!("../../../tests/fixtures/upstream/complex_assets/single_rule.json"),
        include_str!(
            "../../../tests/fixtures/upstream/complex_assets/single_rule_enabled_false.json"
        ),
        include_str!("../../../tests/fixtures/upstream/complex_assets/library.json"),
    ] {
        let raw: Value = serde_json::from_str(text).unwrap();
        let expected = raw
            .get("rules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| vec![raw.clone()]);
        let asset = RuleAsset::parse(text).unwrap();
        let mut draft = VisualDraft::parse(model::TEMPLATE).unwrap();
        assert_eq!(
            draft
                .import_rules(0, &asset, &vec![true; expected.len()])
                .unwrap(),
            expected.len()
        );
        assert_eq!(draft.rules(0), expected.as_slice());
        let report = model::validate(
            &draft.edited_json(model::TEMPLATE).unwrap(),
            model::Target::LinuxKeyboard,
            true,
        );
        // The original upstream rule objects reach complete offline backend
        // preparation without stripping metadata or inventing a `to` action.
        assert!(
            report.accepted,
            "{}: {:?}",
            report.summary, report.diagnostics
        );
        for target in model::Target::ALL {
            for staged in [false, true] {
                if target.mixed() && !staged {
                    continue;
                }
                let report =
                    model::validate(&draft.edited_json(model::TEMPLATE).unwrap(), target, staged);
                assert!(
                    report.accepted,
                    "{target:?}/{staged}: {:?}",
                    report.diagnostics
                );
            }
        }
    }
}
