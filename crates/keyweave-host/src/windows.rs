//! All native owners remain on this independent control thread. No active-loop
//! config reads/compilation, synchronous stdio or user-input waits.
use keyweave_progress::Policy;
use keyweave_types::MonoTime;
use keyweave_windows::keys::IdentityPolicy;
use keyweave_windows::{
    JobProcessState, OwnedJobProcess, RunIdentity, Supervisor, SupervisorSpawnError,
    SupervisorStatus, TerminationReason,
};
use keyweave_worker::preparation::{PreparedConfig, worker_arguments_with_mode};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
const OBSERVE_LIMIT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_DIAGNOSTICS: usize = 16;
#[derive(Debug)]
pub struct Report {
    pub startup_cancelled: bool,
    pub worker_exit: Option<u32>,
    pub reason: Option<TerminationReason>,
    pub resources_complete: bool,
    pub progress_epoch: u64,
    pub diagnostics: Vec<String>,
}
impl Report {
    /// Only actual observed exit codes have a classification; no inferred exit.
    pub fn worker_diagnostic(&self) -> Option<keyweave_worker::ExitDiagnostic> {
        self.worker_exit
            .map(keyweave_worker::ExitDiagnostic::observed)
    }

    pub fn successful(&self) -> bool {
        (if self.startup_cancelled {
            self.worker_exit.is_none() && self.reason.is_none() && self.progress_epoch == 0
        } else {
            self.worker_exit == Some(0)
                && matches!(self.reason, None | Some(TerminationReason::Cooperative))
        }) && self.resources_complete
            && self.diagnostics.is_empty()
    }
}
impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "worker_exit={:?}, reason={:?}, resources_complete={}, progress_epoch={}, diagnostics={:?}, startup_cancelled={}",
            self.worker_exit,
            self.reason,
            self.resources_complete,
            self.progress_epoch,
            self.diagnostics,
            self.startup_cancelled
        )?;
        if let Some(diagnostic) = self.worker_diagnostic() {
            write!(
                f,
                ", worker_classification={}, message={}, remediation={}",
                diagnostic.code, diagnostic.message, diagnostic.remediation
            )?;
            if let Some(evidence) = diagnostic.terminal {
                write!(f, ", terminal_issues={evidence:?}")?;
            }
        }
        Ok(())
    }
}
fn record(errors: &mut Vec<String>, error: impl std::fmt::Display) {
    let value = error.to_string();
    if errors.contains(&value) {
        return;
    }
    if errors.len() < MAX_DIAGNOSTICS - 1 {
        errors.push(value);
    } else if errors.len() == MAX_DIAGNOSTICS - 1 {
        errors.push("additional diagnostics omitted".into());
    }
}
fn cleanup_job(owner: &mut OwnedJobProcess, errors: &mut Vec<String>) -> bool {
    let start = Instant::now();
    while !matches!(
        owner.state(),
        JobProcessState::Prepared | JobProcessState::Exited(_)
    ) {
        if let Err(e) = owner.request_stop() {
            record(errors, e)
        }
        if let Err(e) = owner.poll_exit(Duration::ZERO) {
            record(errors, e)
        }
        if matches!(owner.state(), JobProcessState::Exited(_)) {
            break;
        }
        if start.elapsed() >= OBSERVE_LIMIT {
            record(errors, "startup child exit observation timed out");
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    let mut complete = true;
    match owner.finish_channel_transfer() {
        Ok(r) if r.complete() => (),
        other => {
            complete = false;
            record(errors, format!("startup child channels: {other:?}"));
        }
    }
    if let Err(e) = owner.close_config_preparation_reader() {
        complete = false;
        record(errors, e)
    }
    match owner.close_config() {
        Ok(r) if r.is_none_or(|r| r.complete()) => (),
        other => {
            complete = false;
            record(errors, format!("startup config: {other:?}"));
        }
    }
    complete
}
fn cleanup_spawn(error: &mut SupervisorSpawnError, errors: &mut Vec<String>) -> bool {
    match error {
        SupervisorSpawnError::Config { cause, resources } => {
            let child = cleanup_spawn(cause, errors);
            let local = match resources.close() {
                Ok(r) if r.complete() => true,
                other => {
                    record(errors, format!("preparation resources: {other:?}"));
                    false
                }
            };
            child && local
        }
        SupervisorSpawnError::Job(e) => e
            .owner
            .as_mut()
            .is_none_or(|owner| cleanup_job(owner, errors)),
        SupervisorSpawnError::Resume { owner, .. } => {
            let report = drive(owner, &AtomicBool::new(true), Instant::now(), None);
            let complete = report.worker_exit.is_some() && report.resources_complete;
            for e in report.diagnostics {
                record(errors, e)
            }
            complete
        }
        _ => true,
    }
}
fn should_stop(stop: &AtomicBool, console: Option<&keyweave_windows::ConsoleControl>) -> bool {
    stop.load(Ordering::Acquire) || console.is_some_and(keyweave_windows::ConsoleControl::requested)
}
fn drive(
    owner: &mut Supervisor,
    stop: &AtomicBool,
    start: Instant,
    console: Option<&keyweave_windows::ConsoleControl>,
) -> Report {
    let mut diagnostics = Vec::new();
    let mut requested = false;
    let mut stopping = None;
    let mut last = MonoTime(0);
    loop {
        let now = match u64::try_from(start.elapsed().as_nanos()) {
            Ok(ns) => MonoTime(ns),
            Err(_) => {
                record(&mut diagnostics, "host monotonic clock range exhausted");
                let _ = owner
                    .request_stop()
                    .map_err(|e| record(&mut diagnostics, e));
                last
            }
        };
        last = now;
        if should_stop(stop, console) && !requested {
            requested = true;
            if let Err(e) = owner.request_shutdown(now) {
                record(&mut diagnostics, e)
            }
        }
        if let Err(e) = owner.poll(now) {
            record(&mut diagnostics, e);
            stopping.get_or_insert_with(Instant::now);
        }
        match owner.status() {
            SupervisorStatus::Exited { code, reason } => {
                let resources_complete = match owner.close_resources() {
                    Ok(r) if r.complete() => true,
                    other => {
                        record(&mut diagnostics, format!("exited resources: {other:?}"));
                        false
                    }
                };
                return Report {
                    startup_cancelled: false,
                    worker_exit: Some(code),
                    reason,
                    resources_complete,
                    progress_epoch: owner.progress_epoch(),
                    diagnostics,
                };
            }
            SupervisorStatus::Stopping { .. } => {
                stopping.get_or_insert_with(Instant::now);
            }
            _ => (),
        }
        if stopping.is_some_and(|t: Instant| t.elapsed() >= OBSERVE_LIMIT) {
            record(
                &mut diagnostics,
                "worker actual exit was not observed within forced-stop limit",
            );
            return Report {
                startup_cancelled: false,
                worker_exit: None,
                reason: None,
                resources_complete: false,
                progress_epoch: owner.progress_epoch(),
                diagnostics,
            };
        }
        let wait =
            Duration::from_nanos(owner.deadline().0.saturating_sub(now.0)).min(POLL_INTERVAL);
        std::thread::sleep(wait.max(Duration::from_millis(1)));
    }
}
fn cancelled(artifact: Option<&mut keyweave_windows::ConfigArtifact>) -> Report {
    let mut diagnostics = Vec::new();
    let resources_complete = match artifact.map(|owner| owner.close()) {
        None => true,
        Some(Ok(report)) if report.complete() => true,
        other => {
            record(
                &mut diagnostics,
                format!("cancelled preparation resources: {other:?}"),
            );
            false
        }
    };
    Report {
        startup_cancelled: true,
        worker_exit: None,
        reason: None,
        resources_complete,
        progress_epoch: 0,
        diagnostics,
    }
}
/// Cold setup followed by independently driven owned supervision. Caller must
/// trust the selected executable; canonical path resolution is not authentication.
/// Each invocation mints a new identity. This function may install input only
/// through the actual selected worker, so invoke only for an explicit run command.
pub fn run(
    executable: &Path,
    diagnostic_path: &Path,
    directory: &Path,
    prepared: PreparedConfig,
    identity: IdentityPolicy,
    stop: &AtomicBool,
) -> Result<Report, String> {
    run_impl(
        executable,
        diagnostic_path,
        directory,
        prepared,
        identity,
        stop,
        None,
    )
}
/// Combine user input with an existing process-lifetime console control owner.
pub fn run_with_console(
    executable: &Path,
    diagnostic_path: &Path,
    directory: &Path,
    prepared: PreparedConfig,
    identity: IdentityPolicy,
    stop: &AtomicBool,
    console: &keyweave_windows::ConsoleControl,
) -> Result<Report, String> {
    run_impl(
        executable,
        diagnostic_path,
        directory,
        prepared,
        identity,
        stop,
        Some(console),
    )
}
fn run_impl(
    executable: &Path,
    diagnostic_path: &Path,
    directory: &Path,
    prepared: PreparedConfig,
    identity: IdentityPolicy,
    stop: &AtomicBool,
    console: Option<&keyweave_windows::ConsoleControl>,
) -> Result<Report, String> {
    keyweave_windows::startup::ensure_preparation_context().map_err(|e| e.to_string())?;
    if should_stop(stop, console) {
        return Ok(cancelled(None));
    }
    let fresh = RunIdentity::generate().map_err(|e| e.to_string())?;
    let args = worker_arguments_with_mode(
        diagnostic_path,
        fresh.tag(),
        identity,
        prepared.execution_mode(),
    )
    .map_err(|e| e.to_string())?;
    let mut artifact=prepared.seal(directory).map_err(|mut e|{
        let mut errors=Vec::new();let mut complete=true;
        if let Some(owner)=e.owner.as_mut(){match owner.close(){Ok(r) if r.complete()=>(),other=>{complete=false;record(&mut errors,format!("artifact cleanup: {other:?}"));}}}
        format!("artifact preparation failed: {e}; resources_complete={complete}; diagnostics={errors:?}")
    })?;
    if should_stop(stop, console) {
        return Ok(cancelled(Some(&mut artifact)));
    }
    // Start the clock after cold IO. The same Instant epoch drives every poll.
    let start = Instant::now();
    let mut owner = Supervisor::spawn_with_config(
        executable,
        &args,
        fresh.run_id(),
        Policy::default(),
        MonoTime(0),
        artifact,
    )
    .map_err(|mut e| {
        let mut errors = Vec::new();
        let complete = cleanup_spawn(&mut e, &mut errors);
        format!("worker startup failed: {e}; resources_complete={complete}; diagnostics={errors:?}")
    })?;
    Ok(drive(&mut owner, stop, start, console))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn success_requires_actual_clean_natural_exit_without_hidden_faults() {
        let mut report = Report {
            startup_cancelled: false,
            worker_exit: Some(0),
            reason: Some(TerminationReason::Cooperative),
            resources_complete: true,
            progress_epoch: 0,
            diagnostics: vec![],
        };
        assert!(report.successful());
        report.worker_exit = None;
        assert!(!report.successful());
        report.worker_exit = Some(0);
        report.resources_complete = false;
        assert!(!report.successful());
        report.resources_complete = true;
        report.reason = Some(TerminationReason::Requested);
        assert!(!report.successful());
        report.reason = None;
        report.worker_exit = Some(259);
        assert!(!report.successful());
        report.worker_exit = Some(0);
        report.diagnostics.push("close failed".into());
        assert!(!report.successful());
    }
    #[test]
    fn diagnostics_require_actual_exit_and_preserve_independent_failures() {
        let mut report = Report {
            startup_cancelled: false,
            worker_exit: None,
            reason: Some(TerminationReason::Requested),
            resources_complete: false,
            progress_epoch: 0,
            diagnostics: vec!["independent cleanup failed".into()],
        };
        assert_eq!(report.worker_diagnostic(), None);
        assert!(!report.to_string().contains("worker_classification="));
        report.worker_exit = Some(66);
        assert_eq!(
            report.worker_diagnostic().map(|d| d.code),
            Some("KW-WORKER-066")
        );
        let rendered = report.to_string();
        assert!(rendered.contains("check-windows"));
        assert!(rendered.contains("Requested") && rendered.contains("independent cleanup failed"));
        assert!(rendered.contains("resources_complete=false"));
        assert!(!report.successful());
        // A clean scalar must not override a forced reason/cleanup failure.
        report.worker_exit = Some(0);
        assert_eq!(
            report.worker_diagnostic().map(|d| d.code),
            Some("KW-WORKER-000")
        );
        assert!(!report.successful());
    }
    #[test]
    fn unknown_native_exit_is_reported_in_full_without_false_success() {
        let mut report = Report {
            startup_cancelled: false,
            worker_exit: Some(3763033857),
            reason: None,
            resources_complete: true,
            progress_epoch: 7,
            diagnostics: vec![],
        };
        for raw in [3763033857, 259, u32::MAX] {
            report.worker_exit = Some(raw);
            assert_eq!(report.worker_diagnostic().map(|d| d.raw_code), Some(raw));
            assert!(report.to_string().contains("KW-WORKER-UNKNOWN"));
            assert!(report.to_string().contains(&raw.to_string()));
            assert!(!report.successful());
        }
    }
    #[test]
    fn terminal_evidence_keeps_all_issues_without_overriding_supervisor_failure() {
        let report = Report {
            startup_cancelled: false,
            worker_exit: Some(0x4b57_03ff),
            reason: Some(TerminationReason::Requested),
            resources_complete: false,
            progress_epoch: 3,
            diagnostics: vec!["parent cleanup failed".into()],
        };
        let text = report.to_string();
        assert!(text.contains("KW-WORKER-069"));
        assert!(text.contains("[Fault, ProgressFailed, ShutdownMissing, StoppedUnconfirmed, UnhookUnconfirmed, RecoveryUnconfirmed, NotificationsUnconfirmed, AccessLost, SessionChanged, UntrackedInput]"));
        assert!(text.contains("Requested") && text.contains("parent cleanup failed"));
        assert!(!report.successful());
    }
    #[test]
    fn repeated_errors_deduplicate_and_overflow_is_visible_and_bounded() {
        let mut errors = Vec::new();
        for _ in 0..100 {
            record(&mut errors, "primary");
        }
        assert_eq!(errors, vec!["primary"]);
        for n in 0..100 {
            record(&mut errors, format!("secondary {n}"));
        }
        assert_eq!(errors.len(), MAX_DIAGNOSTICS);
        assert_eq!(errors[0], "primary");
        assert_eq!(
            errors.last().map(String::as_str),
            Some("additional diagnostics omitted")
        );
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn known_stop_prevents_rng_file_job_and_worker_even_with_unusable_paths() {
        let prepared = PreparedConfig::from_bytes(br#"{"profiles":[{"selected":true}]}"#.to_vec());
        let prepared = match prepared {
            Ok(v) => v,
            Err(e) => panic!("synthetic preparation failed: {e}"),
        };
        let report = match run(
            Path::new("relative-nonexistent.exe"),
            Path::new("relative-config"),
            Path::new("relative-directory"),
            prepared,
            IdentityPolicy::StrictUnique,
            &AtomicBool::new(true),
        ) {
            Ok(v) => v,
            Err(e) => panic!("cancel failed: {e}"),
        };
        assert!(report.startup_cancelled && report.successful());
        assert_eq!(report.worker_exit, None);
        let mut invalid = report;
        invalid.worker_exit = Some(0);
        assert!(!invalid.successful());
    }
    #[test]
    fn cancelled_sealed_preparation_explicitly_closes_and_deletes_without_child() {
        let mut artifact = match keyweave_windows::ConfigArtifact::create(
            &std::env::temp_dir(),
            b"owned synthetic cancellation",
        ) {
            Ok(v) => v,
            Err(e) => panic!("owned image: {e}"),
        };
        let path = match artifact.path() {
            Ok(v) => v.to_path_buf(),
            Err(e) => panic!("owned path: {e}"),
        };
        let report = cancelled(Some(&mut artifact));
        assert!(report.startup_cancelled && report.successful());
        assert_eq!(report.worker_exit, None);
        let start = Instant::now();
        loop {
            match std::fs::metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                _ => assert!(start.elapsed() < Duration::from_secs(2)),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
