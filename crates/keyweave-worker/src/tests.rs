#![allow(clippy::unwrap_used)]
use super::*;
use std::io::{self, Cursor};
fn arguments() -> Vec<OsString> {
    vec![
        WORKER_ARGUMENT.into(),
        "42".into(),
        "18446744073709551615".into(),
        "--config".into(),
        std::env::temp_dir().join("config with spaces.jsonc").into(),
        "--tag".into(),
        "4294967295".into(),
        "--identity".into(),
        "strict".into(),
    ]
}
#[test]
fn strict_owned_prefix_and_explicit_identity_are_preserved() {
    let args = Arguments::parse(arguments()).unwrap();
    assert_eq!(args.parent, 42);
    assert_eq!(args.run_id, u64::MAX);
    assert_eq!(args.tag.value(), u32::MAX as usize);
    assert_eq!(args.identity, IdentityPolicy::StrictUnique);
    let mut raw = arguments();
    raw[8] = "canonical-scan".into();
    assert_eq!(
        Arguments::parse(raw).unwrap().identity,
        IdentityPolicy::CanonicalScan
    );
}
#[test]
fn argument_rejection_precedes_any_admission_or_configuration() {
    for index in [1, 2, 6] {
        for bad in [
            "0",
            "01",
            "+1",
            "-1",
            " 1",
            "1 ",
            "１",
            "1\0",
            "18446744073709551616",
        ] {
            let mut args = arguments();
            args[index] = bad.into();
            assert!(Arguments::parse(args).is_err(), "index {index}: {bad}");
        }
    }
    for index in [1, 6] {
        let mut args = arguments();
        args[index] = "4294967296".into();
        assert!(Arguments::parse(args).is_err());
    }
    for (index, bad) in [
        (0, "worker"),
        (3, "--tag"),
        (4, "relative.json"),
        (5, "--config"),
        (7, "--identity= strict"),
        (8, "automatic"),
    ] {
        let mut args = arguments();
        args[index] = bad.into();
        assert!(Arguments::parse(args).is_err());
    }
    for len in 0..ARGUMENT_COUNT {
        assert!(Arguments::parse(arguments().into_iter().take(len)).is_err());
    }
    let mut args = arguments();
    args.push("--config".into());
    assert!(Arguments::parse(args).is_err());
}
#[test]
fn unbounded_argument_producer_is_consumed_only_to_rejection_boundary() {
    let mut seen = 0;
    let stream = std::iter::from_fn(|| {
        seen += 1;
        Some(OsString::from("x"))
    });
    assert!(Arguments::parse(stream).is_err());
    assert_eq!(seen, MAX_ARGUMENT_COUNT + 1);
}
#[cfg(unix)]
#[test]
fn non_unicode_absolute_path_is_not_lossily_replaced() {
    use std::os::unix::ffi::OsStringExt;
    let mut args = arguments();
    let path = OsString::from_vec(b"/tmp/keyweave-\xff.json".to_vec());
    args[4] = path.clone();
    assert_eq!(Arguments::parse(args).unwrap().config.as_os_str(), path);
}
#[test]
fn compiler_keeps_selected_profile_and_rejects_loss_instead_of_partial_activation() {
    let text = r#"{"profiles":[{"name":"selected","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
    let config = load_config(Cursor::new(text)).unwrap();
    assert_eq!(config.name, "selected");
    assert_eq!(config.generation, ConfigGeneration(1));
    assert_eq!(config.simple.len(), 1);
    let lossy = r#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"key_code":"a"},"to":[{"shell_command":"must never execute"}]}]}]}}]}"#;
    let Err(LoadError::Loss {
        disabled,
        diagnostics,
    }) = load_config(Cursor::new(lossy))
    else {
        panic!("must reject loss");
    };
    assert!(disabled > 0);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.pointer.ends_with("/to/0") && d.code == "KW-CAP-001")
    );
    assert!(matches!(
        load_config(Cursor::new(b"{bad")),
        Err(LoadError::Compile(_))
    ));
    assert!(matches!(
        load_config(Cursor::new([0xff])),
        Err(LoadError::Utf8(_))
    ));
}
#[test]
fn oversized_reader_is_bounded_and_io_failure_remains_distinct() {
    let mut read = 0;
    struct Endless<'a>(&'a mut usize);
    impl Read for Endless<'_> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            bytes.fill(b' ');
            *self.0 += bytes.len();
            Ok(bytes.len())
        }
    }
    assert!(matches!(
        load_config(Endless(&mut read)),
        Err(LoadError::TooLarge)
    ));
    assert_eq!(read, MAX_CONFIG_BYTES + 1);
    struct Failure;
    impl Read for Failure {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }
    let Err(LoadError::Io(e)) = load_config(Failure) else {
        panic!("preserve IO cause");
    };
    assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
}
fn clean() -> TerminalFacts {
    TerminalFacts {
        fault: false,
        progress_failed: false,
        shutdown_present: true,
        stopped: true,
        unhook_ok: true,
        recovery_ok: true,
        notifications_ok: true,
        access_lost: false,
        session_changed: false,
        untracked_input: false,
    }
}
#[test]
fn primary_and_each_independent_cleanup_failure_cannot_report_clean_exit() {
    assert_eq!(clean().exit(), WorkerExit::Clean);
    assert_eq!(
        TerminalFacts {
            fault: true,
            ..clean()
        }
        .exit(),
        WorkerExit::Loop
    );
    assert_eq!(
        TerminalFacts {
            progress_failed: true,
            ..clean()
        }
        .exit(),
        WorkerExit::Loop
    );
    for dirty in [
        TerminalFacts {
            shutdown_present: false,
            ..clean()
        },
        TerminalFacts {
            stopped: false,
            ..clean()
        },
        TerminalFacts {
            unhook_ok: false,
            ..clean()
        },
        TerminalFacts {
            recovery_ok: false,
            ..clean()
        },
        TerminalFacts {
            notifications_ok: false,
            ..clean()
        },
        TerminalFacts {
            access_lost: true,
            ..clean()
        },
        TerminalFacts {
            session_changed: true,
            ..clean()
        },
        TerminalFacts {
            untracked_input: true,
            ..clean()
        },
        TerminalFacts {
            fault: true,
            recovery_ok: false,
            ..clean()
        },
    ] {
        assert_eq!(dirty.exit(), WorkerExit::Cleanup);
    }
}
#[test]
fn inherited_handle_suffix_is_explicit_canonical_and_pointer_bounded() {
    let mut raw = arguments();
    raw.extend(["--keyweave-config-handle".into(), "512".into()]);
    assert_eq!(
        Arguments::parse(raw.clone()).unwrap().config_handle,
        Some(512)
    );
    assert_eq!(Arguments::parse(arguments()).unwrap().config_handle, None);
    for bad in [
        "0",
        "01",
        "+1",
        "-1",
        " 1",
        "1 ",
        "0x200",
        "18446744073709551616",
    ] {
        let mut args = raw.clone();
        args[10] = bad.into();
        assert!(Arguments::parse(args).is_err());
    }
    let mut args = raw.clone();
    args[10] = usize::MAX.to_string().into();
    assert!(Arguments::parse(args).is_err());
    let mut args = raw.clone();
    args[9] = "--arbitrary-handle".into();
    assert!(Arguments::parse(args).is_err());
    let mut args = raw;
    args.push("extra".into());
    assert!(Arguments::parse(args).is_err());
}

