use crate::visual::{SimpleRow, VisualDraft};
use eframe::egui;
use serde_json::Value;
use std::collections::BTreeMap;
struct RowInput {
    expected: Value,
    baseline: SimpleRow,
    value: SimpleRow,
}
pub struct VisualPane {
    pub draft: VisualDraft,
    pub rules: crate::rules_view::RulesPane,
    pub devices: crate::devices_view::DevicesPane,
    profile: usize,
    name: String,
    name_baseline: String,
    name_expected: Option<Value>,
    from: String,
    to: String,
    rows: BTreeMap<usize, RowInput>,
    discard: bool,
}
pub enum Action {
    None,
    Apply,
    Close,
}
impl VisualPane {
    pub fn new(source: &str) -> Result<Self, String> {
        let draft = VisualDraft::parse(source)?;
        let profile = draft.active();
        let name = draft.profile_name(profile).to_owned();
        let name_expected = draft.raw["profiles"][profile].get("name").cloned();
        Ok(Self {
            draft,
            rules: Default::default(),
            devices: Default::default(),
            profile,
            name_baseline: name.clone(),
            name,
            name_expected,
            from: "a".into(),
            to: "b".into(),
            rows: BTreeMap::new(),
            discard: false,
        })
    }
    pub fn browse(&mut self, profile: usize) -> Result<(), String> {
        if self.pending() {
            return Err("Commit or discard pending visual inputs before changing profiles".into());
        }
        if profile >= self.draft.len() {
            return Err("Profile no longer exists".into());
        }
        self.profile = profile;
        self.discard_simple_inputs();
        self.rules.profile_changed();
        self.devices.profile_changed();
        Ok(())
    }
    pub fn name_input(&mut self) -> &mut String {
        self.sync_name();
        &mut self.name
    }
    fn sync_name(&mut self) {
        if self.name == self.name_baseline
            && self.draft.raw["profiles"][self.profile].get("name") != self.name_expected.as_ref()
        {
            self.reload_name();
        }
    }
    pub fn commit_name(&mut self) -> Result<(), String> {
        if self.draft.raw["profiles"][self.profile].get("name") != self.name_expected.as_ref() {
            return Err("Profile name changed since editing began; discard and retry".into());
        }
        if self.name != self.name_baseline {
            self.draft.rename(self.profile, &self.name)?;
        }
        self.reload_name();
        Ok(())
    }
    fn reload_name(&mut self) {
        self.name = self.draft.profile_name(self.profile).to_owned();
        self.name_baseline = self.name.clone();
        self.name_expected = self.draft.raw["profiles"][self.profile]
            .get("name")
            .cloned();
    }
    pub fn row_input(&mut self, index: usize) -> Result<&mut SimpleRow, String> {
        let baseline = self
            .draft
            .row(self.profile, index)
            .ok_or("Advanced or missing mapping must be edited as JSON")?;
        let expected = self
            .draft
            .raw_row(self.profile, index)
            .ok_or("Mapping no longer exists")?
            .clone();
        let input = self.rows.entry(index).or_insert_with(|| RowInput {
            expected: expected.clone(),
            value: baseline.clone(),
            baseline: baseline.clone(),
        });
        // An untouched cache may follow an explicit draft edit; pending text
        // always keeps its original fence instead of silently rebasing it.
        if input.value == input.baseline && input.expected != expected {
            *input = RowInput {
                expected,
                value: baseline.clone(),
                baseline,
            };
        }
        Ok(&mut input.value)
    }
    pub fn commit_row(&mut self, index: usize) -> Result<(), String> {
        let input = self.rows.get(&index).ok_or("No row input buffer")?;
        if self.draft.raw_row(self.profile, index) != Some(&input.expected) {
            return Err("Mapping changed since editing began; discard and retry".into());
        }
        if input.value != input.baseline {
            self.draft
                .set_row(self.profile, index, &input.value.from, &input.value.to)?;
        }
        self.rows.remove(&index);
        Ok(())
    }
    pub fn discard_row(&mut self, index: usize) {
        self.rows.remove(&index);
    }
    pub fn new_mapping_inputs(&mut self) -> (&mut String, &mut String) {
        (&mut self.from, &mut self.to)
    }
    pub fn add_mapping(&mut self) -> Result<(), String> {
        self.draft.add_row(self.profile, &self.from, &self.to)?;
        self.from = "a".into();
        self.to = "b".into();
        Ok(())
    }
    pub fn remove_mapping(&mut self, index: usize) -> Result<(), String> {
        if self.pending() {
            return Err("Commit or discard pending inputs before removing a mapping".into());
        }
        self.draft.remove_row(self.profile, index)?;
        self.rows.clear();
        Ok(())
    }
    pub fn simple_pending(&self) -> bool {
        self.name != self.name_baseline
            || self.from != "a"
            || self.to != "b"
            || self
                .rows
                .values()
                .any(|input| input.value != input.baseline)
    }
    pub fn discard_simple_inputs(&mut self) {
        self.rows.clear();
        self.reload_name();
        self.from = "a".into();
        self.to = "b".into();
    }
    pub fn pending(&self) -> bool {
        self.simple_pending() || self.rules.pending() || self.devices.pending()
    }
    pub fn contents(&mut self, ui: &mut egui::Ui, notice: &mut String) -> Action {
        self.sync_name();
        ui.separator();
        ui.heading("Profiles and simple mappings · Draft");
        ui.label(
            "Visual edits rewrite JSON formatting and comments; other JSON fields are preserved.",
        );
        ui.label("Draft edits do not change the document until Use edited JSON. This does not activate a runtime profile.");
        if self.pending() {
            ui.label(
                "Commit or discard changed inputs before changing profiles or applying the draft.",
            );
        }
        ui.add_enabled_ui(!self.pending(), |ui| {
            let mut chosen = self.profile;
            egui::ComboBox::from_id_salt("visual-profile")
                .selected_text(
                    self.draft
                        .profile_name(chosen)
                        .chars()
                        .take(64)
                        .collect::<String>(),
                )
                .show_ui(ui, |ui| {
                    for i in 0..self.draft.len() {
                        let label = format!(
                            "{}{}",
                            if i == self.draft.active() {
                                "[active] "
                            } else {
                                ""
                            },
                            self.draft
                                .profile_name(i)
                                .chars()
                                .take(64)
                                .collect::<String>()
                        );
                        ui.selectable_value(&mut chosen, i, label);
                    }
                });
            if chosen != self.profile
                && let Err(error) = self.browse(chosen)
            {
                *notice = error;
            }
            ui.horizontal(|ui| {
                if ui.button("Select as active").clicked() {
                    show(self.draft.select(self.profile), notice);
                }
                if ui.button("Add profile").clicked() {
                    match self.draft.add_profile() {
                        Ok(index) => show(self.browse(index), notice),
                        Err(error) => *notice = error,
                    }
                }
            });
        });
        let profile = self.profile;
        // Completion controls remain enabled while these buffers are pending.
        ui.horizontal(|ui| {
            ui.label("Profile name");
            ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .id(egui::Id::new(("visual-profile-name", profile)))
                    .char_limit(256),
            );
            if ui.button("Rename").clicked() {
                show(self.commit_name(), notice);
            }
        });
        enum Operation {
            Update(usize),
            Discard(usize),
            Remove(usize),
        }
        let mut operation = None;
        let count = self.draft.row_count(self.profile);
        egui::ScrollArea::vertical()
            .id_salt("visual-rows")
            .max_height(200.0)
            .show_rows(ui, 30.0, count, |ui, range| {
                for index in range {
                    ui.push_id(index, |ui| {
                        ui.horizontal(|ui| {
                            if let Ok(row) = self.row_input(index) {
                                ui.add(
                                    egui::TextEdit::singleline(&mut row.from)
                                        .id(egui::Id::new(("visual-simple-from", profile, index)))
                                        .desired_width(140.0)
                                        .char_limit(128),
                                );
                                ui.label("→");
                                ui.add(
                                    egui::TextEdit::singleline(&mut row.to)
                                        .id(egui::Id::new(("visual-simple-to", profile, index)))
                                        .desired_width(140.0)
                                        .char_limit(128),
                                );
                                if ui.button("Update").clicked() {
                                    operation = Some(Operation::Update(index));
                                }
                                if ui.button("Discard row input").clicked() {
                                    operation = Some(Operation::Discard(index));
                                }
                                if ui.button("Remove").clicked() {
                                    operation = Some(Operation::Remove(index));
                                }
                            } else {
                                ui.label("Advanced mapping — edit JSON");
                            }
                        })
                    });
                }
            });
        if let Some(operation) = operation {
            match operation {
                Operation::Update(index) => show(self.commit_row(index), notice),
                Operation::Discard(index) => self.discard_row(index),
                Operation::Remove(index) => show(self.remove_mapping(index), notice),
            }
        }
        ui.horizontal(|ui| {
            ui.label("New mapping");
            ui.add(
                egui::TextEdit::singleline(&mut self.from)
                    .id(egui::Id::new(("visual-new-from", profile)))
                    .desired_width(140.0)
                    .char_limit(128),
            );
            ui.label("→");
            ui.add(
                egui::TextEdit::singleline(&mut self.to)
                    .id(egui::Id::new(("visual-new-to", profile)))
                    .desired_width(140.0)
                    .char_limit(128),
            );
            if ui.button("Add mapping").clicked() {
                show(self.add_mapping(), notice);
            }
        });
        if self.simple_pending() && ui.button("Discard simple inputs").clicked() {
            self.discard_simple_inputs();
        }
        self.rules
            .contents(ui, &mut self.draft, self.profile, notice);
        self.devices
            .contents(ui, &mut self.draft, self.profile, notice);
        let mut action = Action::None;
        ui.add_enabled_ui(!self.pending(), |ui| {
            ui.horizontal(|ui| {
                if ui.button("Use edited JSON").clicked() {
                    action = Action::Apply;
                }
                if ui.button("Close visual editor").clicked() {
                    if self.draft.dirty() {
                        self.discard = true;
                    } else {
                        action = Action::Close;
                    }
                }
            })
        });
        if self.discard {
            ui.horizontal(|ui| {
                ui.label("Discard visual draft changes?");
                if ui.button("Keep draft").clicked() {
                    self.discard = false;
                }
                if ui.button("Discard draft").clicked() {
                    action = Action::Close;
                }
            });
        }
        action
    }
}
fn show(result: Result<(), String>, notice: &mut String) {
    *notice = match result {
        Ok(()) => "Draft updated; use edited JSON to apply to the document".into(),
        Err(error) => error,
    };
}
