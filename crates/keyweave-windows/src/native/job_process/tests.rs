#![allow(clippy::unwrap_used)]
use super::*;
use std::mem::{align_of, offset_of};
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join("keyweave worker.exe")
}
fn spawn() -> OwnedJobProcess {
    OwnedJobProcess::spawn_suspended(&path(), &[]).unwrap()
}
fn channels() -> (PipeReader, PipeWriter, PipeReader, PipeWriter) {
    let (worker_reader, parent_writer) = PipeReader::pair().unwrap();
    let (parent_reader, worker_writer) = PipeReader::pair().unwrap();
    (worker_reader, worker_writer, parent_reader, parent_writer)
}
#[test]
fn exact_three_handles_are_inherited_and_closed_before_resume() {
    let (reader, writer, _parent_reader, _parent_writer) = channels();
    let mut child =
        OwnedJobProcess::spawn_suspended_with_progress(&path(), &[], reader, writer).unwrap();
    assert_eq!(child.state(), JobProcessState::Suspended);
    assert!(child.channels.is_none());
    assert!(child.finish_channel_transfer().unwrap().complete());
    api::MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.attribute_count, 2);
        assert!(m.calls.contains(&"attribute-handles"));
        let deleted = m
            .calls
            .iter()
            .position(|&s| s == "attribute-delete")
            .unwrap();
        let cleared = m.calls.iter().position(|&s| s == "clear-inherit").unwrap();
        assert!(deleted < cleared);
        assert!(!m.calls.contains(&"resume"));
        assert_eq!(m.inheritance.len(), 3);
        for handle in [100, 210, 400] {
            assert_eq!(m.inheritance.get(&handle), Some(&false));
        }
        assert_eq!(m.closes, [400]);
    });
    child.resume().unwrap();
    assert_eq!(child.state(), JobProcessState::Running);
}
#[test]
fn handle_list_or_creation_failure_closes_channels_without_running_child() {
    for fail in [
        "nul",
        "inherit",
        "attribute-size",
        "attribute-init",
        "attribute-job",
        "attribute-handles",
        "create",
    ] {
        std::thread::spawn(move || {
            let (reader, writer, _parent_reader, _parent_writer) = channels();
            api::MOCK.with(|m| m.borrow_mut().fail = Some(fail));
            let error =
                OwnedJobProcess::spawn_suspended_with_progress(&path(), &[], reader, writer)
                    .unwrap_err();
            assert!(matches!(error.cause, JobError::Api { code: 5, .. }));
            assert!(error.channel_cleanup.unwrap().complete());
            let owner = error.owner.as_ref().unwrap();
            assert_eq!(owner.state(), JobProcessState::Prepared);
            assert!(owner.channels.is_none());
            assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"resume")));
            assert_eq!(api::MOCK.with(|m| m.borrow().initialized_list), 0);
        })
        .join()
        .unwrap();
    }
}
#[test]
fn partial_inheritance_failure_is_cleared_for_all_owned_handles() {
    for handle in [100, 400, 210] {
        std::thread::spawn(move || {
            let (reader, writer, _parent_reader, _parent_writer) = channels();
            api::MOCK.with(|m| m.borrow_mut().fail_inherit = Some((handle, true)));
            let error =
                OwnedJobProcess::spawn_suspended_with_progress(&path(), &[], reader, writer)
                    .unwrap_err();
            assert_eq!(
                error.cause,
                JobError::Api {
                    call: "SetHandleInformation(inherit)",
                    code: 5
                }
            );
            assert!(error.channel_cleanup.unwrap().complete());
            api::MOCK.with(|m| {
                let m = m.borrow();
                assert_eq!(m.inheritance.len(), 3);
                assert!(m.inheritance.values().all(|value| !value));
                assert!(!m.calls.contains(&"create"));
            });
        })
        .join()
        .unwrap();
    }
}
#[test]
fn channel_cleanup_failure_retains_report_and_stops_suspended_child() {
    for fail_clear in [true, false] {
        std::thread::spawn(move || {
            let (reader, writer, _parent_reader, _parent_writer) = channels();
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                if fail_clear {
                    m.fail_inherit = Some((100, false));
                } else {
                    m.fail_close = Some(400);
                }
            });
            let mut error =
                OwnedJobProcess::spawn_suspended_with_progress(&path(), &[], reader, writer)
                    .unwrap_err();
            assert_eq!(error.cause, JobError::ChannelCleanup);
            let report = error.channel_cleanup.as_ref().unwrap();
            assert!(!report.complete());
            assert_eq!(
                report.inheritance_errors[0],
                if fail_clear { Some(5) } else { None }
            );
            assert_eq!(
                report.stdout_close_error,
                if fail_clear { None } else { Some(6) }
            );
            assert!(report.reader.complete() && report.writer.complete());
            let owner = error.owner.as_mut().unwrap();
            assert_eq!(owner.state(), JobProcessState::StopRequested);
            assert!(owner.channels.is_some());
            assert_eq!(owner.resume(), Err(JobError::State));
            assert_eq!(owner.poll_exit(Duration::ZERO).unwrap(), None);
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                m.fail_inherit = None;
                m.fail_close = None;
            });
            assert!(owner.finish_channel_transfer().unwrap().complete());
            assert!(owner.channels.is_none());
            assert_eq!(owner.resume(), Err(JobError::State));
            // Original failure remains independently available after retry.
            assert!(!error.channel_cleanup.unwrap().complete());
        })
        .join()
        .unwrap();
    }
}
#[test]
fn failed_startup_and_stop_can_never_resume_after_cleanup_retry() {
    let (reader, writer, _parent_reader, _parent_writer) = channels();
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.fail_close = Some(400);
        m.fail = Some("kill-job");
    });
    let mut error =
        OwnedJobProcess::spawn_suspended_with_progress(&path(), &[], reader, writer).unwrap_err();
    assert_eq!(error.cause, JobError::ChannelCleanup);
    assert_eq!(
        error.rollback,
        Some(JobError::Api {
            call: "TerminateJobObject",
            code: 5
        })
    );
    let owner = error.owner.as_mut().unwrap();
    assert_eq!(owner.state(), JobProcessState::StartupFailed);
    api::MOCK.with(|m| m.borrow_mut().fail_close = None);
    assert!(owner.finish_channel_transfer().unwrap().complete());
    assert_eq!(owner.resume(), Err(JobError::State));
    assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"resume")));
    api::MOCK.with(|m| m.borrow_mut().fail = None);
    owner.request_stop().unwrap();
    assert_eq!(owner.poll_exit(Duration::ZERO).unwrap(), None);
}
#[test]
fn busy_closed_and_same_pair_channels_are_rejected_before_process_creation() {
    for kind in [0, 1, 2, 3] {
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut reader, mut writer, _parent_reader, _parent_writer) = channels();
            if kind == 0 {
                reader.read(&mut [0u8; 24]).unwrap_err();
            }
            if kind == 1 {
                writer.write(&[1; 24]).unwrap_err();
            }
            if kind == 2 {
                reader.close().unwrap();
            }
            if kind == 3 {
                let pair = PipeReader::pair().unwrap();
                reader = pair.0;
                writer = pair.1;
            }
            let error =
                OwnedJobProcess::spawn_suspended_with_progress(&path(), &[], reader, writer)
                    .unwrap_err();
            assert_eq!(error.cause, JobError::PipeState);
            assert!(error.channel_cleanup.unwrap().complete());
            assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"create")));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn callback_channel_spawn_keeps_owner_without_native_calls_until_outside() {
    let (reader, writer, _parent_reader, _parent_writer) = channels();
    let mut error = super::super::hook_thread::with_callback_borrow(|| {
        OwnedJobProcess::spawn_suspended_with_progress(&path(), &[], reader, writer).unwrap_err()
    });
    assert_eq!(error.cause, JobError::InCallback);
    assert!(api::MOCK.with(|m| m.borrow().calls.is_empty()));
    let owner = error.owner.as_mut().unwrap();
    assert_eq!(owner.state(), JobProcessState::Prepared);
    assert!(owner.finish_channel_transfer().unwrap().complete());
}
#[test]
fn sdk_job_startup_process_layouts() {
    let wide = size_of::<usize>() == 8;
    assert_eq!(size_of::<BasicLimits>(), if wide { 64 } else { 48 });
    assert_eq!(align_of::<BasicLimits>(), 8);
    assert_eq!(size_of::<ExtendedLimits>(), if wide { 144 } else { 112 });
    assert_eq!(align_of::<ExtendedLimits>(), 8);
    assert_eq!(offset_of!(BasicLimits, flags), 16);
    assert_eq!(offset_of!(BasicLimits, active), if wide { 40 } else { 28 });
    assert_eq!(offset_of!(ExtendedLimits, io), if wide { 64 } else { 48 });
    assert_eq!(
        offset_of!(ExtendedLimits, memory),
        if wide { 112 } else { 96 }
    );
    assert_eq!(size_of::<Startup>(), if wide { 104 } else { 68 });
    assert_eq!(offset_of!(Startup, numbers), if wide { 32 } else { 16 });
    assert_eq!(offset_of!(Startup, stdin), if wide { 80 } else { 56 });
    assert_eq!(size_of::<StartupEx>(), if wide { 112 } else { 72 });
    assert_eq!(
        offset_of!(StartupEx, attributes),
        if wide { 104 } else { 68 }
    );
    assert_eq!(size_of::<ProcessInfo>(), if wide { 24 } else { 16 });
}
#[test]
fn assignment_precedes_explicit_resume_and_stop_is_not_exit() {
    let mut child = spawn();
    assert_eq!(child.state(), JobProcessState::Suspended);
    assert_eq!(
        api::MOCK.with(|m| m.borrow().calls.clone()),
        [
            "job",
            "limits",
            "attribute-size",
            "attribute-init",
            "attribute-job",
            "create",
            "attribute-delete"
        ]
    );
    assert_eq!(api::MOCK.with(|m| m.borrow().limits), Some((0x2008, 1)));
    child.resume().unwrap();
    assert_eq!(child.state(), JobProcessState::Running);
    assert_eq!(child.resume(), Err(JobError::State));
    child.request_stop().unwrap();
    assert_eq!(child.state(), JobProcessState::StopRequested);
    assert_eq!(child.poll_exit(Duration::MAX).unwrap(), None);
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.wait = 0;
        m.exit = 259;
    });
    assert_eq!(child.poll_exit(Duration::ZERO).unwrap(), Some(259));
    assert_eq!(child.state(), JobProcessState::Exited(259));
    let calls = api::MOCK.with(|m| m.borrow().calls.len());
    assert_eq!(child.poll_exit(Duration::ZERO).unwrap(), Some(259));
    assert_eq!(api::MOCK.with(|m| m.borrow().calls.len()), calls);
    drop(child);
    assert_eq!(api::MOCK.with(|m| m.borrow().closes.clone()), [3, 1, 2]);
    assert!(api::MOCK.with(|m| m.borrow().waits.iter().all(|&v| v <= 20)));
}
#[test]
fn job_attribute_failure_aborts_before_creating_a_child_and_cleans_list() {
    api::MOCK.with(|m| m.borrow_mut().fail = Some("attribute-job"));
    let error = OwnedJobProcess::spawn_suspended(&path(), &[]).unwrap_err();
    assert_eq!(
        error.cause,
        JobError::Api {
            call: "UpdateProcThreadAttribute(JOB_LIST)",
            code: 5
        }
    );
    assert_eq!(
        error.owner.as_ref().unwrap().state(),
        JobProcessState::Prepared
    );
    assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"create")));
    assert_eq!(api::MOCK.with(|m| m.borrow().initialized_list), 0);
    assert_eq!(
        api::MOCK.with(|m| m
            .borrow()
            .calls
            .iter()
            .filter(|&&s| s == "attribute-delete")
            .count()),
        1
    );
}
#[test]
fn failed_resume_and_unexpected_suspend_count_stop_without_retrying_resume() {
    for value in [0, 2, u32::MAX] {
        std::thread::spawn(move || {
            let mut child = spawn();
            api::MOCK.with(|m| m.borrow_mut().resume_count = Some(value));
            assert!(child.resume().is_err());
            assert_eq!(child.state(), JobProcessState::StopRequested);
            assert_eq!(
                api::MOCK.with(|m| m.borrow().calls.iter().filter(|&&s| s == "resume").count()),
                1
            );
        })
        .join()
        .unwrap();
    }
}
#[test]
fn primary_errors_and_partial_resource_ownership_are_kept() {
    for fail in [
        "job",
        "limits",
        "attribute-size",
        "attribute-init",
        "create",
    ] {
        std::thread::spawn(move || {
            api::MOCK.with(|m| m.borrow_mut().fail = Some(fail));
            let error = OwnedJobProcess::spawn_suspended(&path(), &[]).unwrap_err();
            assert!(matches!(error.cause, JobError::Api { code: 5, .. }));
            assert_eq!(error.owner.is_some(), fail != "job");
            if let Some(owner) = error.owner.as_ref() {
                assert_eq!(owner.state(), JobProcessState::Prepared);
            }
            drop(error);
            assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"resume")));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn termination_or_wait_failure_keeps_live_owner_and_is_not_exit_proof() {
    let mut child = spawn();
    child.resume().unwrap();
    api::MOCK.with(|m| m.borrow_mut().fail = Some("kill-job"));
    assert!(child.request_stop().is_err());
    assert_eq!(child.state(), JobProcessState::Running);
    api::MOCK.with(|m| m.borrow_mut().wait = 128);
    assert_eq!(child.poll_exit(Duration::ZERO), Err(JobError::Wait(128)));
    api::MOCK.with(|m| m.borrow_mut().fail = Some("wait"));
    assert!(matches!(
        child.poll_exit(Duration::ZERO),
        Err(JobError::Api {
            call: "WaitForSingleObject",
            ..
        })
    ));
    assert_eq!(child.state(), JobProcessState::Running);
}
#[test]
fn quoting_is_bounded_and_preserves_empty_space_quote_and_backslash_arguments() {
    let args = vec![
        Vec::new(),
        "a b".encode_utf16().collect(),
        "a\"b".encode_utf16().collect(),
        "c\\".encode_utf16().collect(),
    ];
    assert_eq!(
        String::from_utf16(&quote(&args).unwrap()).unwrap(),
        "\"\" \"a b\" \"a\\\"b\" \"c\\\\\"\0"
    );
    assert_eq!(quote(&[vec![0]]), Err(JobError::Command));
    assert_eq!(quote(&[vec![92; 32766]]), Err(JobError::Command));
    assert!(command(Path::new("relative.exe"), &[]).is_err());
    assert!(command(&std::env::temp_dir().join("script.cmd"), &[]).is_err());
    assert!(command(&path(), &vec![OsString::new(); 129]).is_err());
}
#[test]
fn callback_rejects_spawn_before_any_native_call() {
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(
            OwnedJobProcess::spawn_suspended(&path(), &[])
                .unwrap_err()
                .cause,
            JobError::InCallback
        );
    });
    assert!(api::MOCK.with(|m| m.borrow().calls.is_empty()));
}

