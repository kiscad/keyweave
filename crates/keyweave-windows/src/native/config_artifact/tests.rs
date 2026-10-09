#![allow(clippy::unwrap_used)]
use super::api_mock::{MOCK, Mock};
use super::*;
fn create(bytes: &[u8]) -> ConfigArtifact {
    ConfigArtifact::create(&std::env::temp_dir(), bytes).unwrap()
}
#[test]
fn sealed_image_has_unique_read_only_rewound_reader_and_independent_close() {
    let data = vec![0x61; CHUNK + 17];
    let mut artifact = create(&data);
    assert!(artifact.path().is_ok());
    let mut reader = artifact.prepare_reader().unwrap();
    assert_eq!(
        artifact.prepare_reader().unwrap_err().cause,
        ArtifactError::ReaderAttempted
    );
    let mut out = vec![];
    reader.read_to_end(&mut out).unwrap();
    assert_eq!(out, data);
    assert!(artifact.close().unwrap().complete());
    assert!(artifact.path().is_err());
    reader.close().unwrap();
    reader.close().unwrap();
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.closes, vec![0x100, 0x200]);
        assert_eq!(m.calls.iter().filter(|c| **c == "duplicate").count(), 1);
    });
}
#[test]
fn callback_factory_reader_and_close_reject_before_mutation_or_native_io() {
    let mut artifact = create(b"{}");
    let mut reader = artifact.prepare_reader().unwrap();
    let before = MOCK.with(|m| m.borrow().calls.len());
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(
            ConfigArtifact::create(&std::env::temp_dir(), b"{}")
                .unwrap_err()
                .cause,
            ArtifactError::InCallback
        );
        assert_eq!(
            artifact.prepare_reader().unwrap_err().cause,
            ArtifactError::InCallback
        );
        assert_eq!(artifact.close().unwrap_err(), ArtifactError::InCallback);
        assert!(reader.read(&mut [0; 2]).is_err());
        assert_eq!(reader.close().unwrap_err(), ArtifactError::InCallback);
    });
    MOCK.with(|m| assert_eq!(m.borrow().calls.len(), before));
    assert!(artifact.path().is_ok());
    reader.close().unwrap();
    artifact.close().unwrap();
}
#[test]
fn size_path_and_rng_preflight_do_not_create_or_overwrite_a_file() {
    assert_eq!(
        ConfigArtifact::create(Path::new("relative"), b"{}")
            .unwrap_err()
            .cause,
        ArtifactError::Path
    );
    assert_eq!(
        ConfigArtifact::create(&std::env::temp_dir(), &vec![0; MAX_ARTIFACT_BYTES + 1])
            .unwrap_err()
            .cause,
        ArtifactError::Size
    );
    MOCK.with(|m| assert!(m.borrow().calls.is_empty()));
    MOCK.with(|m| m.borrow_mut().fail = Some("rng"));
    assert_eq!(
        ConfigArtifact::create(&std::env::temp_dir(), b"{}")
            .unwrap_err()
            .cause,
        ArtifactError::Random(-1)
    );
    MOCK.with(|m| assert_eq!(m.borrow().calls, vec!["rng"]));
}
#[test]
fn every_prepublication_api_failure_retains_cause_and_rolls_back_owned_resources() {
    for failure in [
        "security",
        "create",
        "free-security",
        "flags",
        "final-path",
        "volume-path",
        "volume-info",
        "write",
    ] {
        MOCK.with(|m| {
            *m.borrow_mut() = Mock {
                fail: Some(failure),
                ..Mock::default()
            }
        });
        let mut error = ConfigArtifact::create(&std::env::temp_dir(), b"{}").unwrap_err();
        assert!(matches!(error.cause, ArtifactError::Api { code: 5, .. }));
        assert!(error.owner.as_ref().unwrap().path().is_err());
        MOCK.with(|m| m.borrow_mut().fail = None);
        assert!(error.owner.as_mut().unwrap().close().unwrap().complete());
    }
}
#[test]
fn metadata_rejects_inheritable_non_disk_remote_and_non_acl_before_any_write() {
    for mock in [
        Mock {
            flags: 1,
            ..Mock::default()
        },
        Mock {
            file_type: Some(3),
            ..Mock::default()
        },
        Mock {
            drive: Some(4),
            ..Mock::default()
        },
        Mock {
            acl: Some(0),
            ..Mock::default()
        },
        Mock {
            final_path: Some("\\\\?\\UNC\\server\\share\\config".into()),
            ..Mock::default()
        },
    ] {
        MOCK.with(|m| *m.borrow_mut() = mock);
        let error = ConfigArtifact::create(&std::env::temp_dir(), b"private bytes").unwrap_err();
        assert!(matches!(
            error.cause,
            ArtifactError::Metadata | ArtifactError::Volume
        ));
        assert!(error.rollback.unwrap().complete());
        MOCK.with(|m| assert!(!m.borrow().calls.contains(&"write")));
    }
}
#[test]
fn write_zero_bad_count_and_operation_budget_never_publish_partial_file() {
    for (chunk, bad, expected) in [
        (0, false, ArtifactError::WriteProgress),
        (CHUNK, true, ArtifactError::WriteProgress),
        (1, false, ArtifactError::WriteBudget),
    ] {
        MOCK.with(|m| {
            *m.borrow_mut() = Mock {
                chunk: Some(chunk),
                bad_write: bad,
                ..Mock::default()
            }
        });
        let error = ConfigArtifact::create(&std::env::temp_dir(), &vec![3; 257]).unwrap_err();
        assert_eq!(error.cause, expected);
        assert!(error.rollback.unwrap().complete());
        assert!(error.owner.unwrap().path().is_err());
        MOCK.with(|m| {
            assert!(m.borrow().calls.iter().filter(|c| **c == "write").count() <= WRITE_BUDGET)
        });
    }
}
#[test]
fn explicit_close_failure_keeps_owner_and_security_errors_independent_for_retry() {
    let mut artifact = create(b"{}");
    MOCK.with(|m| m.borrow_mut().fail = Some("close"));
    let result = artifact.close().unwrap();
    assert!(!result.file_closed && result.security_released);
    assert!(result.file_error.is_some());
    assert!(artifact.path().is_ok());
    MOCK.with(|m| m.borrow_mut().fail = None);
    assert!(artifact.close().unwrap().complete());
}
#[test]
fn reader_failure_latches_no_retry_and_read_error_does_not_reuse_unknown_bytes() {
    let mut artifact = create(b"abc");
    MOCK.with(|m| m.borrow_mut().fail = Some("duplicate"));
    assert!(artifact.prepare_reader().is_err());
    MOCK.with(|m| m.borrow_mut().fail = None);
    assert_eq!(
        artifact.prepare_reader().unwrap_err().cause,
        ArtifactError::ReaderAttempted
    );
    artifact.close().unwrap();
    MOCK.with(|m| *m.borrow_mut() = Mock::default());
    let mut artifact = create(b"abc");
    let mut reader = artifact.prepare_reader().unwrap();
    MOCK.with(|m| m.borrow_mut().bad_read = true);
    assert!(reader.read(&mut [0; 3]).is_err());
    let n = MOCK.with(|m| m.borrow().calls.len());
    assert!(reader.read(&mut [0; 3]).is_err());
    MOCK.with(|m| assert_eq!(m.borrow().calls.len(), n));
    reader.close().unwrap();
    artifact.close().unwrap();
}
#[test]
fn native_layout_and_local_path_normalization_are_explicit() {
    assert_eq!(
        std::mem::size_of::<SecurityAttributes>(),
        if std::mem::size_of::<usize>() == 8 {
            24
        } else {
            12
        }
    );
    assert_eq!(std::mem::size_of::<LargeInteger>(), 8);
    for path in ["\\\\?\\C:\\file", "\\\\?\\z:\\目录\\file"] {
        assert!(local_path(&path.encode_utf16().collect::<Vec<_>>()));
    }
    for path in [
        "C:\\file",
        "\\\\?\\UNC\\s\\f",
        "\\\\.\\pipe\\x",
        "\\\\?\\C:\\file\0oops",
    ] {
        assert!(!local_path(&path.encode_utf16().collect::<Vec<_>>()));
    }
}

