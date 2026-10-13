//! Cold Linux configuration validation. No device or process resources.
use crate::{
    LoadError, compile_config_bytes_for_generation, next_config_generation, read_config_bytes,
};
use keyweave_engine::{ConfigSnapshot, EngineError};
use keyweave_linux::{
    keys,
    output::{DeviceSpec, RepeatPolicy},
    runtime::{ExecutionMode, RuntimeError, validate_config},
};
use keyweave_types::{BackendCapabilities, ConfigGeneration};
use std::{io::Read, sync::Arc};

#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    #[error(transparent)]
    Load(#[from] LoadError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Engine(#[from] EngineError),
}
/// Maximum supported native output table, not consumer readiness or input coverage.
pub fn default_output_spec() -> std::io::Result<DeviceSpec> {
    DeviceSpec::new(
        "KeyWeave",
        &keys::native_codes().collect::<Vec<_>>(),
        RepeatPolicy::Kernel,
        false,
    )
}
pub struct PreparedConfig {
    bytes: Box<[u8]>,
    snapshot: Arc<ConfigSnapshot>,
    output: DeviceSpec,
    mode: ExecutionMode,
}
impl std::fmt::Debug for PreparedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxPreparedConfig")
            .field("byte_length", &self.bytes.len())
            .field("generation", &self.snapshot.generation)
            .field("execution_mode", &self.mode)
            .finish_non_exhaustive()
    }
}
impl PreparedConfig {
    pub fn read(reader: impl Read, output: DeviceSpec) -> Result<Self, PreparationError> {
        Self::read_for_generation(reader, output, ConfigGeneration(1))
    }
    pub fn from_bytes(bytes: Vec<u8>, output: DeviceSpec) -> Result<Self, PreparationError> {
        Self::from_bytes_for_generation(bytes, output, ConfigGeneration(1))
    }
    /// Cold preparation only. Runtime validates freshness and its actual output
    /// spec at activation; preparation does not acknowledge a successful switch.
    pub fn read_for_generation(
        reader: impl Read,
        output: DeviceSpec,
        generation: ConfigGeneration,
    ) -> Result<Self, PreparationError> {
        if generation.0 == 0 {
            return Err(LoadError::Generation.into());
        }
        Self::from_bytes_for_generation(read_config_bytes(reader)?, output, generation)
    }
    pub fn read_next(
        reader: impl Read,
        output: DeviceSpec,
        current: ConfigGeneration,
    ) -> Result<Self, PreparationError> {
        Self::read_for_generation(reader, output, next_config_generation(current)?)
    }
    pub fn from_bytes_for_generation(
        bytes: Vec<u8>,
        output: DeviceSpec,
        generation: ConfigGeneration,
    ) -> Result<Self, PreparationError> {
        Self::from_bytes_for_generation_with_mode(bytes, output, generation, ExecutionMode::Legacy)
    }
    pub fn read_with_mode(
        reader: impl Read,
        output: DeviceSpec,
        mode: ExecutionMode,
    ) -> Result<Self, PreparationError> {
        Self::from_bytes_for_generation_with_mode(
            read_config_bytes(reader)?,
            output,
            ConfigGeneration(1),
            mode,
        )
    }
    pub fn from_bytes_for_generation_with_mode(
        bytes: Vec<u8>,
        output: DeviceSpec,
        generation: ConfigGeneration,
        mode: ExecutionMode,
    ) -> Result<Self, PreparationError> {
        let snapshot =
            compile_config_bytes_for_generation(&bytes, &BackendCapabilities::linux(), generation)?;
        if output.repeat() != RepeatPolicy::Kernel {
            return Err(RuntimeError::RepeatPolicy.into());
        }
        validate_config(&snapshot, &output)?;
        match mode.validate_snapshot(snapshot.clone()) {
            Err(RuntimeError::Engine(error)) => return Err(PreparationError::Engine(error)),
            Err(error) => return Err(PreparationError::Runtime(error)),
            Ok(()) => {}
        }
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            snapshot,
            output,
            mode,
        })
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
    pub fn output_spec(&self) -> &DeviceSpec {
        &self.output
    }
    /// Seals exactly the cold-validated bytes. Does not spawn or grab devices.
    #[cfg(target_os = "linux")]
    pub fn seal(&self) -> std::io::Result<keyweave_linux::config_artifact::ConfigArtifact> {
        keyweave_linux::config_artifact::ConfigArtifact::create(&self.bytes)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use keyweave_types::ConfigGeneration;
    const JSONC: &[u8] = br#"// private bytes
{"profiles":[{"name":"private name","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b","modifiers":["left_control"]}]}]}]}"#;
    fn prepare(bytes: &[u8]) -> Result<PreparedConfig, PreparationError> {
        PreparedConfig::read(bytes, default_output_spec().unwrap())
    }
    const TIMED: &[u8] = br#"// exact staged bytes
{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"description":"timed","manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"b","hold_down_milliseconds":50},{"key_code":"c"}]}]}]}}]}"#;
    #[test]
    fn explicit_staged_preparation_binds_mode_bytes_generation_and_default_gate() {
        let prepared = PreparedConfig::read_with_mode(
            TIMED,
            default_output_spec().unwrap(),
            ExecutionMode::StagedKeyboard,
        )
        .unwrap();
        assert_eq!(prepared.bytes(), TIMED);
        assert_eq!(prepared.execution_mode(), ExecutionMode::StagedKeyboard);
        assert_eq!(
            prepared.snapshot().complex[0].to[0].hold_down_ns,
            50_000_000
        );
        assert!(matches!(
            prepare(TIMED),
            Err(PreparationError::Engine(
                EngineError::UnsupportedActionTiming
            ))
        ));
        let next = PreparedConfig::from_bytes_for_generation_with_mode(
            TIMED.to_vec(),
            default_output_spec().unwrap(),
            ConfigGeneration(2),
            prepared.execution_mode(),
        )
        .unwrap();
        assert_eq!(next.snapshot().generation, ConfigGeneration(2));
        assert_eq!(next.bytes(), prepared.bytes());
    }
    #[test]
    fn staged_preparation_keeps_repeat_policy_and_output_capability_rejection() {
        let external = DeviceSpec::new(
            "external",
            &keys::native_codes().collect::<Vec<_>>(),
            RepeatPolicy::External,
            false,
        )
        .unwrap();
        assert!(matches!(
            PreparedConfig::read_with_mode(TIMED, external, ExecutionMode::StagedKeyboard),
            Err(PreparationError::Runtime(RuntimeError::RepeatPolicy))
        ));
        let narrow = DeviceSpec::new("narrow", &[48], RepeatPolicy::Kernel, false).unwrap();
        assert!(matches!(
            PreparedConfig::read_with_mode(TIMED, narrow, ExecutionMode::StagedKeyboard),
            Err(PreparationError::Runtime(RuntimeError::ConfigurationKey(_)))
        ));
    }
    #[test]
    fn exact_bytes_snapshot_and_spec_remain_bound() {
        let prepared = prepare(JSONC).unwrap();
        assert_eq!(prepared.bytes(), JSONC);
        assert_eq!(prepared.snapshot().generation, ConfigGeneration(1));
        assert_eq!(prepared.snapshot().simple.len(), 1);
        assert_eq!(prepared.output_spec().repeat(), RepeatPolicy::Kernel);
        assert!(!prepared.output_spec().leds());
        assert!(!format!("{prepared:?}").contains("private"));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn cold_prepared_image_seals_exact_bytes_without_reopening_a_path() {
        assert_eq!(
            keyweave_config::MAX_CONFIG_BYTES,
            keyweave_linux::config_artifact::MAX_ARTIFACT_BYTES
        );
        let prepared = prepare(JSONC).unwrap();
        let artifact = prepared.seal().unwrap();
        assert_eq!(artifact.read_bytes().unwrap(), prepared.bytes());
        assert_eq!(artifact.byte_length(), JSONC.len());
        assert!(!format!("{artifact:?}").contains("private"));
    }
    #[test]
    fn byte_constructor_enforces_inclusive_size_limit() {
        let mut bytes = JSONC.to_vec();
        bytes.resize(keyweave_config::MAX_CONFIG_BYTES, b' ');
        let prepared = PreparedConfig::from_bytes(bytes, default_output_spec().unwrap()).unwrap();
        assert_eq!(prepared.bytes().len(), keyweave_config::MAX_CONFIG_BYTES);
        let mut bytes = prepared.bytes().to_vec();
        bytes.push(b' ');
        assert!(matches!(
            PreparedConfig::from_bytes(bytes, default_output_spec().unwrap()),
            Err(PreparationError::Load(LoadError::TooLarge))
        ));
    }
    #[test]
    fn output_key_and_modifier_must_both_be_advertised() {
        for codes in [&[30, 29][..], &[30, 48][..]] {
            let spec = DeviceSpec::new("test", codes, RepeatPolicy::Kernel, false).unwrap();
            assert!(matches!(
                PreparedConfig::read(JSONC, spec),
                Err(PreparationError::Runtime(RuntimeError::ConfigurationKey(_)))
            ));
        }
        let spec = DeviceSpec::new("test", &[30, 48, 29], RepeatPolicy::Kernel, false).unwrap();
        assert!(PreparedConfig::read(JSONC, spec).is_ok());
    }
    #[test]
    fn external_repeat_cannot_be_prepared_for_current_runtime() {
        let spec = DeviceSpec::new("test", &[30, 48, 29], RepeatPolicy::External, false).unwrap();
        assert!(matches!(
            PreparedConfig::read(JSONC, spec),
            Err(PreparationError::Runtime(RuntimeError::RepeatPolicy))
        ));
    }
    #[test]
    fn disabled_actions_cannot_silently_activate() {
        let bytes=br#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"shell_command":"never execute"}]}]}]}}]}"#;
        assert!(matches!(
            prepare(bytes),
            Err(PreparationError::Load(LoadError::Loss { .. }))
        ));
    }
    #[test]
    fn oversized_reads_are_bounded_and_utf8_and_io_failures_preserved() {
        struct Endless(usize);
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.0 += buf.len();
                buf.fill(b' ');
                Ok(buf.len())
            }
        }
        let mut reader = Endless(0);
        assert!(matches!(
            PreparedConfig::read(&mut reader, default_output_spec().unwrap()),
            Err(PreparationError::Load(LoadError::TooLarge))
        ));
        assert_eq!(reader.0, keyweave_config::MAX_CONFIG_BYTES + 1);
        assert!(matches!(
            prepare(&[0xff]),
            Err(PreparationError::Load(LoadError::Utf8(_)))
        ));
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::PermissionDenied.into())
            }
        }
        assert!(matches!(
            PreparedConfig::read(Broken, default_output_spec().unwrap()),
            Err(PreparationError::Load(LoadError::Io(_)))
        ));
    }
}
