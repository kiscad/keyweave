#![allow(clippy::unwrap_used)]
use keyweave_ui::{
    files,
    model::{self, Editor, Target},
    view::SettingsApp,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
fn complete(editor: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(5);
    while editor.pending() {
        assert!(Instant::now() < until);
        editor.poll();
        std::thread::yield_now();
    }
}
const POINT: &str = r#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"pointing_button":"button1"},"to":[{"pointing_button":"button2"}]}]}]}"#;
#[test]
fn offline_gui_validation_uses_backend_scope_and_full_native_coverage() {
    for target in [Target::LinuxMixed, Target::WindowsMixed] {
        assert!(model::validate(POINT, target, true).accepted);
        assert!(!model::validate(POINT, target, false).accepted);
    }
    for target in [Target::LinuxKeyboard, Target::WindowsKeyboard] {
        let report = model::validate(POINT, target, true);
        assert!(!report.accepted);
        assert!(!report.diagnostics.is_empty());
    }
    let invalid = POINT.replace("button2", "button6");
    assert!(!model::validate(&invalid, Target::WindowsMixed, true).accepted);
    assert!(model::validate(&invalid, Target::LinuxMixed, true).accepted);
}
#[test]
fn edits_and_target_changes_revoke_validation_and_stale_jobs_cannot_enable_save() {
    let mut editor = Editor::default();
    editor.start_validation().unwrap();
    assert!(editor.start_validation().is_err());
    editor.replace("{invalid".into());
    complete(&mut editor);
    assert!(editor.report().is_none());
    assert!(!editor.can_save());
    editor.replace(model::TEMPLATE.into());
    editor.start_validation().unwrap();
    complete(&mut editor);
    assert!(editor.can_save());
    editor.text.push(' ');
    assert!(
        !editor.can_save(),
        "exact text fence also covers direct edits"
    );
    editor.changed();
    assert!(editor.report().is_none());
    assert!(editor.dirty());
    editor.start_validation().unwrap();
    editor.target = Target::WindowsMixed;
    complete(&mut editor);
    assert!(editor.report().is_none());
    assert!(!editor.can_save());
    editor.start_validation().unwrap();
    complete(&mut editor);
    assert!(editor.can_save());
    editor.mark_saved();
    assert!(!editor.dirty());
}
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "keyweave-ui-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn saving_copy_preserves_comments_utf8_and_existing_files_without_temp_leaks() {
    let dir = Directory::new();
    let original = dir.0.join("original.jsonc");
    let text = format!("// 原始注释\r\n{}", model::TEMPLATE);
    fs::write(&original, text.as_bytes()).unwrap();
    let opened = files::read_document(&original).unwrap();
    assert_eq!(opened, text);
    let destination = dir.0.join("new copy.jsonc");
    let report = files::save_copy(&destination, &opened).unwrap();
    assert_eq!(report.path, destination.canonicalize().unwrap());
    assert!(report.temporary_cleanup.is_none());
    assert_eq!(fs::read(&destination).unwrap(), text.as_bytes());
    assert!(files::save_copy(&original, "replacement").is_err());
    assert_eq!(fs::read(&original).unwrap(), text.as_bytes());
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
}
#[test]
fn invalid_utf8_oversize_and_directory_load_reject_without_publishing() {
    let dir = Directory::new();
    let bad = dir.0.join("bad.json");
    fs::write(&bad, [0xff]).unwrap();
    assert!(files::read_document(&bad).is_err());
    assert!(files::read_document(&dir.0).is_err());
    let huge = " ".repeat(keyweave_config::MAX_CONFIG_BYTES + 1);
    assert!(!model::validate(&huge, Target::LinuxKeyboard, true).accepted);
    let target = dir.0.join("too-large.json");
    assert!(files::save_copy(&target, &huge).is_err());
    assert!(!target.exists());
    fs::write(&bad, huge).unwrap();
    assert!(files::read_document(&bad).is_err());
}
#[cfg(unix)]
#[test]
fn copy_never_replaces_symlink_and_publishes_private_file_mode() {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};
    let dir = Directory::new();
    let existing = dir.0.join("target");
    fs::write(&existing, b"original").unwrap();
    let link = dir.0.join("link");
    symlink(&existing, &link).unwrap();
    assert!(files::save_copy(&link, "new").is_err());
    assert_eq!(fs::read(existing).unwrap(), b"original");
    let path = dir.0.join("copy");
    files::save_copy(&path, model::TEMPLATE).unwrap();
    assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
}
#[test]
fn headless_native_settings_layout_paints_diagnostics_without_runtime() {
    let mut app = SettingsApp::default();
    app.editor.replace(POINT.into());
    app.editor.target = Target::WindowsKeyboard;
    app.editor.start_validation().unwrap();
    complete(&mut app.editor);
    assert!(!app.editor.can_save());
    let context = eframe::egui::Context::default();
    let mut output = context.run_ui(Default::default(), |ui| app.contents(ui));
    assert!(!output.shapes.is_empty());
    assert!(
        !context
            .tessellate(output.shapes, output.pixels_per_point)
            .is_empty()
    );
    output.textures_delta.clear();
    assert!(
        app.editor
            .report()
            .is_some_and(|report| !report.diagnostics.is_empty())
    );
}
#[test]
fn offline_device_mapping_validation_keeps_windows_gate_and_exposes_metadata_readiness() {
    let text = r#"{"profiles":[{"devices":[{"identifiers":{"vendor_id":10,"product_id":20,"is_keyboard":true,"is_pointing_device":false},"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}]}"#;
    for staged in [false, true] {
        for target in [
            Target::LinuxKeyboard,
            Target::LinuxMixed,
            Target::WindowsKeyboard,
            Target::WindowsMixed,
        ] {
            if target.mixed() && !staged {
                continue;
            }
            let report = model::validate(text, target, staged);
            if matches!(target, Target::LinuxKeyboard | Target::LinuxMixed) {
                assert!(report.accepted, "{:?}", report.diagnostics);
                assert!(report.diagnostics.iter().any(|d| d.code == "KW-DEVICE-001"
                    && d.severity == keyweave_config::Severity::Warning));
            } else {
                assert!(!report.accepted);
                assert!(
                    report
                        .diagnostics
                        .iter()
                        .any(|d| d.pointer.contains("/devices/0"))
                );
            }
        }
    }
}
#[test]
fn utf8_application_and_input_source_patterns_reach_all_offline_backend_preparations() {
    let text = r#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"b","conditions":[{"type":"input_source_if","input_sources":[{"language":"^日本語$"}]}]}],"conditions":[{"type":"frontmost_application_if","file_paths":["编辑器\\.exe$"]}]}]}]}}]}"#;
    for target in Target::ALL {
        for staged in [false, true] {
            if target.mixed() && !staged {
                continue;
            }
            let report = model::validate(text, target, staged);
            assert!(
                report.accepted,
                "{target:?}/{staged}: {:?}",
                report.diagnostics
            );
        }
    }
}
