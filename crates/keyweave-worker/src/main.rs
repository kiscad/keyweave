//! Real process main; binary stdin/stderr must never be read/formatted by stdio.
use keyweave_worker::{Arguments, NativeWorkerExit, WorkerExit};
use std::process::ExitCode;

fn main() -> ExitCode {
    // Ordinary Rust panic diagnostics must not corrupt the binary receipt pipe.
    // A panic still fails the process; it cannot certify output recovery.
    std::panic::set_hook(Box::new(|_| {}));
    #[cfg(target_os = "linux")]
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(
            keyweave_worker::linux_arguments::LINUX_WORKER_ARGUMENT,
        ))
    {
        let args = match keyweave_worker::linux_arguments::LinuxArguments::parse(
            std::env::args_os().skip(1),
        ) {
            Ok(args) => args,
            Err(_) => return ExitCode::from(WorkerExit::Arguments as u8),
        };
        return finish(execute_linux(args));
    }
    let args = match Arguments::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(_) => return ExitCode::from(WorkerExit::Arguments as u8),
    };
    finish(execute(args))
}

#[cfg(target_os = "linux")]
fn execute_linux(args: keyweave_worker::linux_arguments::LinuxArguments) -> NativeWorkerExit {
    use keyweave_worker::linux_preparation::{PreparedConfig, default_output_spec};
    // Real main, before path lookup/device opening/threads. No fallback to a
    // config pathname or weaker two-channel startup if this claim fails.
    let mut startup = match keyweave_linux::worker::ReportingStartup::from_supervisor_environment()
    {
        Ok(startup) => startup,
        Err(_) => return WorkerExit::Admission.into(),
    };
    if startup.guard().parent_pid() != args.parent || startup.run_id() != args.run_id {
        return WorkerExit::Admission.into();
    }
    let config = match startup.claim_config() {
        Ok(config) => config,
        Err(_) => return WorkerExit::Admission.into(),
    };
    let bytes = match config.read_bytes() {
        Ok(bytes) => bytes,
        Err(_) => return WorkerExit::Config.into(),
    };
    drop(config); // Original inherited FD remains reserved until true exit.
    let spec = match default_output_spec() {
        Ok(spec) => spec,
        Err(_) => return WorkerExit::Config.into(),
    };
    let prepared = match PreparedConfig::from_bytes(bytes, spec) {
        Ok(prepared) => prepared,
        Err(_) => return WorkerExit::Config.into(),
    };
    // None must NEVER open devices, even though an explicit console mode exists.
    match args.session_provider {
        keyweave_worker::linux_arguments::SessionProvider::None => {
            WorkerExit::UnsupportedPlatform.into()
        }
        keyweave_worker::linux_arguments::SessionProvider::Console { vt } => {
            let inputs = args
                .inputs
                .into_iter()
                .enumerate()
                .map(|(index, path)| {
                    (
                        keyweave_types::DeviceInstanceId {
                            id: index as u64 + 1,
                            generation: 1,
                        },
                        path,
                    )
                })
                .collect();
            match keyweave_linux::worker::run_reporting_console_session(
                startup,
                &vt,
                &args.uinput,
                inputs,
                prepared.snapshot().clone(),
                prepared.output_spec().clone(),
            ) {
                Ok(keyweave_linux::worker::ConsoleSessionResult::StartupStopped) => {
                    WorkerExit::Clean.into()
                }
                Ok(keyweave_linux::worker::ConsoleSessionResult::Worker(report)) => {
                    linux_report_exit(report).into()
                }
                Err(keyweave_linux::worker::ConsoleSessionError::Bootstrap(error)) => match error {
                    keyweave_linux::bootstrap::BootstrapError::Progress(_)
                    | keyweave_linux::bootstrap::BootstrapError::Endpoint => {
                        WorkerExit::Loop.into()
                    }
                    _ => WorkerExit::Install.into(),
                },
                Err(keyweave_linux::worker::ConsoleSessionError::Worker(_)) => {
                    WorkerExit::Loop.into()
                }
            }
        }
    }
}
#[cfg(target_os = "linux")]
fn linux_report_exit(report: keyweave_linux::worker::ReportedWorker) -> WorkerExit {
    let Some(terminal) = report.worker.termination else {
        return WorkerExit::Cleanup;
    };
    if !terminal.input_owners_released
        || !terminal.engine_reset_confirmed
        || terminal.recovery_failed
        || terminal.recovery_budget_exhausted
        || terminal.submitted_remaining
        || terminal.report_pending
        || terminal.acceptance_unknown
        || !terminal.output_recovery_confirmed
        || !terminal.output_owner_dropped
        || !terminal.backend_transition_confirmed
        || !report.delivery.frame_sent
        || report.delivery.failure.is_some()
    {
        return WorkerExit::Cleanup;
    }
    if !matches!(report.worker.outcome,Ok(reason) if reason==terminal.reason && matches!(reason,
        keyweave_linux::runtime::StopReason::Pause|keyweave_linux::runtime::StopReason::Emergency))
    {
        return WorkerExit::Loop;
    }
    WorkerExit::Clean
}
#[cfg(all(test, target_os = "linux"))]
mod linux_exit_tests {
    use super::*;
    use keyweave_linux::{
        runtime::{StopReason, TerminationReport},
        termination::bounded::{DeliveryFailure, DeliveryReport},
        worker::{ReportedWorker, WorkerError, WorkerReport},
    };
    fn report() -> ReportedWorker {
        ReportedWorker {
            worker: WorkerReport {
                outcome: Ok(StopReason::Pause),
                termination: Some(TerminationReport {
                    reason: StopReason::Pause,
                    input_owners_released: true,
                    engine_reset_confirmed: true,
                    recovery_failed: false,
                    recovery_budget_exhausted: false,
                    submitted_remaining: false,
                    report_pending: false,
                    acceptance_unknown: false,
                    output_recovery_confirmed: true,
                    output_owner_dropped: true,
                    backend_transition_confirmed: true,
                }),
            },
            delivery: DeliveryReport {
                frame_sent: true,
                attempts: 1,
                failure: None,
            },
        }
    }
    #[test]
    fn delivery_and_recovery_uncertainty_override_success_or_loop_error() {
        let mut r = report();
        r.delivery.failure = Some(DeliveryFailure::MissingReport);
        assert_eq!(linux_report_exit(r), WorkerExit::Cleanup);
        let mut r = report();
        r.worker.outcome = Err(WorkerError::Endpoint(StopReason::ControlFault));
        if let Some(t) = r.worker.termination.as_mut() {
            t.output_recovery_confirmed = false;
            t.submitted_remaining = true;
        }
        assert_eq!(linux_report_exit(r), WorkerExit::Cleanup);
        let mut r = report();
        r.worker.termination = None;
        assert_eq!(linux_report_exit(r), WorkerExit::Cleanup);
    }
    #[test]
    fn loop_error_and_contradictory_reason_cannot_become_clean() {
        assert_eq!(linux_report_exit(report()), WorkerExit::Clean);
        let mut r = report();
        r.worker.outcome = Err(WorkerError::Endpoint(StopReason::ControlFault));
        assert_eq!(linux_report_exit(r), WorkerExit::Loop);
        let mut r = report();
        r.worker.outcome = Ok(StopReason::Emergency);
        assert_eq!(linux_report_exit(r), WorkerExit::Loop);
        let mut r = report();
        r.worker.outcome = Ok(StopReason::Emergency);
        if let Some(t) = r.worker.termination.as_mut() {
            t.reason = StopReason::Emergency;
        }
        assert_eq!(linux_report_exit(r), WorkerExit::Clean);
    }
}

