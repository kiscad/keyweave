//! Cold host preparation. No backend, input API, timers or process creation.
use crate::{
    ArgumentError, LoadError, compile_config_bytes_for_generation_with_mode,
    next_config_generation, read_config_bytes,
};
use keyweave_engine::ConfigSnapshot;
use keyweave_types::{BackendCapabilities, ConfigGeneration};
pub use keyweave_windows::runtime::ExecutionMode;
use keyweave_windows::{
    keys::{IdentityPolicy, InjectionTag},
    runtime::{RuntimeError, preflight_config, preflight_staged_config},
};
use std::{ffi::OsString, io::Read, path::Path, sync::Arc};
#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    #[error("mixed Windows capture requires staged execution")]
    Scope,
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaptureScope {
    #[default]
    Keyboard,
    StagedMixed,
}
pub struct PreparedConfig {
    bytes: Box<[u8]>,
    snapshot: Arc<ConfigSnapshot>,
    mode: ExecutionMode,
    scope: CaptureScope,
}
impl std::fmt::Debug for PreparedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedConfig")
            .field("byte_length", &self.bytes.len())
            .field("generation", &self.snapshot.generation)
            .field("execution_mode", &self.mode)
            .field("capture_scope", &self.scope)
            .finish_non_exhaustive()
    }
}
impl PreparedConfig {
    pub fn read(reader: impl Read) -> Result<Self, PreparationError> {
        Self::read_for_generation(reader, ConfigGeneration(1))
    }
    /// Retains exact original JSONC/UTF8 bytes. Rejects before any native resource.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, PreparationError> {
        Self::from_bytes_for_generation(bytes, ConfigGeneration(1))
    }
    /// Cold only; actor activation still decides whether this generation is fresh.
    pub fn read_for_generation(
        reader: impl Read,
        generation: ConfigGeneration,
    ) -> Result<Self, PreparationError> {
        if generation.0 == 0 {
            return Err(LoadError::Generation.into());
        }
        Self::from_bytes_for_generation(read_config_bytes(reader)?, generation)
    }
    /// Must use the actor's observed active generation, not the latest prepared
    /// candidate. Failed/queued preparation never acknowledges activation.
    pub fn read_next(
        reader: impl Read,
        current: ConfigGeneration,
    ) -> Result<Self, PreparationError> {
        Self::read_for_generation(reader, next_config_generation(current)?)
    }
    pub fn from_bytes_for_generation(
        bytes: Vec<u8>,
        generation: ConfigGeneration,
    ) -> Result<Self, PreparationError> {
        Self::from_bytes_for_generation_with_mode(bytes, generation, ExecutionMode::Legacy)
    }
    pub fn read_with_mode(
        reader: impl Read,
        mode: ExecutionMode,
    ) -> Result<Self, PreparationError> {
        Self::from_bytes_for_generation_with_mode(
            read_config_bytes(reader)?,
            ConfigGeneration(1),
            mode,
        )
    }
    pub fn from_bytes_for_generation_with_mode(
        bytes: Vec<u8>,
        generation: ConfigGeneration,
        mode: ExecutionMode,
    ) -> Result<Self, PreparationError> {
        Self::from_bytes_for_generation_with_scope(bytes, generation, mode, CaptureScope::Keyboard)
    }
    pub fn read_for_generation_with_scope(
        reader: impl Read,
        generation: ConfigGeneration,
        mode: ExecutionMode,
        scope: CaptureScope,
    ) -> Result<Self, PreparationError> {
        if generation.0 == 0 {
            return Err(LoadError::Generation.into());
        }
        if scope == CaptureScope::StagedMixed && mode != ExecutionMode::StagedKeyboard {
            return Err(PreparationError::Scope);
        }
        Self::from_bytes_for_generation_with_scope(
            read_config_bytes(reader)?,
            generation,
            mode,
            scope,
        )
    }
    pub fn from_bytes_for_generation_with_scope(
        bytes: Vec<u8>,
        generation: ConfigGeneration,
        mode: ExecutionMode,
        scope: CaptureScope,
    ) -> Result<Self, PreparationError> {
        if generation.0 == 0 {
            return Err(LoadError::Generation.into());
        }
        if scope == CaptureScope::StagedMixed && mode != ExecutionMode::StagedKeyboard {
            return Err(PreparationError::Scope);
        }
        let mut capabilities = BackendCapabilities::windows_hook();
        if scope == CaptureScope::StagedMixed {
            capabilities.name = "windows-hook-mixed".into();
            capabilities.pointing_buttons = true;
        }
        let snapshot = compile_config_bytes_for_generation_with_mode(
            &bytes,
            &capabilities,
            generation,
            match mode {
                ExecutionMode::Legacy => keyweave_config::CompileMode::Legacy,
                ExecutionMode::StagedKeyboard => keyweave_config::CompileMode::StagedKeyboard,
            },
        )?;
        if scope == CaptureScope::StagedMixed {
            keyweave_windows::mixed_actor::preflight_mixed_config(snapshot.clone())?;
        } else {
            match mode {
                ExecutionMode::Legacy => preflight_config(snapshot.clone())?,
                ExecutionMode::StagedKeyboard => preflight_staged_config(snapshot.clone())?,
            }
        }

        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            snapshot,
            mode,
            scope,
        })
    }
    pub fn capture_scope(&self) -> CaptureScope {
        self.scope
    }
    pub fn execution_mode(&self) -> ExecutionMode {
        self.mode
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        &self.snapshot
    }
    /// Caller retains returned file owner until actual child Exited. No spawn.
    #[cfg(windows)]
    pub fn seal(
        &self,
        directory: &Path,
    ) -> Result<keyweave_windows::ConfigArtifact, keyweave_windows::ArtifactCreateError> {
        keyweave_windows::ConfigArtifact::create(directory, &self.bytes)
    }
}
/// Fixed extra args for Supervisor; parent/run and config handle are added by
/// its owned transaction. Preserve OS path units; no quoting/shell expansion.
pub fn worker_arguments(
    path: &Path,
    tag: InjectionTag,
    identity: IdentityPolicy,
) -> Result<Vec<OsString>, ArgumentError> {
    worker_arguments_with_mode(path, tag, identity, ExecutionMode::Legacy)
}
pub fn worker_arguments_with_mode(
    path: &Path,
    tag: InjectionTag,
    identity: IdentityPolicy,
    mode: ExecutionMode,
) -> Result<Vec<OsString>, ArgumentError> {
    worker_arguments_with_scope(path, tag, identity, mode, CaptureScope::Keyboard)
}
pub fn worker_arguments_with_scope(
    path: &Path,
    tag: InjectionTag,
    identity: IdentityPolicy,
    mode: ExecutionMode,
    scope: CaptureScope,
) -> Result<Vec<OsString>, ArgumentError> {
    if scope == CaptureScope::StagedMixed && mode != ExecutionMode::StagedKeyboard {
        return Err(ArgumentError);
    }
    if !path.is_absolute() || tag.value() > u32::MAX as usize {
        return Err(ArgumentError);
    }
    let mut args = vec![
        "--config".into(),
        path.as_os_str().to_owned(),
        "--tag".into(),
        tag.value().to_string().into(),
        "--identity".into(),
        match identity {
            IdentityPolicy::StrictUnique => "strict",
            IdentityPolicy::CanonicalScan => "canonical-scan",
        }
        .into(),
    ];
    if mode == ExecutionMode::StagedKeyboard {
        args.extend(["--execution-mode".into(), "staged-keyboard".into()]);
    }
    if scope == CaptureScope::StagedMixed {
        args.extend(["--capture-scope".into(), "staged-mixed".into()]);
    }
    Ok(args)
}
#[cfg(test)]
mod tests;
