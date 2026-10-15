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

const CHORD: &[u8] = br#"// exact chord bytes
{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"description":"chord","manipulators":[{"type":"basic","from":{"simultaneous":[{"key_code":"a"},{"key_code":"b"}],"simultaneous_options":{"key_up_when":"all","to_after_key_up":[{"key_code":"d"}]}},"to":[{"key_code":"c"}]}]}]}}]}"#;
#[test]
fn staged_chord_windows_preparation_binds_exact_bytes_generation_and_default_rejection() {
    let prepared = PreparedConfig::from_bytes_for_generation_with_mode(
        CHORD.to_vec(),
        ConfigGeneration(2),
        ExecutionMode::StagedKeyboard,
    )
    .unwrap();
    assert_eq!(prepared.bytes(), CHORD);
    assert_eq!(prepared.snapshot().generation, ConfigGeneration(2));
    assert_eq!(prepared.execution_mode(), ExecutionMode::StagedKeyboard);
    assert_eq!(
        prepared.snapshot().complex[0]
            .extras
            .simultaneous
            .as_ref()
            .unwrap()
            .after_key_up[0]
            .key(),
        Some("d".parse().unwrap())
    );
    assert!(matches!(
        PreparedConfig::read(CHORD),
        Err(PreparationError::Load(LoadError::Loss { .. }))
    ));
    assert!(crate::load_config(CHORD).is_err());
}
#[test]
fn staged_chord_windows_preparation_checks_native_group_after_up_encoding() {
    let bytes = std::str::from_utf8(CHORD)
        .unwrap()
        .replace("\"d\"", "\"print_screen\"");
    assert!(matches!(
        PreparedConfig::read_with_mode(bytes.as_bytes(), ExecutionMode::StagedKeyboard),
        Err(PreparationError::Runtime(RuntimeError::Key(_)))
    ));
    let bytes = std::str::from_utf8(CHORD).unwrap().replace(
        "\"key_code\":\"d\"",
        "\"consumer_key_code\":\"volume_increment\"",
    );
    assert!(matches!(
        PreparedConfig::read_with_mode(bytes.as_bytes(), ExecutionMode::StagedKeyboard),
        Err(PreparationError::Load(LoadError::Loss { .. }))
    ));
}

const MIXED:&[u8]=br#"// exact mixed bytes
{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"pointing_button":"button1"},"to":[{"pointing_button":"button2"}]}]}]}}]}"#;
#[test]
fn mixed_scope_preserves_exact_bytes_and_requires_explicit_staged_admission() {
    let prepared = PreparedConfig::read_for_generation_with_scope(
        MIXED,
        ConfigGeneration(7),
        ExecutionMode::StagedKeyboard,
        CaptureScope::StagedMixed,
    )
    .unwrap();
    assert_eq!(prepared.capture_scope(), CaptureScope::StagedMixed);
    assert_eq!(prepared.execution_mode(), ExecutionMode::StagedKeyboard);
    assert_eq!(prepared.bytes(), MIXED);
    assert_eq!(prepared.snapshot().generation, ConfigGeneration(7));
    assert_eq!(
        prepared.snapshot().complex[0].from,
        keyweave_types::KeyId::new(keyweave_types::BUTTON_PAGE, 1)
    );
    assert!(PreparedConfig::read_with_mode(MIXED, ExecutionMode::StagedKeyboard).is_err());
    assert!(!BackendCapabilities::windows_hook().pointing_buttons);
}
#[test]
fn mixed_preparation_rejects_unavailable_button_sources_and_all_output_branches() {
    for source in ["button6", "button32"] {
        let text = std::str::from_utf8(MIXED)
            .unwrap()
            .replace("button1", source);
        assert!(
            PreparedConfig::from_bytes_for_generation_with_scope(
                text.into_bytes(),
                ConfigGeneration(1),
                ExecutionMode::StagedKeyboard,
                CaptureScope::StagedMixed
            )
            .is_err()
        );
    }
    for branch in ["to", "to_if_alone", "to_if_held_down", "to_after_key_up"] {
        let text = std::str::from_utf8(MIXED)
            .unwrap()
            .replace("button2", "button6")
            .replace("\"to\":", &format!("\"{branch}\":"));
        assert!(
            PreparedConfig::from_bytes_for_generation_with_scope(
                text.into_bytes(),
                ConfigGeneration(1),
                ExecutionMode::StagedKeyboard,
                CaptureScope::StagedMixed
            )
            .is_err(),
            "{branch}"
        );
    }
}
#[test]
fn mixed_generation_and_mode_reject_before_reader_access() {
    struct Never;
    impl Read for Never {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("invalid cold metadata must reject before read")
        }
    }
    assert!(matches!(
        PreparedConfig::read_for_generation_with_scope(
            Never,
            ConfigGeneration(0),
            ExecutionMode::StagedKeyboard,
            CaptureScope::StagedMixed
        ),
        Err(PreparationError::Load(LoadError::Generation))
    ));
    assert!(matches!(
        PreparedConfig::read_for_generation_with_scope(
            Never,
            ConfigGeneration(1),
            ExecutionMode::Legacy,
            CaptureScope::StagedMixed
        ),
        Err(PreparationError::Scope)
    ));
    assert!(matches!(
        PreparedConfig::from_bytes_for_generation_with_scope(
            vec![0xff],
            ConfigGeneration(1),
            ExecutionMode::Legacy,
            CaptureScope::StagedMixed
        ),
        Err(PreparationError::Scope)
    ));
}
#[test]
fn windows_default_scope_and_same_keyboard_bytes_remain_explicit() {
    for mode in [ExecutionMode::Legacy, ExecutionMode::StagedKeyboard] {
        let prepared = PreparedConfig::read_with_mode(JSONC, mode).unwrap();
        assert_eq!(prepared.capture_scope(), CaptureScope::Keyboard);
        assert_eq!(prepared.bytes(), JSONC);
    }
    let prepared = PreparedConfig::read_for_generation_with_scope(
        JSONC,
        ConfigGeneration(1),
        ExecutionMode::StagedKeyboard,
        CaptureScope::StagedMixed,
    )
    .unwrap();
    assert_eq!(prepared.capture_scope(), CaptureScope::StagedMixed);
}

