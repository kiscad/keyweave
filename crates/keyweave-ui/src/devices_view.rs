use crate::{
    devices::{DeviceEdit, Row},
    visual::VisualDraft,
};
use eframe::egui;
use serde_json::Value;
#[derive(Default)]
pub struct DevicesPane {
    edit: Option<DeviceEdit>,
    delete: Option<(usize, usize, Value)>,
}
impl DevicesPane {
    pub fn pending(&self) -> bool {
        self.edit.as_ref().is_some_and(DeviceEdit::pending)
    }
    pub fn profile_changed(&mut self) {
        self.edit = None;
        self.delete = None;
    }
    pub fn open(
        &mut self,
        draft: &VisualDraft,
        profile: usize,
        index: Option<usize>,
    ) -> Result<(), String> {
        if self.pending() {
            return Err("Commit or discard the current device edits first".into());
        }
        self.edit = Some(DeviceEdit::open(draft, profile, index)?);
        self.delete = None;
        Ok(())
    }
    pub fn editor(&mut self) -> Option<&mut DeviceEdit> {
        self.edit.as_mut()
    }
    pub fn discard(&mut self) {
        self.edit = None;
    }
    pub fn commit(&mut self, draft: &mut VisualDraft) -> Result<(), String> {
        self.edit
            .as_ref()
            .ok_or("No device editor")?
            .commit(draft)?;
        self.edit = None;
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
        ui.heading("Configured devices");
        ui.label("Configured selectors are not a connected-device inventory. Validate target support before running.");
        let mut edit = None;
        let mut deletion = None;
        ui.add_enabled_ui(!self.pending(), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("configured-devices")
                .max_height(150.0)
                .show_rows(ui, 28.0, draft.device_count(profile), |ui, range| {
                    for index in range {
                        ui.push_id(index, |ui| {
                            ui.horizontal(|ui| {
                                let raw = draft.device(profile, index);
                                let identifiers = raw.and_then(|v| v.get("identifiers"));
                                let vendor = identifiers
                                    .and_then(|v| v.get("vendor_id"))
                                    .and_then(Value::as_u64);
                                let product = identifiers
                                    .and_then(|v| v.get("product_id"))
                                    .and_then(Value::as_u64);
                                ui.label(format!(
                                    "Device {} · vendor {:?} · product {:?}",
                                    index + 1,
                                    vendor,
                                    product
                                ));
                                if ui.button("Edit device").clicked() {
                                    edit = Some(Some(index));
                                }
                                if ui.button("Delete device").clicked()
                                    && let Some(raw) = raw
                                {
                                    deletion = Some((profile, index, raw.clone()));
                                }
                            })
                        });
                    }
                });
            if ui.button("Add configured device").clicked() {
                edit = Some(None);
            }
        });
        if let Some(index) = edit
            && let Err(error) = self.open(draft, profile, index)
        {
            *notice = error;
        }
        if let Some(deletion) = deletion {
            self.edit = None;
            self.delete = Some(deletion);
        }
        if let Some((_, index, _)) = &self.delete {
            ui.label(format!(
                "Delete configured device {} and all its settings/mappings?",
                index + 1
            ));
            ui.horizontal(|ui| {
                if ui.button("Keep device").clicked() {
                    self.delete = None;
                }
                if ui.button("Confirm device deletion").clicked()
                    && let Some((profile, index, expected)) = self.delete.take()
                {
                    *notice = match draft.delete_device(profile, index, &expected) {
                        Ok(()) => "Device removed from visual draft".into(),
                        Err(error) => error,
                    };
                }
            });
        }
        let mut commit = false;
        let mut discard = false;
        if let Some(edit) = &mut self.edit {
            let ui_key = edit.ui_key();
            ui.label(
                "Match vendor/product and both type flags exactly. Omitted IDs/types mean 0/false.",
            );
            ui.horizontal(|ui| {
                ui.label("Vendor ID");
                ui.add(
                    egui::TextEdit::singleline(&mut edit.selector.vendor)
                        .id(egui::Id::new(("device-vendor", ui_key)))
                        .char_limit(12)
                        .desired_width(115.0),
                );
                ui.label("Product ID");
                ui.add(
                    egui::TextEdit::singleline(&mut edit.selector.product)
                        .id(egui::Id::new(("device-product", ui_key)))
                        .char_limit(12)
                        .desired_width(115.0),
                );
                ui.checkbox(&mut edit.selector.keyboard, "Keyboard");
                ui.checkbox(&mut edit.selector.pointing, "Pointing device");
            });
            ui.label("IDs accept decimal or 0x hexadecimal. Advanced and hardware fields remain in JSON.");
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_salt("device-simple-rows")
                .max_height(200.0)
                .show_rows(ui, 30.0, edit.rows.len(), |ui, range| {
                    for index in range {
                        ui.push_id(index, |ui| {
                            ui.horizontal(|ui| match &mut edit.rows[index] {
                                Row::Editable(row) => {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut row.from)
                                            .id(egui::Id::new((
                                                "device-simple-from",
                                                ui_key,
                                                index,
                                            )))
                                            .char_limit(128)
                                            .desired_width(135.0),
                                    );
                                    ui.label("→");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut row.to)
                                            .id(egui::Id::new(("device-simple-to", ui_key, index)))
                                            .char_limit(128)
                                            .desired_width(135.0),
                                    );
                                    if ui.button("Remove mapping").clicked() {
                                        remove = Some(index);
                                    }
                                }
                                Row::Advanced(_) => {
                                    ui.label("Advanced mapping preserved · edit JSON");
                                }
                            })
                        });
                    }
                });
            if let Some(index) = remove
                && let Err(error) = edit.remove_row(index)
            {
                *notice = error;
            }
            if ui.button("Add device mapping").clicked()
                && let Err(error) = edit.add_row()
            {
                *notice = error;
            }
            ui.horizontal(|ui| {
                commit = ui.button("Commit device draft").clicked();
                discard = ui.button("Discard device edits").clicked();
            });
        }
        if discard {
            self.discard();
        }
        if commit {
            *notice = match self.commit(draft) {
                Ok(()) => {
                    "Device committed to visual draft; use edited JSON for the document".into()
                }
                Err(error) => error,
            };
        }
    }
}