#[test]
fn failed_rollback_cannot_allow_a_second_resume() {
    let mut child = spawn();
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.resume_count = Some(2);
        m.fail = Some("kill-job");
    });
    assert_eq!(child.resume(), Err(JobError::SuspendCount(2)));
    assert_eq!(child.state(), JobProcessState::ResumeAttempted);
    assert_eq!(child.resume(), Err(JobError::State));
    assert_eq!(
        api::MOCK.with(|m| m.borrow().calls.iter().filter(|&&s| s == "resume").count()),
        1
    );
    api::MOCK.with(|m| m.borrow_mut().fail = None);
    child.request_stop().unwrap();
    assert_eq!(child.state(), JobProcessState::StopRequested);
}

#[test]
fn attribute_size_query_rejects_zero_excess_and_unexpected_success() {
    for (size, success) in [(0, false), (4097, false), (96, true)] {
        std::thread::spawn(move || {
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                m.query_bytes = Some(size);
                m.query_success = success;
            });
            let error = OwnedJobProcess::spawn_suspended(&path(), &[]).unwrap_err();
            assert_eq!(error.cause, JobError::AttributeSize);
            assert_eq!(
                error.owner.as_ref().unwrap().state(),
                JobProcessState::Prepared
            );
            assert!(!api::MOCK.with(|m| {
                m.borrow()
                    .calls
                    .iter()
                    .any(|s| ["attribute-init", "attribute-delete", "create"].contains(s))
            }));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn create_failure_deletes_initialized_attributes_without_claiming_child_exit() {
    api::MOCK.with(|m| m.borrow_mut().fail = Some("create"));
    let error = OwnedJobProcess::spawn_suspended(&path(), &[]).unwrap_err();
    assert_eq!(
        error.owner.as_ref().unwrap().state(),
        JobProcessState::Prepared
    );
    assert_eq!(error.rollback, None);
    let calls = api::MOCK.with(|m| m.borrow().calls.clone());
    assert!(calls.ends_with(&["create", "attribute-delete"]));
    assert!(!calls.contains(&"kill-job"));
    assert!(!calls.contains(&"resume"));
    assert_eq!(api::MOCK.with(|m| m.borrow().initialized_list), 0);
}

#[test]
fn supervisor_resume_failure_retains_owner_and_primary_until_exit_observation() {
    use super::super::supervisor::{
        Supervisor, SupervisorSpawnError, SupervisorStatus, TerminationReason,
    };
    use keyweave_progress::Policy;
    use keyweave_types::MonoTime;
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.resume_count = Some(0);
        m.fail = Some("kill-job");
    });
    let error = Supervisor::spawn(&path(), &[], 7, Policy::default(), MonoTime(0)).unwrap_err();
    let SupervisorSpawnError::Resume {
        cause,
        stop,
        mut owner,
    } = error
    else {
        panic!("expected retained resume failure")
    };
    assert_eq!(cause, JobError::SuspendCount(0));
    assert!(stop.is_some());
    assert_eq!(
        owner.status(),
        SupervisorStatus::Stopping {
            reason: TerminationReason::Process(cause),
            stop_sent: false
        }
    );
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.fail = None;
        m.wait = 258;
    });
    owner.poll(MonoTime(0)).unwrap();
    assert_eq!(
        owner.status(),
        SupervisorStatus::Stopping {
            reason: TerminationReason::Process(cause),
            stop_sent: true
        }
    );
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.wait = 0;
        m.exit = 259;
    });
    assert_eq!(
        owner.poll(MonoTime(0)).unwrap(),
        SupervisorStatus::Exited {
            code: 259,
            reason: Some(TerminationReason::Process(cause))
        }
    );
    assert!(owner.close_channels().unwrap().complete());
}