#[test]
fn mixed_private_arguments_roundtrip_builder_and_same_cold_scope_bytes() {
    use preparation::{CaptureScope, ExecutionMode, PreparedConfig, worker_arguments_with_scope};
    let bytes=br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"pointing_button":"button1"},"to":[{"key_code":"a"}]}]}]}"#;
    for identity in [IdentityPolicy::StrictUnique, IdentityPolicy::CanonicalScan] {
        let path = std::env::temp_dir().join("literal mixed config.json");
        let mut argv = vec![WORKER_ARGUMENT.into(), "42".into(), "7".into()];
        argv.extend(
            worker_arguments_with_scope(
                &path,
                InjectionTag::new(17).unwrap(),
                identity,
                ExecutionMode::StagedKeyboard,
                CaptureScope::StagedMixed,
            )
            .unwrap(),
        );
        argv.extend(["--keyweave-config-handle".into(), "512".into()]);
        assert_eq!(argv.len(), MAX_ARGUMENT_COUNT);
        let parsed = Arguments::parse(argv).unwrap();
        assert_eq!(parsed.config, path);
        assert_eq!(parsed.identity, identity);
        assert_eq!(parsed.config_handle, Some(512));
        assert_eq!(parsed.capture_scope, CaptureScope::StagedMixed);
        let prepared = PreparedConfig::read_for_generation_with_scope(
            bytes.as_slice(),
            ConfigGeneration(1),
            parsed.execution_mode,
            parsed.capture_scope,
        )
        .unwrap();
        assert_eq!(prepared.bytes(), bytes);
        assert_eq!(prepared.capture_scope(), CaptureScope::StagedMixed);
    }
}
#[test]
fn mixed_private_scope_requires_canonical_order_mode_and_owned_config_handle() {
    let mut valid = arguments();
    valid.extend([
        "--execution-mode".into(),
        "staged-keyboard".into(),
        "--capture-scope".into(),
        "staged-mixed".into(),
        "--keyweave-config-handle".into(),
        "512".into(),
    ]);
    for value in ["mixed", "StagedMixed", "staged-mixed ", "", "０"] {
        let mut args = valid.clone();
        args[12] = value.into();
        assert!(Arguments::parse(args).is_err());
    }
    let mut args = valid.clone();
    args.truncate(13);
    assert!(Arguments::parse(args).is_err());
    let mut args = valid.clone();
    args[10] = "legacy".into();
    assert!(Arguments::parse(args).is_err());
    let mut args = valid.clone();
    args.drain(9..11);
    assert!(Arguments::parse(args).is_err());
    let mut args = valid.clone();
    args.swap(9, 11);
    args.swap(10, 12);
    assert!(Arguments::parse(args).is_err());
    let mut args = valid.clone();
    args.swap(11, 13);
    args.swap(12, 14);
    assert!(Arguments::parse(args).is_err());
    let mut args = valid;
    args.extend(["--capture-scope".into(), "staged-mixed".into()]);
    assert!(Arguments::parse(args).is_err());
    assert!(
        preparation::worker_arguments_with_scope(
            &std::env::temp_dir().join("config"),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::StrictUnique,
            preparation::ExecutionMode::Legacy,
            preparation::CaptureScope::StagedMixed
        )
        .is_err()
    );
}
#[test]
fn keyboard_scope_remains_default_with_legacy_and_staged_arguments() {
    assert_eq!(
        Arguments::parse(arguments()).unwrap().capture_scope,
        preparation::CaptureScope::Keyboard
    );
    let mut args = arguments();
    args.extend(["--execution-mode".into(), "staged-keyboard".into()]);
    assert_eq!(
        Arguments::parse(args).unwrap().capture_scope,
        preparation::CaptureScope::Keyboard
    );
}
#[test]
fn mouse_unhook_evidence_cannot_erase_keyboard_or_other_terminal_failures() {
    for keyboard in [true, false] {
        for mouse in [true, false] {
            let facts = TerminalFacts {
                unhook_ok: keyboard,
                ..clean()
            }
            .with_mouse_unhook(mouse);
            assert_eq!(facts.unhook_ok, keyboard && mouse);
            let exit = facts.native_exit();
            if keyboard && mouse {
                assert_eq!(exit.raw(), WorkerExit::Clean as u32);
            } else {
                assert!(
                    exit.evidence()
                        .unwrap()
                        .contains(TerminalIssue::UnhookUnconfirmed)
                );
                assert_eq!(facts.exit(), WorkerExit::Cleanup);
            }
            let dirty = TerminalFacts {
                fault: true,
                progress_failed: true,
                access_lost: true,
                unhook_ok: keyboard,
                ..clean()
            }
            .with_mouse_unhook(mouse)
            .native_exit()
            .evidence()
            .unwrap();
            assert!(
                dirty.contains(TerminalIssue::Fault)
                    && dirty.contains(TerminalIssue::ProgressFailed)
                    && dirty.contains(TerminalIssue::AccessLost)
            );
            assert_eq!(
                dirty.contains(TerminalIssue::UnhookUnconfirmed),
                !(keyboard && mouse)
            );
        }
    }
}
#[cfg(unix)]
#[test]
fn mixed_builder_preserves_literal_non_unicode_path() {
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(OsString::from_vec(b"/tmp/mixed-\xff.json".to_vec()));
    let extra = preparation::worker_arguments_with_scope(
        &path,
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
        preparation::ExecutionMode::StagedKeyboard,
        preparation::CaptureScope::StagedMixed,
    )
    .unwrap();
    assert_eq!(extra[1], path.as_os_str());
    let mut argv = vec![WORKER_ARGUMENT.into(), "42".into(), "7".into()];
    argv.extend(extra);
    argv.extend(["--keyweave-config-handle".into(), "512".into()]);
    assert_eq!(Arguments::parse(argv).unwrap().config, path);
}
