use crate::visual::VisualDraft;
use eframe::egui;
use std::collections::BTreeMap;

pub struct VisualPane {
    pub draft: VisualDraft,
    pub rules: crate::rules_view::RulesPane,
    profile: usize,
    name: String,
    from: String,
    to: String,
    rows: BTreeMap<usize, (String, String)>,
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
        let name = draft.profile_name(profile).into();
        Ok(Self {
            draft,
            rules: Default::default(),
            profile,
            name,
            from: "a".into(),
            to: "b".into(),
            rows: BTreeMap::new(),
            discard: false,
        })
    }
    fn browse(&mut self, profile: usize) {
        self.profile = profile;
        self.name = self.draft.profile_name(profile).into();
        self.rows.clear();
        self.rules.profile_changed();
    }
    pub fn pending(&self) -> bool {
        self.rules.pending()
    }
    pub fn contents(&mut self, ui: &mut egui::Ui, notice: &mut String) -> Action {
        ui.separator();
        ui.heading("Profiles and simple mappings · Draft");
        ui.label(
            "Visual edits rewrite JSON formatting and comments; other JSON fields are preserved.",
        );
        ui.label("Draft edits do not change the document until Use edited JSON. This does not activate a runtime profile.");
        if self.pending() {
            ui.label("Commit or discard rule JSON edits to change profiles or apply the draft.");
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
            if chosen != self.profile {
                self.browse(chosen);
            }
            ui.horizontal(|ui| {
                if ui.button("Select as active").clicked() {
                    show(self.draft.select(self.profile), notice);
                }
                if ui.button("Add profile").clicked() {
                    match self.draft.add_profile() {
                        Ok(i) => self.browse(i),
                        Err(e) => *notice = e,
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Profile name");
                ui.text_edit_singleline(&mut self.name);
                if ui.button("Rename").clicked() {
                    show(self.draft.rename(self.profile, &self.name), notice);
                }
            });
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_salt("visual-rows")
                .max_height(200.0)
                .show_rows(ui, 30.0, self.draft.row_count(self.profile), |ui, range| {
                    for i in range {
                        ui.push_id(i, |ui| {
                            ui.horizontal(|ui| {
                                if let Some(row) = self.draft.row(self.profile, i) {
                                    let pair = self.rows.entry(i).or_insert((row.from, row.to));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut pair.0)
                                            .desired_width(140.0)
                                            .char_limit(128),
                                    );
                                    ui.label("→");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut pair.1)
                                            .desired_width(140.0)
                                            .char_limit(128),
                                    );
                                    if ui.button("Update").clicked() {
                                        show(
                                            self.draft.set_row(self.profile, i, &pair.0, &pair.1),
                                            notice,
                                        );
                                    }
                                    if ui.button("Remove").clicked() {
                                        remove = Some(i);
                                    }
                                } else {
                                    ui.label("Advanced mapping — edit JSON");
                                }
                            });
                        });
                    }
                });
            if let Some(i) = remove {
                show(self.draft.remove_row(self.profile, i), notice);
                self.rows.clear();
            }
            ui.horizontal(|ui| {
                ui.label("New mapping");
                ui.add(
                    egui::TextEdit::singleline(&mut self.from)
                        .desired_width(140.0)
                        .char_limit(128),
                );
                ui.label("→");
                ui.add(
                    egui::TextEdit::singleline(&mut self.to)
                        .desired_width(140.0)
                        .char_limit(128),
                );
                if ui.button("Add mapping").clicked() {
                    show(
                        self.draft.add_row(self.profile, &self.from, &self.to),
                        notice,
                    );
                }
            });
        });
        self.rules
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
            });
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
        Err(e) => e,
    };
}