#[test]
fn supervisor_preflight_rejects_invalid_policy_and_oversize_command_before_child_creation() {
    use super::super::supervisor::{Supervisor, SupervisorSpawnError};
    use keyweave_progress::{Policy, ProgressError};
    use keyweave_types::MonoTime;
    assert!(matches!(
        Supervisor::spawn(&path(), &[], 0, Policy::default(), MonoTime(0)),
        Err(SupervisorSpawnError::Protocol(ProgressError::Policy))
    ));
    for args in [
        vec!["x".into(); 126],
        vec!["x".repeat(32768).into()],
        vec!["bad\0arg".into()],
    ] {
        assert!(matches!(
            Supervisor::spawn(&path(), &args, 7, Policy::default(), MonoTime(0)),
            Err(SupervisorSpawnError::Command(JobError::Command))
        ));
    }
    assert!(matches!(
        Supervisor::spawn(
            Path::new("relative.exe"),
            &[],
            7,
            Policy::default(),
            MonoTime(0)
        ),
        Err(SupervisorSpawnError::Command(JobError::Command))
    ));
    api::MOCK.with(|m| assert!(m.borrow().calls.is_empty()));
}

#[test]
fn supervisor_cooperative_api_rejects_callback_without_changing_owned_state() {
    use super::super::supervisor::{Supervisor, SupervisorStatus, TerminationReason};
    use keyweave_progress::Policy;
    use keyweave_types::MonoTime;
    let mut owner = Supervisor::spawn(&path(), &[], 7, Policy::default(), MonoTime(0)).unwrap();
    let status = owner.request_shutdown(MonoTime(1)).unwrap();
    assert_eq!(owner.deadline(), MonoTime(2_000_000_001));
    super::super::hook_thread::with_callback_borrow(|| {
        let err = owner.request_shutdown(MonoTime(u64::MAX)).unwrap_err();
        assert_eq!(err.cause, JobError::InCallback);
    });
    assert_eq!(owner.status(), status);
    assert_eq!(owner.deadline(), MonoTime(2_000_000_001));
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.wait = 0;
        m.exit = 0;
    });
    assert_eq!(
        owner.poll(MonoTime(5)).unwrap(),
        SupervisorStatus::Exited {
            code: 0,
            reason: Some(TerminationReason::Cooperative)
        }
    );
    assert!(owner.close_channels().unwrap().complete());
}
