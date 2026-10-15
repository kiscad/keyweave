//! Offline editing model. Never starts a runtime, command or input backend.
use keyweave_config::{Diagnostic, Severity};
use keyweave_types::{BackendCapabilities, ConfigGeneration};
use keyweave_worker::{linux_preparation as linux, preparation as windows};
use std::thread::JoinHandle;
pub const TEMPLATE: &str = "{\n  \"profiles\": [{\n    \"name\": \"Default\",\n    \"selected\": true,\n    \"simple_modifications\": []\n  }]\n}\n";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    LinuxKeyboard,
    LinuxMixed,
    WindowsKeyboard,
    WindowsMixed,
}
impl Target {
    pub const ALL: [Self; 4] = [
        Self::LinuxKeyboard,
        Self::LinuxMixed,
        Self::WindowsKeyboard,
        Self::WindowsMixed,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::LinuxKeyboard => "Linux keyboard",
            Self::LinuxMixed => "Linux keyboard + mouse",
            Self::WindowsKeyboard => "Windows keyboard",
            Self::WindowsMixed => "Windows keyboard + mouse",
        }
    }
    pub fn mixed(self) -> bool {
        matches!(self, Self::LinuxMixed | Self::WindowsMixed)
    }
    fn capabilities(self) -> BackendCapabilities {
        let mut caps = if matches!(self, Self::LinuxKeyboard | Self::LinuxMixed) {
            BackendCapabilities::linux()
        } else {
            BackendCapabilities::windows_hook()
        };
        if self.mixed() {
            caps.pointing_buttons = true;
        }
        caps
    }
}
#[derive(Debug)]
pub struct Validation {
    pub accepted: bool,
    pub summary: String,
    pub diagnostics: Vec<Diagnostic>,
}
fn rejected(message: impl Into<String>) -> Validation {
    Validation {
        accepted: false,
        summary: message.into(),
        diagnostics: vec![],
    }
}
pub fn validate(text: &str, target: Target, staged: bool) -> Validation {
    if text.len() > keyweave_config::MAX_CONFIG_BYTES {
        return rejected("Configuration exceeds 8 MiB");
    }
    if target.mixed() && !staged {
        return rejected("Keyboard + mouse requires timed rules mode");
    }
    let mode = if staged {
        keyweave_config::CompileMode::StagedKeyboard
    } else {
        keyweave_config::CompileMode::Legacy
    };
    let compiled = match keyweave_config::compile_with_mode(
        text,
        &target.capabilities(),
        ConfigGeneration(1),
        mode,
    ) {
        Ok(c) => c,
        Err(e) => return rejected(e.to_string()),
    };
    if !compiled.executable_without_loss() {
        return Validation {
            accepted: false,
            summary: format!("{} unsupported rule(s)", compiled.disabled_rules),
            diagnostics: compiled.diagnostics,
        };
    }
    let cold = match target {
        Target::WindowsKeyboard | Target::WindowsMixed => {
            windows::PreparedConfig::from_bytes_for_generation_with_scope(
                text.as_bytes().to_vec(),
                ConfigGeneration(1),
                if staged {
                    windows::ExecutionMode::StagedKeyboard
                } else {
                    windows::ExecutionMode::Legacy
                },
                if target.mixed() {
                    windows::CaptureScope::StagedMixed
                } else {
                    windows::CaptureScope::Keyboard
                },
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        }
        Target::LinuxKeyboard | Target::LinuxMixed => (|| {
            let mut spec = linux::default_output_spec().map_err(|e| e.to_string())?;
            if target.mixed() {
                spec = spec
                    .with_relative_axes(
                        keyweave_linux::relative::RelativeAxes::new(
                            &keyweave_linux::relative::SUPPORTED_AXES,
                        )
                        .map_err(|e| e.to_string())?,
                    )
                    .with_pointing_buttons(
                        keyweave_linux::keys::PointingButtons::new(
                            &keyweave_linux::keys::BUTTON_CODES,
                        )
                        .map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
            }
            linux::PreparedConfig::from_bytes_for_generation_with_scope(
                text.as_bytes().to_vec(),
                spec,
                ConfigGeneration(1),
                if staged {
                    linux::ExecutionMode::StagedKeyboard
                } else {
                    linux::ExecutionMode::Legacy
                },
                if target.mixed() {
                    linux::CaptureScope::StagedRelative
                } else {
                    linux::CaptureScope::Keyboard
                },
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        })(),
    };
    let mut diagnostics = compiled.diagnostics;
    match cold {
        Ok(()) => Validation {
            accepted: true,
            summary: "Validated offline. Device readiness unverified.".into(),
            diagnostics,
        },
        Err(message) => {
            diagnostics.push(Diagnostic {
                code: "KW-UI-BACKEND".into(),
                severity: Severity::Error,
                pointer: "/profiles".into(),
                message,
            });
            Validation {
                accepted: false,
                summary: "Selected backend cannot run this configuration".into(),
                diagnostics,
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Revision {
    number: u64,
    target: Target,
    staged: bool,
}
pub struct Editor {
    pub text: String,
    pub target: Target,
    pub staged: bool,
    revision: u64,
    exhausted: bool,
    baseline: String,
    validated: Option<(Revision, String, Validation)>,
    job: Option<JoinHandle<(Revision, String, Validation)>>,
}
impl Default for Editor {
    fn default() -> Self {
        Self {
            text: TEMPLATE.into(),
            target: if cfg!(windows) {
                Target::WindowsKeyboard
            } else {
                Target::LinuxKeyboard
            },
            staged: true,
            revision: 1,
            exhausted: false,
            baseline: TEMPLATE.into(),
            validated: None,
            job: None,
        }
    }
}
impl Editor {
    fn revision(&self) -> Revision {
        Revision {
            number: self.revision,
            target: self.target,
            staged: self.staged,
        }
    }
    pub fn changed(&mut self) {
        self.validated = None;
        if let Some(next) = self.revision.checked_add(1) {
            self.revision = next
        } else {
            self.exhausted = true;
        }
    }
    pub fn replace(&mut self, text: String) {
        self.baseline = text.clone();
        self.text = text;
        self.changed();
    }
    /// A user edit preserves the loaded/saved baseline and revokes validation.
    pub fn edit(&mut self, text: String) {
        self.text = text;
        self.changed();
    }
    pub fn dirty(&self) -> bool {
        self.text != self.baseline
    }
    pub fn mark_saved(&mut self) {
        self.baseline = self.text.clone();
    }
    pub fn report(&self) -> Option<&Validation> {
        self.validated
            .as_ref()
            .filter(|(revision, text, _)| {
                *revision == self.revision() && *text == self.text && !self.exhausted
            })
            .map(|(_, _, report)| report)
    }
    pub fn pending(&self) -> bool {
        self.job.is_some()
    }
    pub fn can_save(&self) -> bool {
        self.report().is_some_and(|report| report.accepted)
            && self.text.len() <= keyweave_config::MAX_CONFIG_BYTES
    }
    pub fn start_validation(&mut self) -> Result<(), String> {
        if self.job.is_some() {
            return Err("A validation is already running".into());
        }
        if self.exhausted {
            return Err("Document revision exhausted; reopen the editor".into());
        }
        if self.text.len() > keyweave_config::MAX_CONFIG_BYTES {
            return Err("Configuration exceeds 8 MiB".into());
        }
        let revision = self.revision();
        let text = self.text.clone();
        self.validated = None;
        self.job = Some(
            std::thread::Builder::new()
                .name("keyweave-ui-validation".into())
                .spawn(move || {
                    let report = validate(&text, revision.target, revision.staged);
                    (revision, text, report)
                })
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    }
    pub fn poll(&mut self) {
        if !self.job.as_ref().is_some_and(JoinHandle::is_finished) {
            return;
        }
        if let Some(job) = self.job.take() {
            match job.join() {
                Ok((revision, text, report))
                    if revision == self.revision() && text == self.text && !self.exhausted =>
                {
                    self.validated = Some((revision, text, report))
                }
                Ok(_) => {}
                Err(_) => {
                    self.validated = Some((
                        self.revision(),
                        self.text.clone(),
                        rejected("Validation worker failed; retry validation"),
                    ))
                }
            }
        }
    }
}
