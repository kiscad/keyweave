use crate::{
    files,
    model::{Editor, Target},
    visual_view::{Action, VisualPane},
};
use eframe::egui;
#[derive(Default)]
pub struct SettingsApp {
    pub editor: Editor,
    pub open_path: String,
    pub copy_path: String,
    pub notice: String,
    pub visual: Option<VisualPane>,
    pending_document: Option<String>,
    confirm_close: bool,
    allow_close: bool,
    pub smoke_passes: Option<u8>,
}
impl SettingsApp {
    pub fn unsaved(&self) -> bool {
        self.editor.dirty() || self.visual.as_ref().is_some_and(|p| p.draft.dirty())
    }
    pub fn open_visual(&mut self) -> Result<(), String> {
        if self.visual.is_some() {
            return Err("Close the current visual draft first".into());
        }
        self.visual = Some(VisualPane::new(&self.editor.text)?);
        Ok(())
    }
    pub fn apply_visual(&mut self) -> Result<(), String> {
        let text = self
            .visual
            .as_ref()
            .ok_or("No visual draft")?
            .draft
            .edited_json(&self.editor.text)?;
        self.editor.edit(text);
        self.visual = None;
        Ok(())
    }

    pub fn contents(&mut self, ui: &mut egui::Ui) {
        self.editor.poll();
        if let Some(passes) = self.smoke_passes.as_mut() {
            *passes = passes.saturating_sub(1);
            if *passes == 0 {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.ctx().request_repaint();
            }
        }
        if ui.ctx().input(|i| i.viewport().close_requested()) && self.unsaved() && !self.allow_close
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if self.confirm_close {
            ui.horizontal(|ui| {
                ui.label("Unsaved edits. Close the editor?");
                if ui.button("Keep editing").clicked() {
                    self.confirm_close = false;
                }
                if ui.button("Discard and close").clicked() {
                    self.allow_close = true;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        }
        if self.pending_document.is_some() {
            let mut replace = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                ui.label("Replace unsaved edits with the opened file?");
                replace = ui.button("Discard edits and open").clicked();
                cancel = ui.button("Cancel open").clicked();
            });
            if replace && let Some(text) = self.pending_document.take() {
                self.editor.replace(text);
                self.visual = None;
                self.notice = "Configuration opened".into();
            }
            if cancel {
                self.pending_document = None;
            }
        }
        ui.heading("KeyWeave Settings");
        ui.label("Configuration editor · Offline");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Open configuration");
            ui.text_edit_singleline(&mut self.open_path);
            if ui.button("Open").clicked() {
                match files::read_document(std::path::Path::new(&self.open_path)) {
                    Ok(text) => {
                        if self.unsaved() {
                            self.pending_document = Some(text);
                        } else {
                            self.editor.replace(text);
                            self.visual = None;
                            self.notice = "Configuration opened".into();
                        }
                    }
                    Err(e) => self.notice = e,
                }
            }
        });
        let previous = (self.editor.target, self.editor.staged);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("target")
                .selected_text(self.editor.target.label())
                .show_ui(ui, |ui| {
                    for target in Target::ALL {
                        ui.selectable_value(&mut self.editor.target, target, target.label());
                    }
                });
            ui.checkbox(&mut self.editor.staged, "Timed rules and chords");
            if previous != (self.editor.target, self.editor.staged) {
                self.editor.changed();
            }
            if ui
                .add_enabled(!self.editor.pending(), egui::Button::new("Validate"))
                .clicked()
            {
                if let Err(e) = self.editor.start_validation() {
                    self.notice = e;
                } else {
                    self.notice.clear();
                }
            }
        });
        if self.editor.target == Target::LinuxMixed {
            ui.label("Offline target: full supported mouse output capabilities.");
        }
        if self.visual.is_none()
            && ui.button("Edit profiles and simple mappings").clicked()
            && let Err(e) = self.open_visual()
        {
            self.notice = e;
        }
        if let Some(pane) = self.visual.as_mut() {
            match pane.contents(ui, &mut self.notice) {
                Action::Apply => match self.apply_visual() {
                    Ok(()) => {
                        self.notice =
                            "Draft applied; validate the changed document before saving".into()
                    }
                    Err(e) => self.notice = e,
                },
                Action::Close => self.visual = None,
                Action::None => {}
            }
        }
        egui::ScrollArea::vertical()
            .id_salt("document")
            .max_height(360.0)
            .show(ui, |ui| {
                let response = ui.add(
                    egui::TextEdit::multiline(&mut self.editor.text)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(18)
                        .char_limit(keyweave_config::MAX_CONFIG_BYTES),
                );
                if response.changed() {
                    self.editor.changed();
                }
            });
        if self.editor.pending() {
            ui.spinner();
            ui.label("Checking configuration…");
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(20));
        }
        if let Some(report) = self.editor.report() {
            ui.colored_label(
                if report.accepted {
                    egui::Color32::from_rgb(50, 180, 110)
                } else {
                    egui::Color32::from_rgb(235, 110, 90)
                },
                &report.summary,
            );
            egui::ScrollArea::vertical()
                .id_salt("diagnostics")
                .max_height(140.0)
                .show(ui, |ui| {
                    for diagnostic in &report.diagnostics {
                        ui.label(format!(
                            "{:?}  {}  {}  {}",
                            diagnostic.severity,
                            diagnostic.code,
                            diagnostic.pointer,
                            diagnostic.message
                        ));
                    }
                });
        } else if !self.editor.pending() {
            ui.label("Validate the current document to save a verified copy.");
        }
        ui.horizontal(|ui| {
            ui.label("New copy path");
            ui.text_edit_singleline(&mut self.copy_path);
            if ui
                .add_enabled(
                    self.editor.can_save() && !self.copy_path.is_empty(),
                    egui::Button::new("Save validated copy"),
                )
                .clicked()
            {
                self.notice = match files::save_copy(
                    std::path::Path::new(&self.copy_path),
                    &self.editor.text,
                ) {
                    Ok(report) => {
                        self.editor.mark_saved();
                        match report.temporary_cleanup {
                            None => format!("Copy saved: {}", report.path.display()),
                            Some(e) => format!(
                                "Copy saved: {}; temporary cleanup failed: {e}",
                                report.path.display()
                            ),
                        }
                    }
                    Err(e) => e,
                };
            }
        });
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
    }
}
impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.contents(ui));
    }
}
