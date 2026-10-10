//! Cold host preparation. No backend, input API, timers or process creation.
use crate::{ArgumentError, LoadError, compile_config_bytes, read_config_bytes};
use keyweave_engine::ConfigSnapshot;
use keyweave_windows::{
    keys::{IdentityPolicy, InjectionTag},
    runtime::{RuntimeError, preflight_config},
};
use std::{ffi::OsString, io::Read, path::Path, sync::Arc};
#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
pub struct PreparedConfig {
    bytes: Box<[u8]>,
    snapshot: Arc<ConfigSnapshot>,
}
impl std::fmt::Debug for PreparedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedConfig")
            .field("byte_length", &self.bytes.len())
            .field("generation", &self.snapshot.generation)
            .finish_non_exhaustive()
    }
}
impl PreparedConfig {
    pub fn read(reader: impl Read) -> Result<Self, PreparationError> {
        let bytes = read_config_bytes(reader)?;
        Self::from_bytes(bytes)
    }
    /// Retains exact original JSONC/UTF8 bytes. Rejects before any native resource.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, PreparationError> {
        let snapshot = compile_config_bytes(&bytes)?;
        preflight_config(snapshot.clone())?;
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            snapshot,
        })
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
    if !path.is_absolute() || tag.value() > u32::MAX as usize {
        return Err(ArgumentError);
    }
    Ok(vec![
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
    ])
}
#[cfg(test)]
mod tests;