#[cfg(not(windows))]
fn execute(_args: Arguments) -> NativeWorkerExit {
    // Legacy Windows arguments do not select the guarded Linux console entry.
    WorkerExit::UnsupportedPlatform.into()
}

#[cfg(windows)]
fn execute(args: Arguments) -> NativeWorkerExit {
    use keyweave_windows::{
        NotificationCleanup, SendInputBackend, Worker, WorkerStartup, WorkerStopReason,
        runtime::ActorPhase,
    };
    // Initial main thread: no other thread, logging, input API, or synchronous
    // stdio before this claim. Rejection is final; never retry/install directly.
    let mut startup = match WorkerStartup::claim(args.parent, args.run_id) {
        Ok(startup) => startup,
        Err(_) => return WorkerExit::Admission.into(),
    };
    let loaded = if let Some(value) = args.config_handle {
        let mut reader = match startup.claim_config_reader(value) {
            Ok(reader) => reader,
            Err(_) => return WorkerExit::Admission.into(),
        };
        // Consume the inherited file object, never reopen the diagnostic path.
        let loaded = keyweave_worker::load_config(&mut reader);
        if reader.close().is_err() {
            return WorkerExit::Cleanup.into();
        }
        loaded
    } else {
        // Explicit legacy three-handle mode only; complete host must select
        // the immutable four-handle contract instead of reopening user paths.
        std::fs::File::open(&args.config)
            .map_err(keyweave_worker::LoadError::from)
            .and_then(keyweave_worker::load_config)
    };
    let config = match loaded {
        Ok(config) => config,
        Err(_) => return WorkerExit::Config.into(),
    };
    // This constructor allocates only; it does not install or inject input.
    let backend = SendInputBackend::new(args.tag);
    let mut worker = match Worker::install(startup, backend, args.tag, args.identity, config) {
        Ok(worker) => worker,
        Err(_) => return WorkerExit::Install.into(),
    };
    let run_failed = worker.run().is_err();
    if run_failed {
        let _ = worker.request_stop();
    }
    let Some(report) = worker.report() else {
        return WorkerExit::Loop.into();
    };
    let Some(shutdown) = worker.shutdown_report() else {
        return WorkerExit::Cleanup.into();
    };
    let outcome = keyweave_worker::TerminalFacts {
        fault: run_failed
            || report.reason == WorkerStopReason::Fault
            || report.primary.is_some()
            || shutdown.application_provider.is_some(),
        progress_failed: report.progress.is_some(),
        shutdown_present: true,
        stopped: worker.phase() == ActorPhase::Stopped,
        unhook_ok: shutdown.unhook.is_ok(),
        recovery_ok: shutdown.recovery.is_ok(),
        notifications_ok: shutdown.notifications == NotificationCleanup::default(),
        access_lost: shutdown.access_lost,
        session_changed: shutdown.session_changed,
        untracked_input: shutdown.untracked_input,
    }
    .native_exit();
    // Explicit Session shutdown already ran. Drop does not inject. Returning
    // exits the real process and releases original borrowed binary std handles.
    // Terminal mask preserves independent facts, not API details/ledger proof.
    drop(worker);
    outcome
}

#[cfg(windows)]
fn finish(outcome: NativeWorkerExit) -> ExitCode {
    // execute has returned and dropped all owners, including worker. Stable
    // ExitCode only accepts u8; preserve the Windows mask without skipping an
    // active owner's Drop. The closed outcome type guarantees nonnegative i32.
    match u8::try_from(outcome.raw()) {
        Ok(code) => ExitCode::from(code),
        Err(_) => std::process::exit(outcome.native_i32()),
    }
}
#[cfg(not(windows))]
fn finish(outcome: NativeWorkerExit) -> ExitCode {
    // Preserve small scalar failures; never truncate the Windows terminal mask.
    ExitCode::from(u8::try_from(outcome.raw()).unwrap_or(WorkerExit::UnsupportedPlatform as u8))
}
