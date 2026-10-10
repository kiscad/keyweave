#![allow(clippy::unwrap_used)]
use super::*;
use crate::native::{
    config_artifact::api_mock::MOCK as FILE, hook_thread, job_process::api::MOCK as JOB,
    progress_pipe::api_mock::MOCK as PIPE,
};
fn artifact() -> ConfigArtifact {
    ConfigArtifact::create(&std::env::temp_dir(), b"synthetic image").unwrap()
}
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join("keyweave worker.exe")
}
fn spawn() -> Supervisor {
    Supervisor::spawn_with_config(&path(), &[], 7, Policy::default(), MonoTime(0), artifact())
        .unwrap()
}
fn exited(owner: &mut Supervisor) {
    JOB.with(|m| {
        let mut m = m.borrow_mut();
        m.wait = 0;
        m.exit = 259;
    });
    assert!(matches!(
        owner.poll(MonoTime(1)).unwrap(),
        SupervisorStatus::Exited { code: 259, .. }
    ));
}
#[test]
fn callback_config_factory_retains_writer_and_cleanup_is_zero_change() {
    let config = artifact();
    let before = FILE.with(|m| m.borrow().calls.len());
    let error = hook_thread::with_callback_borrow(|| {
        Supervisor::spawn_with_config(&path(), &[], 7, Policy::default(), MonoTime(0), config)
            .unwrap_err()
    });
    let SupervisorSpawnError::Config {
        cause,
        mut resources,
    } = error
    else {
        panic!("expected envelope")
    };
    assert!(matches!(*cause, SupervisorSpawnError::Io(_)));
    assert_eq!(FILE.with(|m| m.borrow().calls.len()), before);
    PIPE.with(|m| assert!(m.borrow().calls.is_empty()));
    JOB.with(|m| assert!(m.borrow().calls.is_empty()));
    hook_thread::with_callback_borrow(|| {
        assert_eq!(resources.close().unwrap_err(), JobError::InCallback)
    });
    assert_eq!(FILE.with(|m| m.borrow().calls.len()), before);
    let report: crate::native::supervisor::SupervisorConfigCleanup = resources.close().unwrap();
    assert!(report.complete());
    assert!(report.config.unwrap().unwrap().complete());
}
#[test]
fn invalid_identity_or_full_suffix_preflight_does_not_prepare_pipes_or_reader() {
    for kind in 0..5 {
        std::thread::spawn(move || {
            let config = artifact();
            let before = FILE.with(|m| m.borrow().calls.len());
            let args = match kind {
                1 => vec!["x".into(); 124],
                2 => vec!["x".repeat(32768).into()],
                3 => vec!["bad\0arg".into()],
                _ => vec![],
            };
            let exe = if kind == 4 {
                std::path::PathBuf::from("relative.exe")
            } else {
                path()
            };
            let error = Supervisor::spawn_with_config(
                &exe,
                &args,
                if kind == 0 { 0 } else { 7 },
                Policy::default(),
                MonoTime(0),
                config,
            )
            .unwrap_err();
            let SupervisorSpawnError::Config {
                cause,
                mut resources,
            } = error
            else {
                panic!("envelope")
            };
            assert!(matches!(
                *cause,
                SupervisorSpawnError::Protocol(_)
                    | SupervisorSpawnError::Command(JobError::Command)
            ));
            assert_eq!(FILE.with(|m| m.borrow().calls.len()), before);
            PIPE.with(|m| assert!(m.borrow().calls.is_empty()));
            JOB.with(|m| assert!(m.borrow().calls.is_empty()));
            assert!(resources.close().unwrap().complete());
        })
        .join()
        .unwrap();
    }
}
#[test]
fn running_config_supervisor_owns_exact_startup_prefix_and_requires_actual_exit() {
    let mut owner = spawn();
    assert_eq!(owner.status(), SupervisorStatus::Running);
    assert!(matches!(owner.close_resources(), Err(JobError::State)));
    JOB.with(|m| {
        let m = m.borrow();
        assert_eq!(m.handles_count, 4);
        assert!(m.resumed);
        let command = String::from_utf16(&m.command).unwrap();
        assert!(command.contains(&format!(
            "\"--keyweave-worker\" \"{}\" \"7\"",
            std::process::id()
        )));
        assert!(command.ends_with("\"--keyweave-config-handle\" \"512\""));
    });
    FILE.with(|m| assert_eq!(m.borrow().closes, [512]));
    owner.request_stop().unwrap();
    assert!(matches!(owner.close_resources(), Err(JobError::State)));
    assert_eq!(owner.progress_epoch(), 0);
    exited(&mut owner);
    let report: crate::native::supervisor::SupervisorCleanup = owner.close_resources().unwrap();
    assert!(report.complete());
    assert!(report.config.unwrap().unwrap().complete());
    FILE.with(|m| assert_eq!(m.borrow().closes, [512, 256]));
    assert!(owner.close_resources().unwrap().complete());
}
#[test]
fn pipe_factory_failure_preserves_config_and_all_returned_first_pair_owners() {
    for pair in [1, 2] {
        std::thread::spawn(move || {
            let config = artifact();
            PIPE.with(|m| m.borrow_mut().fail_rng_pair = Some(pair));
            let error = Supervisor::spawn_with_config(
                &path(),
                &[],
                7,
                Policy::default(),
                MonoTime(0),
                config,
            )
            .unwrap_err();
            let SupervisorSpawnError::Config {
                cause,
                mut resources,
            } = error
            else {
                panic!("envelope")
            };
            assert!(matches!(*cause, SupervisorSpawnError::Io(_)));
            JOB.with(|m| assert!(m.borrow().calls.is_empty()));
            let report = resources.close().unwrap();
            assert!(report.complete());
            assert_eq!(report.worker_reader.is_some(), pair == 2);
            assert_eq!(report.writer.is_some(), pair == 2);
            assert!(report.reader.is_none() && report.worker_writer.is_none());
            assert!(report.config.unwrap().unwrap().complete());
        })
        .join()
        .unwrap();
    }
}
#[test]
fn job_failure_keeps_config_in_job_and_parent_pipes_in_separate_envelope() {
    let config = artifact();
    JOB.with(|m| m.borrow_mut().fail = Some("create"));
    let error =
        Supervisor::spawn_with_config(&path(), &[], 7, Policy::default(), MonoTime(0), config)
            .unwrap_err();
    let SupervisorSpawnError::Config {
        cause,
        mut resources,
    } = error
    else {
        panic!("envelope")
    };
    let SupervisorSpawnError::Job(mut error) = *cause else {
        panic!("Job cause")
    };
    assert_eq!(
        error.cause,
        JobError::Api {
            call: "CreateProcessW",
            code: 5
        }
    );
    assert!(error.channel_cleanup.as_ref().unwrap().complete());
    let report = resources.close().unwrap();
    assert!(report.complete());
    assert!(report.config.is_none());
    assert!(report.reader.is_some() && report.writer.is_some());
    let owner = error.owner.as_mut().unwrap();
    assert_eq!(
        owner.state(),
        crate::native::job_process::JobProcessState::Prepared
    );
    assert!(owner.close_config().unwrap().unwrap().complete());
}
#[test]
fn actual_exit_cleanup_attempts_all_file_and_pipe_results_and_keeps_history() {
    let mut owner = spawn();
    exited(&mut owner);
    PIPE.with(|m| m.borrow_mut().fail = Some("close"));
    FILE.with(|m| m.borrow_mut().close_fail = true);
    let before = PIPE.with(|m| m.borrow().calls.len());
    let report = owner.close_resources().unwrap();
    assert!(!report.complete());
    assert!(!report.channels.complete());
    assert!(!report.config.as_ref().unwrap().unwrap().complete());
    PIPE.with(|m| {
        assert_eq!(
            m.borrow().calls[before..]
                .iter()
                .filter(|&&c| c == "close")
                .count(),
            4
        )
    });
    PIPE.with(|m| m.borrow_mut().fail = None);
    FILE.with(|m| m.borrow_mut().close_fail = false);
    assert!(owner.close_resources().unwrap().complete());
    assert!(!report.complete());
}
#[test]
fn resume_failure_keeps_full_config_owner_and_stops_without_ipc_until_exit() {
    JOB.with(|m| {
        let mut m = m.borrow_mut();
        m.resume_count = Some(0);
        m.fail = Some("kill-job");
    });
    let error =
        Supervisor::spawn_with_config(&path(), &[], 7, Policy::default(), MonoTime(0), artifact())
            .unwrap_err();
    let SupervisorSpawnError::Resume {
        cause,
        stop,
        mut owner,
    } = error
    else {
        panic!("resume owner")
    };
    assert_eq!(cause, JobError::SuspendCount(0));
    assert!(stop.is_some());
    assert!(matches!(owner.close_resources(), Err(JobError::State)));
    FILE.with(|m| assert_eq!(m.borrow().closes, [512]));
    JOB.with(|m| m.borrow_mut().fail = None);
    owner.poll(MonoTime(0)).unwrap();
    PIPE.with(|m| {
        assert!(!m.borrow().calls.contains(&"read") && !m.borrow().calls.contains(&"write"))
    });
    exited(&mut owner);
    assert!(owner.close_resources().unwrap().complete());
}
#[test]
fn cleanup_callback_after_exit_does_not_touch_any_owned_resource() {
    let mut owner = spawn();
    exited(&mut owner);
    let counts = (
        FILE.with(|m| m.borrow().calls.len()),
        PIPE.with(|m| m.borrow().calls.len()),
        JOB.with(|m| m.borrow().calls.len()),
    );
    hook_thread::with_callback_borrow(|| {
        assert!(matches!(owner.close_resources(), Err(JobError::InCallback)))
    });
    assert_eq!(
        counts,
        (
            FILE.with(|m| m.borrow().calls.len()),
            PIPE.with(|m| m.borrow().calls.len()),
            JOB.with(|m| m.borrow().calls.len())
        )
    );
    assert!(owner.close_resources().unwrap().complete());
}
