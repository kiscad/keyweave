#![allow(clippy::unwrap_used)]
use super::*;
#[test]
fn metadata_only_synchronous_diagnostic_never_claims_or_duplicates() {
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.native_mode = Some(0x20);
        m.peer = Some(77);
    });
    check_synchronous_progress_pipe_rejected().unwrap();
    assert!(!CLAIMED.get());
    assert!(api::MOCK.with(|m| m.borrow().duplicates.is_empty()));
}
#[test]
fn scalar_layouts_match_sdk_and_documented_native_payloads() {
    use std::mem::{align_of, offset_of};
    assert_eq!(
        size_of::<ProcessEntry>(),
        if size_of::<usize>() == 8 { 568 } else { 556 }
    );
    assert_eq!(
        offset_of!(ProcessEntry, parent),
        if size_of::<usize>() == 8 { 32 } else { 24 }
    );
    assert_eq!(size_of::<ProcessList>(), 8 + size_of::<usize>());
    assert_eq!(size_of::<IoStatusBlock>(), 2 * size_of::<usize>());
    assert_eq!(align_of::<IoStatusBlock>(), size_of::<usize>());
    assert_eq!(offset_of!(IoStatusBlock, information), size_of::<usize>());
    assert_eq!(offset_of!(Query, bytes), size_of::<IoStatusBlock>());
}
#[test]
fn admitted_worker_owns_only_noninherited_private_progress_copies() {
    let startup = WorkerStartup::claim(76, 42).unwrap();
    assert_eq!(startup.guard().parent_pid(), 76);
    assert_eq!(startup.guard().worker_pid(), 77);
    assert_eq!(startup.run_id(), 42);
    assert!(matches!(
        WorkerStartup::claim(76, 42),
        Err(WorkerStartupError::AlreadyClaimed)
    ));
    let (_guard, run, mut reader, mut writer) = startup.into_parts();
    assert_eq!(run, 42);
    assert_eq!(reader.transfer_state().0 as usize, 501);
    assert_eq!(writer.transfer_state().0 as usize, 502);
    assert_ne!(reader.transfer_state().1, writer.transfer_state().1);
    assert!(reader.close().unwrap().complete() && writer.close().unwrap().complete());
    api::MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.duplicates, [100, 210]);
        assert_eq!(m.closes, [600]);
        assert_eq!(
            m.queries,
            [(100, 16), (100, 8), (100, 9), (210, 16), (210, 8), (210, 9)]
        );
    });
}
#[test]
fn invalid_identity_latches_failure_without_os_or_retry() {
    for (parent, run) in [(0, 42), (76, 0), (77, 42)] {
        std::thread::spawn(move || {
            assert!(matches!(
                WorkerStartup::claim(parent, run),
                Err(WorkerStartupError::Identity)
            ));
            assert!(matches!(
                WorkerStartup::claim(76, 42),
                Err(WorkerStartupError::AlreadyClaimed)
            ));
            assert!(api::MOCK.with(|m| m.borrow().duplicates.is_empty()));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn callback_cannot_claim_or_query_anything() {
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(matches!(
            WorkerStartup::claim(76, 42),
            Err(WorkerStartupError::InCallback)
        ));
    });
    assert!(api::MOCK.with(|m| m.borrow().calls.is_empty()));
    assert!(WorkerStartup::claim(76, 42).is_ok());
}
#[test]
fn all_metadata_api_failures_abort_before_cloning() {
    for fail in [
        "snapshot",
        "process-first",
        "close-snapshot",
        "job-limits",
        "job-processes",
        "pipe-info",
        "pipe-state",
        "pipe-server",
        "pipe-client",
        "clear-inherit",
        "duplicate",
    ] {
        std::thread::spawn(move || {
            api::MOCK.with(|m| m.borrow_mut().fail = Some(fail));
            assert!(matches!(
                WorkerStartup::claim(76, 42),
                Err(WorkerStartupError::Api { code: 5, .. })
            ));
            assert!(api::MOCK.with(|m| m.borrow().duplicates.is_empty()));
            assert!(matches!(
                WorkerStartup::claim(76, 42),
                Err(WorkerStartupError::AlreadyClaimed)
            ));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn changed_parent_job_or_standard_binding_is_rejected() {
    for kind in 0..12 {
        std::thread::spawn(move || {
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                match kind {
                    0 => m.parent = Some(75),
                    1 => m.missing_current = true,
                    2 => m.flags = Some(8),
                    3 => m.active = Some(2),
                    4 => m.list_pid = Some(78),
                    5 => m.job_length = Some(0),
                    6 => m.startup_flags = Some(0),
                    7 => m.std_mismatch = true,
                    8 => m.file_type = Some(1),
                    9 => m.list_assigned = Some(2),
                    10 => m.list_count = Some(0),
                    _ => m.list_count = Some(2),
                }
            });
            assert!(WorkerStartup::claim(76, 42).is_err());
            assert!(api::MOCK.with(|m| m.borrow().duplicates.is_empty()));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn synchronous_wrong_access_names_and_pipe_modes_are_rejected() {
    for kind in 0..11 {
        std::thread::spawn(move || {
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                match kind {
                    0 => m.native_mode = Some(0x10),
                    1 => m.native_mode = Some(0x20),
                    2 => m.native_mode = Some(4),
                    3 => m.access = Some(3 | 0x80),
                    4 => m.access = Some(1),
                    5 => m.same_name = true,
                    6 => m.bad_name = true,
                    7 => m.pipe_flags = Some(1),
                    8 => m.pipe_mode = Some(1),
                    9 => m.instances = Some(2),
                    _ => m.peer = Some(75),
                }
            });
            assert!(WorkerStartup::claim(76, 42).is_err());
            assert!(api::MOCK.with(|m| m.borrow().duplicates.is_empty()));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn each_direction_rejects_synchronous_or_wrong_access_before_duplication() {
    for handle in [100, 210] {
        for class in [16, 8] {
            std::thread::spawn(move || {
                api::MOCK.with(|m| {
                    let mut m = m.borrow_mut();
                    m.query_class = Some(class);
                    m.query_handle = Some(handle);
                    m.native_mode = Some(0x20);
                    m.access = Some(0x83);
                });
                let error = WorkerStartup::claim(76, 42).unwrap_err();
                assert!(matches!(
                    (class, error),
                    (16, WorkerStartupError::PipeMode) | (8, WorkerStartupError::PipeAccess)
                ));
                api::MOCK.with(|m| {
                    let m = m.borrow();
                    assert_eq!(m.queries.last(), Some(&(handle, class)));
                    assert!(m.duplicates.is_empty());
                });
            })
            .join()
            .unwrap();
        }
    }
}
#[test]
fn native_failure_retains_output_for_late_completion_and_cannot_retry() {
    for status in [0x103, -1073741790, 1] {
        std::thread::spawn(move || {
            api::MOCK.with(|m| m.borrow_mut().status = Some(status));
            assert!(matches!(
                WorkerStartup::claim(76, 42),
                Err(WorkerStartupError::NativeQuery { class: 16, .. })
            ));
            api::MOCK.with(|m| {
                let m = m.borrow();
                let (io, out) = m.retained.unwrap();
                // SAFETY: failed query retains its stable heap/UnsafeCell outputs;
                // mock completes after claim returned, as a real pending OS write.
                unsafe {
                    *(out as *mut u32) = 0;
                    (*(io as *mut IoStatusBlock)).information = 4;
                    (*(io as *mut IoStatusBlock)).status.status = 0;
                }
            });
            assert!(matches!(
                WorkerStartup::claim(76, 42),
                Err(WorkerStartupError::AlreadyClaimed)
            ));
            assert!(api::MOCK.with(|m| m.borrow().duplicates.is_empty()));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn malformed_native_completion_is_not_async_evidence() {
    for (class, count, status) in [
        (16, 0, 0),
        (16, 5, 0),
        (16, 4, 5),
        (8, 3, 0),
        (9, 4, 0),
        (9, 257, 0),
    ] {
        std::thread::spawn(move || {
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                m.information = Some(count);
                m.io_status = Some(status);
                m.query_class = Some(class);
            });
            assert!(WorkerStartup::claim(76, 42).is_err());
            assert_eq!(
                api::MOCK.with(|m| m.borrow().queries.last().copied().map(|entry| entry.1)),
                Some(class)
            );
            assert!(api::MOCK.with(|m| m.borrow().duplicates.is_empty()));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn inherited_config_is_one_shot_read_only_and_never_closes_rejected_handles() {
    use super::super::config_artifact::api_mock::MOCK as FILE;
    let mut startup = WorkerStartup::claim(76, 42).unwrap();
    FILE.with(|m| {
        let mut m = m.borrow_mut();
        m.flags = 1;
        m.data = b"sealed bytes".to_vec();
        m.cursor = 5;
    });
    let mut reader = startup.claim_config_reader(512).unwrap();
    assert!(matches!(
        startup.claim_config_reader(512),
        Err(WorkerStartupError::ConfigClaimed)
    ));
    use std::io::Read;
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"sealed bytes");
    reader.close().unwrap();
    FILE.with(|m| assert_eq!(m.borrow().closes, [512]));
}
#[test]
fn inherited_config_callback_does_not_claim_or_query() {
    use super::super::config_artifact::api_mock::MOCK as FILE;
    let mut startup = WorkerStartup::claim(76, 42).unwrap();
    let before = api::MOCK.with(|m| m.borrow().calls.len());
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(matches!(
            startup.claim_config_reader(512),
            Err(WorkerStartupError::InCallback)
        ));
    });
    assert_eq!(api::MOCK.with(|m| m.borrow().calls.len()), before);
    FILE.with(|m| {
        assert!(m.borrow().calls.is_empty());
        m.borrow_mut().flags = 1;
    });
    startup.claim_config_reader(512).unwrap().close().unwrap();
}
#[test]
fn inherited_config_rejects_invalid_identity_access_mode_and_flags_without_ownership() {
    use super::super::config_artifact::api_mock::MOCK as FILE;
    for kind in 0..9 {
        std::thread::spawn(move || {
            let mut startup = WorkerStartup::claim(76, 42).unwrap();
            FILE.with(|m| m.borrow_mut().flags = 1);
            let value = match kind {
                0 => 0,
                1 => usize::MAX,
                2 => 100,
                _ => 512,
            };
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                if kind == 3 {
                    m.access = Some(0x12019f);
                }
                if kind == 4 {
                    m.native_mode = Some(0);
                }
                if kind == 5 {
                    m.native_mode = Some(0x10);
                }
                if kind == 8 {
                    m.fail = Some("clear-inherit");
                }
            });
            FILE.with(|m| {
                let mut m = m.borrow_mut();
                if kind == 6 {
                    m.flags = 0;
                }
                if kind == 7 {
                    m.drive = Some(4);
                }
            });
            assert!(startup.claim_config_reader(value).is_err());
            assert!(matches!(
                startup.claim_config_reader(512),
                Err(WorkerStartupError::ConfigClaimed)
            ));
            FILE.with(|m| assert!(m.borrow().closes.is_empty()));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn inherited_config_pending_query_keeps_stable_outputs_without_ownership() {
    use super::super::config_artifact::api_mock::MOCK as FILE;
    let mut startup = WorkerStartup::claim(76, 42).unwrap();
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.query_handle = Some(512);
        m.query_class = Some(8);
        m.status = Some(0x103);
    });
    assert!(matches!(
        startup.claim_config_reader(512),
        Err(WorkerStartupError::NativeQuery {
            class: 8,
            status: 0x103
        })
    ));
    let (io, data) = api::MOCK.with(|m| m.borrow().retained.unwrap());
    // SAFETY: production deliberately retained these unique initialized query
    // allocations on STATUS_PENDING. Fake OS completion proves they remain live.
    unsafe {
        (*(io as *mut IoStatusBlock)).information = 4;
        *(data as *mut u8) = 0x89;
    }
    FILE.with(|m| assert!(m.borrow().calls.is_empty() && m.borrow().closes.is_empty()));
    assert!(matches!(
        startup.claim_config_reader(512),
        Err(WorkerStartupError::ConfigClaimed)
    ));
}
