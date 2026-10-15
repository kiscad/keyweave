//! Private worker entry contract. Parsing/compilation have no input side effects.
use keyweave_config::{ConfigError, Diagnostic, MAX_CONFIG_BYTES};
use keyweave_engine::ConfigSnapshot;
use keyweave_types::{BackendCapabilities, ConfigGeneration};
use keyweave_windows::keys::{IdentityPolicy, InjectionTag};
use std::{ffi::OsString, io::Read, path::PathBuf, sync::Arc};

pub mod linux_arguments;
pub mod linux_preparation;
pub mod preparation;
pub mod reload_control;
pub mod reload_preparation;

pub const WORKER_ARGUMENT: &str = "--keyweave-worker";
pub const ARGUMENT_COUNT: usize = 9;
pub const MAX_ARGUMENT_COUNT: usize = 15;

mod exit;
mod terminal;
pub use exit::{ExitDiagnostic, WorkerExit};
pub use terminal::{NativeWorkerExit, TerminalEvidence, TerminalIssue};

#[derive(Debug, thiserror::Error)]
#[error("invalid private worker arguments")]
pub struct ArgumentError;

#[derive(Debug)]
pub struct Arguments {
    pub parent: u32,
    pub run_id: u64,
    pub config: PathBuf,
    pub config_handle: Option<usize>,
    pub tag: InjectionTag,
    pub identity: IdentityPolicy,
    pub execution_mode: preparation::ExecutionMode,
    pub capture_scope: preparation::CaptureScope,
}

fn decimal(value: &std::ffi::OsStr) -> Result<u64, ArgumentError> {
    let value = value.to_str().ok_or(ArgumentError)?;
    // Canonical positive decimal only: no whitespace, signs or zero padding.
    if value.is_empty()
        || value.len() > 20
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ArgumentError);
    }
    value.parse().map_err(|_| ArgumentError)
}

