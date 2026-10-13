#![allow(clippy::unwrap_used)]
use super::*;
use crate::Arguments;
use keyweave_types::ConfigGeneration;
use std::io::{self, Cursor};
const JSONC: &[u8] = br#"// preserved comment
{"profiles":[{"name":"private profile","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}
"#;
#[test]
fn exact_jsonc_image_and_selected_snapshot_use_same_worker_compiler() {
    let prepared = PreparedConfig::read(Cursor::new(JSONC)).unwrap();
    assert_eq!(prepared.bytes(), JSONC);
    assert_eq!(prepared.snapshot().generation, ConfigGeneration(1));
    assert_eq!(prepared.snapshot().name, "private profile");
    let worker = crate::load_config(Cursor::new(prepared.bytes())).unwrap();
    assert_eq!(worker.name, prepared.snapshot().name);
    assert_eq!(worker.simple.len(), 1);
    assert_eq!(prepared.snapshot().simple.len(), 1);
    assert_eq!(worker.simple[0].from, prepared.snapshot().simple[0].from);
    assert_eq!(worker.simple[0].to, prepared.snapshot().simple[0].to);
    assert!(worker.complex.is_empty() && prepared.snapshot().complex.is_empty());
    let debug = format!("{prepared:?}");
    assert!(!debug.contains("private profile"));
    assert!(!debug.contains("preserved comment"));
}
#[test]
fn native_special_output_is_rejected_before_artifact_or_backend() {
    let bytes = String::from_utf8(JSONC.to_vec())
        .unwrap()
        .replace("\"b\"", "\"print_screen\"");
    // Compiler accepts HID intent; native encoding gate rejects multi-packet
    // special sequence that isn't implemented as an ordinary scan injection.
    assert!(crate::load_config(Cursor::new(bytes.as_bytes())).is_ok());
    assert!(matches!(
        PreparedConfig::from_bytes(bytes.into_bytes()),
        Err(PreparationError::Runtime(RuntimeError::Key(_)))
    ));
}
#[test]
fn disabled_rule_cannot_be_prepared_as_a_partially_executable_image() {
    let bytes=br#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"shell_command":"never execute this"}]}]}]}}]}"#;
    assert!(matches!(
        PreparedConfig::read(Cursor::new(bytes)),
        Err(PreparationError::Load(LoadError::Loss { .. }))
    ));
}
#[test]
fn invalid_utf8_diagnostic_never_retains_or_prints_original_config_body() {
    let mut bytes = b"private-config-secret".to_vec();
    bytes.push(0xff);
    let error = PreparedConfig::from_bytes(bytes).unwrap_err();
    assert!(matches!(error, PreparationError::Load(LoadError::Utf8(_))));
    assert!(!format!("{error:?}").contains("private-config-secret"));
    assert!(!error.to_string().contains("private-config-secret"));
}
#[test]
fn size_and_io_failures_remain_bounded_and_distinct() {
    let mut bytes = JSONC.to_vec();
    bytes.resize(keyweave_config::MAX_CONFIG_BYTES, b' ');
    let prepared = PreparedConfig::from_bytes(bytes).unwrap();
    assert_eq!(prepared.bytes().len(), keyweave_config::MAX_CONFIG_BYTES);
    let mut consumed = 0usize;
    struct Reader<'a>(&'a mut usize);
    impl Read for Reader<'_> {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            *self.0 += b.len();
            b.fill(b' ');
            Ok(b.len())
        }
    }
    assert!(matches!(
        PreparedConfig::read(Reader(&mut consumed)),
        Err(PreparationError::Load(LoadError::TooLarge))
    ));
    assert_eq!(consumed, keyweave_config::MAX_CONFIG_BYTES + 1);
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }
    assert!(matches!(
        PreparedConfig::read(Broken),
        Err(PreparationError::Load(LoadError::Io(_)))
    ));
}
#[test]
fn strict_worker_extra_arguments_roundtrip_both_identity_policies() {
    let path = std::env::temp_dir().join("config with spaces.jsonc");
    for identity in [IdentityPolicy::StrictUnique, IdentityPolicy::CanonicalScan] {
        let extra = worker_arguments(
            &path,
            InjectionTag::new(u32::MAX as usize).unwrap(),
            identity,
        )
        .unwrap();
        let mut args: Vec<OsString> = vec![crate::WORKER_ARGUMENT.into(), "42".into(), "7".into()];
        args.extend(extra);
        args.extend(["--keyweave-config-handle".into(), "512".into()]);
        let parsed = Arguments::parse(args).unwrap();
        assert_eq!(parsed.config, path);
        assert_eq!(parsed.identity, identity);
        assert_eq!(parsed.config_handle, Some(512));
    }
    assert!(
        worker_arguments(
            Path::new("relative.json"),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::StrictUnique
        )
        .is_err()
    );
    if usize::BITS > 32 {
        assert!(
            worker_arguments(
                &path,
                InjectionTag::new(usize::MAX).unwrap(),
                IdentityPolicy::StrictUnique
            )
            .is_err()
        );
    }
}
#[cfg(unix)]
#[test]
fn worker_builder_preserves_non_unicode_path_units() {
    use std::os::unix::ffi::OsStringExt;
    let path = std::path::PathBuf::from(OsString::from_vec(b"/tmp/private-\xff.json".to_vec()));
    let extra = worker_arguments(
        &path,
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
    )
    .unwrap();
    assert_eq!(extra[1], path.as_os_str());
}
#[test]
fn staged_windows_preparation_retains_exact_timed_bytes_and_default_gate() {
    let timed = br#"// original bytes
{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"description":"timed","manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"b","hold_down_milliseconds":50},{"key_code":"c"}]}]}]}}]}"#;
    let prepared =
        PreparedConfig::read_with_mode(timed.as_slice(), ExecutionMode::StagedKeyboard).unwrap();
    assert_eq!(prepared.execution_mode(), ExecutionMode::StagedKeyboard);
    assert_eq!(prepared.bytes(), timed);
    assert_eq!(
        prepared.snapshot().complex[0].to[0].hold_down_ns,
        50_000_000
    );
    assert!(matches!(
        PreparedConfig::read(timed.as_slice()),
        Err(PreparationError::Runtime(RuntimeError::Engine(
            keyweave_engine::EngineError::UnsupportedActionTiming
        )))
    ));
}
#[test]
fn staged_worker_builder_binds_mode_before_config_handle_and_rejects_bad_modes_or_duplicates() {
    let path = std::env::current_dir()
        .unwrap()
        .join("unused config with spaces.json");
    let extra = worker_arguments_with_mode(
        &path,
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
        ExecutionMode::StagedKeyboard,
    )
    .unwrap();
    let mut args = vec![crate::WORKER_ARGUMENT.into(), "42".into(), "57".into()];
    args.extend(extra);
    let parsed = Arguments::parse(args.clone()).unwrap();
    assert_eq!(parsed.execution_mode, ExecutionMode::StagedKeyboard);
    assert_eq!(parsed.config, path);
    args.extend(["--keyweave-config-handle".into(), "512".into()]);
    assert_eq!(
        Arguments::parse(args.clone()).unwrap().config_handle,
        Some(512)
    );
    let mut unknown = args.clone();
    unknown[10] = "auto".into();
    assert!(Arguments::parse(unknown).is_err());
    let mut reordered = args.clone();
    reordered.swap(9, 11);
    assert!(Arguments::parse(reordered).is_err());
    args.extend(["--execution-mode".into(), "legacy".into()]);
    assert!(Arguments::parse(args).is_err());
}
