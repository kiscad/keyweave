#![allow(clippy::unwrap_used)]
use super::*;
use std::mem::{align_of, offset_of};
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join("keyweave worker.exe")
}
fn spawn() -> OwnedJobProcess {
    OwnedJobProcess::spawn_suspended(&path(), &[]).unwrap()
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
    assert_eq!(size_of::<ProcessInfo>(), if wide { 24 } else { 16 });
}
#[test]
fn assignment_precedes_explicit_resume_and_stop_is_not_exit() {
    let mut child = spawn();
    assert_eq!(child.state(), JobProcessState::Suspended);
    assert_eq!(
        api::MOCK.with(|m| m.borrow().calls.clone()),
        ["job", "limits", "create", "assign"]
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
fn assignment_failure_preserves_owner_and_uses_only_own_process_for_rollback() {
    api::MOCK.with(|m| m.borrow_mut().fail = Some("assign"));
    let mut error = OwnedJobProcess::spawn_suspended(&path(), &[]).unwrap_err();
    assert!(matches!(
        error.cause,
        JobError::Api {
            call: "AssignProcessToJobObject",
            code: 5
        }
    ));
    let child = error.owner.as_mut().unwrap();
    assert_eq!(child.state(), JobProcessState::StopRequested);
    assert_eq!(child.resume(), Err(JobError::State));
    assert_eq!(child.poll_exit(Duration::ZERO).unwrap(), None);
    assert!(api::MOCK.with(|m| m.borrow().calls.contains(&"kill-process")));
    assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"resume")));
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
    for fail in ["job", "limits", "create"] {
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