impl Arguments {
    /// Consume at most sixteen OS arguments; preserve non-Unicode configuration paths.
    /// Fixed ordering prevents duplicate/unknown switches from reaching admission.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, ArgumentError> {
        let args: Vec<_> = args.into_iter().take(MAX_ARGUMENT_COUNT + 1).collect();
        if ![ARGUMENT_COUNT, 11, 13, MAX_ARGUMENT_COUNT].contains(&args.len())
            || args[0] != WORKER_ARGUMENT
            || args[3] != "--config"
            || args[5] != "--tag"
            || args[7] != "--identity"
        {
            return Err(ArgumentError);
        }
        let mut suffix = 9;
        let execution_mode = if args
            .get(suffix)
            .is_some_and(|arg| arg == "--execution-mode")
        {
            let mode = match args.get(suffix + 1).and_then(|arg| arg.to_str()) {
                Some("legacy") => preparation::ExecutionMode::Legacy,
                Some("staged-keyboard") => preparation::ExecutionMode::StagedKeyboard,
                _ => return Err(ArgumentError),
            };
            suffix += 2;
            mode
        } else {
            preparation::ExecutionMode::Legacy
        };
        let capture_scope = if args.get(suffix).is_some_and(|arg| arg == "--capture-scope") {
            let scope = match args.get(suffix + 1).and_then(|arg| arg.to_str()) {
                Some("keyboard") => preparation::CaptureScope::Keyboard,
                Some("staged-mixed") => preparation::CaptureScope::StagedMixed,
                _ => return Err(ArgumentError),
            };
            suffix += 2;
            scope
        } else {
            preparation::CaptureScope::Keyboard
        };
        let config_handle = if args.len() == suffix + 2 {
            if args[suffix] != "--keyweave-config-handle" {
                return Err(ArgumentError);
            }
            let value = usize::try_from(decimal(&args[suffix + 1])?).map_err(|_| ArgumentError)?;
            if value == usize::MAX {
                return Err(ArgumentError);
            }
            Some(value)
        } else if args.len() == suffix {
            None
        } else {
            return Err(ArgumentError);
        };
        if capture_scope == preparation::CaptureScope::StagedMixed
            && (execution_mode != preparation::ExecutionMode::StagedKeyboard
                || config_handle.is_none())
        {
            return Err(ArgumentError);
        }
        let parent = u32::try_from(decimal(&args[1])?).map_err(|_| ArgumentError)?;
        let run_id = decimal(&args[2])?;
        // Marker is explicitly restricted to nonzero u32 on both architectures.
        let marker = u32::try_from(decimal(&args[6])?).map_err(|_| ArgumentError)?;
        let tag = InjectionTag::new(marker as usize).map_err(|_| ArgumentError)?;
        let config = PathBuf::from(&args[4]);
        if !config.is_absolute() {
            return Err(ArgumentError);
        }
        let identity = match args[8].to_str() {
            Some("strict") => IdentityPolicy::StrictUnique,
            Some("canonical-scan") => IdentityPolicy::CanonicalScan,
            _ => return Err(ArgumentError),
        };
        Ok(Self {
            parent,
            run_id,
            config,
            config_handle,
            tag,
            identity,
            execution_mode,
            capture_scope,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("configuration generation is zero or exhausted")]
    Generation,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("configuration exceeds {MAX_CONFIG_BYTES} bytes")]
    TooLarge,
    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Context(#[from] keyweave_windows::startup::ProbeError),
    #[error(transparent)]
    Compile(#[from] ConfigError),
    #[error("configuration cannot be activated without loss")]
    Loss {
        disabled: usize,
        diagnostics: Vec<Diagnostic>,
    },
}

/// Must run after native parent admission and before constructing input resources.
/// Native output/engine compatibility is also checked by Worker.install before
/// its first input probe/notification/Hook call. No metadata/script evaluation.
pub fn load_config(reader: impl Read) -> Result<Arc<ConfigSnapshot>, LoadError> {
    compile_config_bytes(&read_config_bytes(reader)?)
}
fn read_config_bytes(reader: impl Read) -> Result<Vec<u8>, LoadError> {
    keyweave_windows::startup::ensure_preparation_context()?;
    let mut bytes = Vec::new();
    reader
        .take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(LoadError::TooLarge);
    }
    Ok(bytes)
}
fn compile_config_bytes(bytes: &[u8]) -> Result<Arc<ConfigSnapshot>, LoadError> {
    compile_config_bytes_for(bytes, &BackendCapabilities::windows_hook())
}
/// Caller supplies the observed active generation; not an activation ack.
/// Pure arithmetic, no global counter or wall-clock-derived identity.
pub fn next_config_generation(current: ConfigGeneration) -> Result<ConfigGeneration, LoadError> {
    if current.0 == 0 {
        return Err(LoadError::Generation);
    }
    current
        .0
        .checked_add(1)
        .map(ConfigGeneration)
        .ok_or(LoadError::Generation)
}
fn compile_config_bytes_for(
    bytes: &[u8],
    capabilities: &BackendCapabilities,
) -> Result<Arc<ConfigSnapshot>, LoadError> {
    compile_config_bytes_for_generation(bytes, capabilities, ConfigGeneration(1))
}
fn compile_config_bytes_for_generation(
    bytes: &[u8],
    capabilities: &BackendCapabilities,
    generation: ConfigGeneration,
) -> Result<Arc<ConfigSnapshot>, LoadError> {
    compile_config_bytes_for_generation_with_mode(
        bytes,
        capabilities,
        generation,
        keyweave_config::CompileMode::Legacy,
    )
}
fn compile_config_bytes_for_generation_with_mode(
    bytes: &[u8],
    capabilities: &BackendCapabilities,
    generation: ConfigGeneration,
    mode: keyweave_config::CompileMode,
) -> Result<Arc<ConfigSnapshot>, LoadError> {
    if generation.0 == 0 {
        return Err(LoadError::Generation);
    }
    keyweave_windows::startup::ensure_preparation_context()?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(LoadError::TooLarge);
    }
    let text = std::str::from_utf8(bytes)?;
    let compiled = keyweave_config::compile_with_mode(text, capabilities, generation, mode)?;
    if !compiled.executable_without_loss() {
        return Err(LoadError::Loss {
            disabled: compiled.disabled_rules,
            diagnostics: compiled.diagnostics,
        });
    }
    Ok(Arc::new(compiled.snapshot))
}

/// Facts include every independent shutdown result, not just the primary fault.
#[derive(Clone, Copy, Debug)]
pub struct TerminalFacts {
    pub fault: bool,
    pub progress_failed: bool,
    pub shutdown_present: bool,
    pub stopped: bool,
    pub unhook_ok: bool,
    pub recovery_ok: bool,
    pub notifications_ok: bool,
    pub access_lost: bool,
    pub session_changed: bool,
    pub untracked_input: bool,
}
impl TerminalFacts {
    /// Mouse cleanup can only add an unhook obligation, never erase keyboard failure.
    pub fn with_mouse_unhook(mut self, mouse_ok: bool) -> Self {
        self.unhook_ok &= mouse_ok;
        self
    }
    pub fn exit(self) -> WorkerExit {
        if !self.shutdown_present
            || !self.stopped
            || !self.unhook_ok
            || !self.recovery_ok
            || !self.notifications_ok
            || self.access_lost
            || self.session_changed
            || self.untracked_input
        {
            WorkerExit::Cleanup
        } else if self.fault || self.progress_failed {
            WorkerExit::Loop
        } else {
            WorkerExit::Clean
        }
    }
}

#[cfg(test)]
mod tests;
