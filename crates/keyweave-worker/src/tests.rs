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