#[test]
fn mixed_simultaneous_cold_checks_every_native_button_member_and_default_gate() {
    for first in [
        r#"{"key_code":"a"},{"pointing_button":"button1"}"#,
        r#"{"pointing_button":"button1"},{"key_code":"a"}"#,
    ] {
        let text = format!(
            r#"{{"profiles":[{{"selected":true,"complex_modifications":{{"rules":[{{"manipulators":[{{"type":"basic","from":{{"simultaneous":[{first}]}},"to":[{{"pointing_button":"button2"}}]}}]}}]}}}}]}}"#
        );
        let prepared = PreparedConfig::from_bytes_for_generation_with_scope(
            text.as_bytes().to_vec(),
            ConfigGeneration(1),
            ExecutionMode::StagedKeyboard,
            CaptureScope::StagedMixed,
        )
        .unwrap();
        assert_eq!(
            prepared.snapshot().complex[0]
                .extras
                .simultaneous
                .as_ref()
                .unwrap()
                .keys
                .len(),
            2
        );
        assert!(
            PreparedConfig::read_with_mode(text.as_bytes(), ExecutionMode::StagedKeyboard).is_err()
        );
        let unsupported = text.replace("button1", "button6");
        assert!(
            PreparedConfig::from_bytes_for_generation_with_scope(
                unsupported.into_bytes(),
                ConfigGeneration(1),
                ExecutionMode::StagedKeyboard,
                CaptureScope::StagedMixed
            )
            .is_err()
        );
    }
}
#[test]
fn mixed_simultaneous_json_runs_actual_controller_and_windows_output_ledger() {
    use keyweave_types::{BUTTON_PAGE, CaptureDecision, KeyEdge, KeyId, MonoTime};
    use keyweave_windows::{
        hook::HookEvent,
        keys::ResolvedKey,
        mixed::{NativePacket, SendMixedBackend},
        mixed_actor::MixedSessionActor,
        mouse_hook::{MouseHookEvent, MouseInputEvent},
        pump::{Insertion, SubmitError},
    };
    struct Sender;
    impl SendMixedBackend for Sender {
        fn submit_mixed(&mut self, packets: &[NativePacket]) -> Result<Insertion, SubmitError> {
            Ok(Insertion {
                accepted: packets.len(),
                last_error: None,
            })
        }
    }
    for pointer_first in [true, false] {
        let first = if pointer_first {
            r#"{"pointing_button":"button1"},{"key_code":"a"}"#
        } else {
            r#"{"key_code":"a"},{"pointing_button":"button1"}"#
        };
        let text = format!(
            r#"{{"profiles":[{{"selected":true,"complex_modifications":{{"rules":[{{"manipulators":[{{"type":"basic","from":{{"simultaneous":[{first}],"simultaneous_options":{{"key_up_when":"all"}}}},"to":[{{"pointing_button":"button2"}}]}}]}}]}}}}]}}"#
        );
        let prepared = PreparedConfig::from_bytes_for_generation_with_scope(
            text.into_bytes(),
            ConfigGeneration(1),
            ExecutionMode::StagedKeyboard,
            CaptureScope::StagedMixed,
        )
        .unwrap();
        let mut actor = MixedSessionActor::new(
            Sender,
            InjectionTag::new(1).unwrap(),
            prepared.snapshot().clone(),
        )
        .unwrap();
        actor.start_after_neutral().unwrap();
        for is_pointer in [pointer_first, !pointer_first] {
            if is_pointer {
                actor
                    .mouse(
                        MouseHookEvent {
                            input: MouseInputEvent::Button {
                                key: KeyId::new(BUTTON_PAGE, 1),
                                edge: KeyEdge::Down,
                            },
                            binding: None,
                            pair_bindings: None,
                        },
                        MonoTime(0),
                    )
                    .unwrap();
            } else {
                actor
                    .key(
                        HookEvent {
                            identity: ResolvedKey {
                                key: KeyId::new(7, 4),
                                known_aliases: false,
                            },
                            edge: KeyEdge::Down,
                            binding: None,
                        },
                        MonoTime(0),
                    )
                    .unwrap();
            }
        }
        actor.tick(MonoTime(50_000_000)).unwrap();
        assert!(
            actor
                .output()
                .ledger()
                .submitted()
                .contains(&KeyId::new(BUTTON_PAGE, 2))
        );
        assert_eq!(actor.output().ledger().submitted().len(), 1);
        actor
            .key(
                HookEvent {
                    identity: ResolvedKey {
                        key: KeyId::new(7, 4),
                        known_aliases: false,
                    },
                    edge: KeyEdge::Up,
                    binding: Some(CaptureDecision::Capture),
                },
                MonoTime(50_000_001),
            )
            .unwrap();
        actor
            .mouse(
                MouseHookEvent {
                    input: MouseInputEvent::Button {
                        key: KeyId::new(BUTTON_PAGE, 1),
                        edge: KeyEdge::Up,
                    },
                    binding: Some(CaptureDecision::Capture),
                    pair_bindings: None,
                },
                MonoTime(50_000_002),
            )
            .unwrap();
        actor.tick(MonoTime(50_000_002)).unwrap();
        assert!(actor.output().ledger().submitted().is_empty());
        actor.stop_after_unhook().unwrap();
    }
}