#[test]
fn failed_security_conversion_does_not_claim_or_free_unspecified_output() {
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.fail = Some("security");
        m.dirty_failure_output = true;
    });
    let error = ConfigArtifact::create(&std::env::temp_dir(), b"{}").unwrap_err();
    assert!(error.rollback.unwrap().complete());
    MOCK.with(|m| assert!(!m.borrow().calls.contains(&"free-security")));
}
#[test]
fn reader_validation_failure_retains_primary_cleanup_error_and_unique_owner() {
    let mut artifact = create(b"abc");
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.fail = Some("rewind");
        m.close_fail = true;
    });
    let mut error = artifact.prepare_reader().unwrap_err();
    assert_eq!(
        error.cause,
        ArtifactError::Api {
            call: "config reader rewind",
            code: 5
        }
    );
    assert_eq!(
        error.rollback,
        Some(ArtifactError::Api {
            call: "close config reader",
            code: 5
        })
    );
    assert!(error.owner.as_mut().unwrap().read(&mut [0; 3]).is_err());
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.fail = None;
        m.close_fail = false;
    });
    error.owner.as_mut().unwrap().close().unwrap();
    assert!(artifact.close().unwrap().complete());
    assert!(error.rollback.is_some());
}
#[test]
fn transfer_rewinds_after_local_read_and_permanently_locks_reader_io() {
    let mut artifact = create(b"unchanged image");
    let mut reader = artifact.prepare_reader().unwrap();
    reader.read_exact(&mut [0; 4]).unwrap();
    assert_eq!(MOCK.with(|m| m.borrow().cursor), 4);
    assert_eq!(reader.prepare_transfer().unwrap() as usize, 0x200);
    assert_eq!(MOCK.with(|m| m.borrow().cursor), 0);
    let before = MOCK.with(|m| m.borrow().calls.len());
    assert!(reader.read(&mut [0; 1]).is_err());
    assert_eq!(reader.prepare_transfer(), Err(ArtifactError::NotSealed));
    assert_eq!(MOCK.with(|m| m.borrow().calls.len()), before);
    reader.close().unwrap();
}
#[test]
fn failed_transfer_and_callback_cannot_rewind_or_reuse_reader() {
    let mut artifact = create(b"image");
    let mut reader = artifact.prepare_reader().unwrap();
    let before = MOCK.with(|m| m.borrow().calls.len());
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(reader.prepare_transfer(), Err(ArtifactError::InCallback));
    });
    assert_eq!(MOCK.with(|m| m.borrow().calls.len()), before);
    MOCK.with(|m| m.borrow_mut().fail = Some("rewind"));
    assert!(matches!(
        reader.prepare_transfer(),
        Err(ArtifactError::Api { .. })
    ));
    MOCK.with(|m| m.borrow_mut().fail = None);
    let before = MOCK.with(|m| m.borrow().calls.len());
    assert!(reader.prepare_transfer().is_err());
    assert!(reader.read(&mut [0; 1]).is_err());
    assert_eq!(MOCK.with(|m| m.borrow().calls.len()), before);
    reader.close().unwrap();
}
