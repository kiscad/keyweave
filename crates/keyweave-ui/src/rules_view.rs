use crate::{files, rules::RuleAsset, visual::VisualDraft};
use eframe::egui;
use serde_json::Value;
struct RuleEdit {
    profile: usize,
    index: usize,
    expected: Value,
    baseline: String,
    text: String,
}
#[derive(Default)]
pub struct RulesPane {
    edit: Option<RuleEdit>,
    pub import_path: String,
    asset: Option<(String, RuleAsset, Vec<bool>)>,
    delete: Option<usize>,
}
impl RulesPane {
    pub fn pending(&self) -> bool {
        self.edit.as_ref().is_some_and(|e| e.text != e.baseline)
    }
    pub fn discard_edit(&mut self) {
        self.edit = None;
    }
    pub fn edit_text(&mut self) -> Option<&mut String> {
        self.edit.as_mut().map(|e| &mut e.text)
    }
    pub fn open_rule(
        &mut self,
        draft: &VisualDraft,
        profile: usize,
        index: usize,
    ) -> Result<(), String> {
        if self.pending() {
            return Err("Commit or discard the current rule JSON changes first".into());
        }
        let expected = draft
            .rules(profile)
            .get(index)
            .ok_or("Rule no longer exists")?
            .clone();
        let text = serde_json::to_string_pretty(&expected).map_err(|e| e.to_string())?;
        self.edit = Some(RuleEdit {
            profile,
            index,
            expected,
            baseline: text.clone(),
            text,
        });
        Ok(())
    }
    pub fn commit(&mut self, draft: &mut VisualDraft) -> Result<(), String> {
        let edit = self.edit.as_ref().ok_or("No rule editor")?;
        draft.replace_rule(edit.profile, edit.index, &edit.expected, &edit.text)?;
        self.edit = None;
        Ok(())
    }
    pub fn load_asset(&mut self, source: String, text: &str) -> Result<(), String> {
        let asset = RuleAsset::parse(text)?;
        let selected = vec![false; asset.rules().len()];
        self.asset = Some((source, asset, selected));
        Ok(())
    }
    pub fn contents(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut VisualDraft,
        profile: usize,
        notice: &mut String,
    ) {
        ui.separator();
        ui.heading("Complex rules");
        ui.label("Order is significant. Unsupported fields are retained; Validate checks backend compatibility after Use edited JSON.");
        let pending = self.pending();
        ui.add_enabled_ui(!pending, |ui| {
            enum Operation {
                Edit(usize),
                Enable(usize, bool),
                Move(usize, usize),
                Delete(usize),
            }
            let mut operation = None;
            let rules = draft.rules(profile);
            egui::ScrollArea::vertical()
                .id_salt("complex-list")
                .max_height(180.0)
                .show_rows(ui, 30.0, rules.len(), |ui, range| {
                    for i in range {
                        ui.push_id(("complex", i), |ui| {
                            ui.horizontal(|ui| {
                                let mut enabled =
                                    rules[i].get("enabled") != Some(&Value::Bool(false));
                                if ui.checkbox(&mut enabled, "").changed() {
                                    operation = Some(Operation::Enable(i, enabled));
                                }
                                ui.label(format!("{}: {}", i + 1, description(&rules[i])));
                                if ui.button("Edit JSON").clicked() {
                                    operation = Some(Operation::Edit(i));
                                }
                                if ui.add_enabled(i > 0, egui::Button::new("↑")).clicked() {
                                    operation = Some(Operation::Move(i, i - 1));
                                }
                                if ui
                                    .add_enabled(i + 1 < rules.len(), egui::Button::new("↓"))
                                    .clicked()
                                {
                                    operation = Some(Operation::Move(i, i + 1));
                                }
                                if ui.button("Delete rule").clicked() {
                                    operation = Some(Operation::Delete(i));
                                }
                            });
                        });
                    }
                });
            if let Some(op) = operation {
                let result = match op {
                    Operation::Edit(i) => self.open_rule(draft, profile, i),
                    Operation::Enable(i, e) => {
                        self.discard_edit();
                        draft.set_rule_enabled(profile, i, e)
                    }
                    Operation::Move(a, b) => {
                        self.discard_edit();
                        self.delete = None;
                        draft.move_rule(profile, a, b)
                    }
                    Operation::Delete(i) => {
                        self.delete = Some(i);
                        Ok(())
                    }
                };
                if let Err(e) = result {
                    *notice = e;
                }
            }
            if let Some(index) = self.delete {
                ui.horizontal(|ui| {
                    ui.label(format!("Delete rule {}?", index + 1));
                    if ui.button("Confirm delete rule").clicked() {
                        if let Err(e) = draft.remove_rule(profile, index) {
                            *notice = e;
                        }
                        self.discard_edit();
                        self.delete = None;
                    }
                    if ui.button("Keep rule").clicked() {
                        self.delete = None;
                    }
                });
            }
        });
        if let Some(edit) = self.edit.as_mut() {
            ui.label(format!(
                "Rule {} JSON — commit or discard before changing profile or applying the draft",
                edit.index + 1
            ));
            egui::ScrollArea::vertical()
                .id_salt("rule-json")
                .max_height(200.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut edit.text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(8)
                            .char_limit(keyweave_config::MAX_CONFIG_BYTES),
                    );
                });
            ui.horizontal(|ui| {
                if ui.button("Commit rule JSON").clicked() {
                    match self.commit(draft) {
                        Ok(()) => {
                            *notice = "Rule added to draft; apply edited JSON and validate".into()
                        }
                        Err(e) => *notice = e,
                    }
                }
                if ui.button("Discard rule JSON edits").clicked() {
                    self.discard_edit();
                }
            });
        }
        ui.collapsing("Import Karabiner rule asset",|ui| {
            ui.label("Imports selected complete rules only, appended in source order. Asset wrapper metadata (title, maintainers and other fields) is not copied. Single-rule enabled:false stays disabled. No JavaScript or user actions run.");
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.import_path);
                if ui.button("Load rule asset").clicked() {
                    match files::read_document(std::path::Path::new(&self.import_path))
                        .and_then(|text|self.load_asset(self.import_path.clone(),&text)) {
                        Ok(())=>*notice="Asset loaded; select rules explicitly, append, then apply and Validate".into(),
                        Err(e)=>*notice=e,
                    }
                }
            });
            if let Some((source,asset,selected))=self.asset.as_mut() {
                ui.label(format!("Source: {source}"));
                ui.label(format!("{} — {} rules",asset.title().chars().take(100).collect::<String>(),asset.rules().len()));
                egui::ScrollArea::vertical().id_salt("asset-rules").max_height(180.0)
                    .show_rows(ui,26.0,asset.rules().len(),|ui,range| {
                        for i in range { ui.checkbox(&mut selected[i],format!("{}: {}",i+1,description(&asset.rules()[i]))); }
                    });
                if ui.add_enabled(!self.edit.as_ref().is_some_and(|e|e.text!=e.baseline),egui::Button::new("Append selected rules to draft")).clicked() {
                    match draft.import_rules(profile,asset,selected) {
                        Ok(n)=>{selected.fill(false);self.edit=None;*notice=format!("Appended {n} complete rules; apply edited JSON and Validate");},
                        Err(e)=>*notice=e,
                    }
                }
            }
        });
    }
    pub fn profile_changed(&mut self) {
        self.discard_edit();
        self.delete = None;
    }
}
fn description(value: &Value) -> String {
    value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("Unnamed rule")
        .chars()
        .take(64)
        .collect()
}
