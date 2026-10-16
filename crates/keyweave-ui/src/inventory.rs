//! Explicit asynchronous metadata discovery, separate from runtime inventory.
use crate::devices::Selector;
use eframe::egui;
use keyweave_linux::inventory::{Entry, Snapshot};
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const MAX_AGE: Duration = Duration::from_secs(30);
static SCAN_BUSY: OnceLock<Arc<AtomicBool>> = OnceLock::new();
struct Guard(Arc<AtomicBool>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
struct Observation {
    started: Instant,
    snapshot: Snapshot,
}
struct Job {
    started: Instant,
    handle: JoinHandle<Snapshot>,
}
pub struct InventoryPane {
    gate: Arc<AtomicBool>,
    job: Option<Job>,
    observation: Option<Observation>,
    error: Option<String>,
}
impl Default for InventoryPane {
    fn default() -> Self {
        Self::with_gate(
            SCAN_BUSY
                .get_or_init(|| Arc::new(AtomicBool::new(false)))
                .clone(),
        )
    }
}
impl InventoryPane {
    fn with_gate(gate: Arc<AtomicBool>) -> Self {
        Self {
            gate,
            job: None,
            observation: None,
            error: None,
        }
    }
    fn start(
        &mut self,
        now: Instant,
        scan: impl FnOnce() -> Snapshot + Send + 'static,
    ) -> Result<(), String> {
        if self.job.is_some() {
            return Err("A metadata refresh is still running".into());
        }
        self.observation = None;
        self.error = None;
        // Even another pane's detached job retains the guard until scan returns.
        self.gate
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "A metadata scan from another pane is still running")?;
        let guard = Guard(self.gate.clone());
        match thread::Builder::new()
            .name("keyweave-device-metadata".into())
            .spawn(move || {
                let _guard = guard;
                scan()
            }) {
            Ok(handle) => {
                self.job = Some(Job {
                    started: now,
                    handle,
                });
                Ok(())
            }
            Err(error) => {
                let message = format!("Could not start metadata scan: {error}");
                self.error = Some(message.clone());
                Err(message)
            }
        }
    }
    fn poll(&mut self) {
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.handle.is_finished())
            && let Some(job) = self.job.take()
        {
            match job.handle.join() {
                Ok(snapshot) => {
                    self.observation = Some(Observation {
                        started: job.started,
                        snapshot,
                    })
                }
                Err(_) => {
                    self.error = Some("Metadata scan failed; refresh explicitly to retry".into())
                }
            }
        }
    }
    pub fn selector(&self, index: usize, now: Instant) -> Result<Selector, String> {
        if self.job.is_some() {
            return Err("Wait for the current metadata refresh".into());
        }
        let observation = self
            .observation
            .as_ref()
            .ok_or("Refresh device metadata first")?;
        if now
            .checked_duration_since(observation.started)
            .is_none_or(|age| age > MAX_AGE)
        {
            return Err("Metadata observation is old; refresh before generating a selector".into());
        }
        let Some(Entry::Observed { metadata, .. }) = observation.snapshot.entries.get(index) else {
            return Err("No observed metadata for this node".into());
        };
        let types = metadata
            .types
            .as_ref()
            .ok_or("Device types are Unknown; no selector was generated")?;
        Ok(Selector {
            vendor: metadata.vendor_id.to_string(),
            product: metadata.product_id.to_string(),
            keyboard: types.is_keyboard,
            pointing: types.is_pointing_device,
        })
    }
    pub fn contents(
        &mut self,
        ui: &mut egui::Ui,
        can_select: bool,
        notice: &mut String,
    ) -> Option<Selector> {
        self.poll();
        let now = Instant::now();
        ui.separator();
        ui.heading("Linux device metadata");
        ui.label("Source: /dev/input · metadata only. Names and paths are observations, not stable device identity.");
        ui.label("Generate a selector draft, then review and commit it. Identical selectors match all devices with those IDs/types.");
        if ui
            .add_enabled(
                self.job.is_none(),
                egui::Button::new("Refresh Linux devices"),
            )
            .clicked()
            && let Err(error) = self.start(now, keyweave_linux::inventory::scan)
        {
            *notice = error;
        }
        if self.job.is_some() {
            ui.label("Reading metadata… No capture or event read. Closing this pane does not cancel a blocked scan.");
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        let mut selected = None;
        if let Some(observation) = &self.observation {
            let snapshot = &observation.snapshot;
            let age = now.checked_duration_since(observation.started);
            ui.label(format!(
                "Age since refresh began: {} · directory complete: {} · probes complete: {}",
                age.map_or("Unknown".into(), |age| format!("{:.1}s", age.as_secs_f64())),
                snapshot.scan_complete,
                snapshot.probes_complete
            ));
            if !snapshot.scan_complete || !snapshot.probes_complete {
                ui.label(
                    "Partial snapshot. Missing or unavailable nodes are not evidence of absence.",
                );
            }
            if age.is_none_or(|age| age > MAX_AGE) {
                ui.label("Observation expired for selector generation. Refresh to select.");
            } else {
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            for issue in snapshot.issues.iter().take(16) {
                ui.label(issue);
            }
            if snapshot.issues.len() > 16 {
                ui.label(format!(
                    "{} further scan diagnostics",
                    snapshot.issues.len() - 16
                ));
            }
            egui::ScrollArea::vertical()
                .id_salt("linux-device-metadata")
                .max_height(220.0)
                .show(ui, |ui| {
                    for (index, entry) in snapshot.entries.iter().enumerate() {
                        ui.push_id(index, |ui| match entry {
                            Entry::Unavailable { path, failure } => {
                                ui.label(format!(
                                    "{} · unavailable: {} / errno {:?}",
                                    path.display(),
                                    failure.kind,
                                    failure.os_code
                                ));
                            }
                            Entry::Observed { path, metadata } => {
                                let name = metadata
                                    .name
                                    .as_ref()
                                    .and_then(|name| serde_json::to_string(name).ok())
                                    .unwrap_or_else(|| "Unknown name".into());
                                ui.label(format!(
                                    "{} · {} · {:04x}:{:04x}{}",
                                    path.display(),
                                    name,
                                    metadata.vendor_id,
                                    metadata.product_id,
                                    if metadata.name_was_lossy {
                                        " · lossy name"
                                    } else {
                                        ""
                                    }
                                ));
                                ui.label(metadata.types.as_ref().map_or(
                                    "Types: Unknown".into(),
                                    |types| {
                                        format!(
                                            "Keyboard: {} · Pointing: {}",
                                            types.is_keyboard, types.is_pointing_device
                                        )
                                    },
                                ));
                                let selector = self.selector(index, now);
                                if ui
                                    .add_enabled(
                                        can_select && selector.is_ok(),
                                        egui::Button::new("Generate selector draft"),
                                    )
                                    .clicked()
                                {
                                    selected = selector.ok();
                                }
                            }
                        });
                    }
                    if snapshot.entries.is_empty() {
                        ui.label("No node metadata in this snapshot.");
                    }
                });
        }
        selected
    }
}
#[cfg(test)]
mod tests;
