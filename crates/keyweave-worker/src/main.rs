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
    use keyweave_worker::linux_preparation::PreparedConfig;
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
    if matches!(
        args.session_provider,
        keyweave_worker::linux_arguments::SessionProvider::Console { .. }
            | keyweave_worker::linux_arguments::SessionProvider::X11 { .. }
    ) && startup.guard().verify_isolated_group().is_err()
    {
        return WorkerExit::Admission.into();
    }
    let reload = match startup.reload_requested() {
        Ok(false) => None,
        Ok(true) => {
            if startup.guard().verify_isolated_group().is_err() {
                return WorkerExit::Admission.into();
            }
            match startup.claim_reload() {
                Ok(endpoints) => Some(endpoints),
                Err(_) => return WorkerExit::Admission.into(),
            }
        }
        Err(_) => return WorkerExit::Admission.into(),
    };
    let config = match startup.claim_config() {
        Ok(config) => config,
        Err(_) => return WorkerExit::Admission.into(),
    };
    let bytes = match config.read_bytes() {
        Ok(bytes) => bytes,
        Err(_) => return WorkerExit::Config.into(),
    };
    drop(config); // Original inherited FD remains reserved until true exit.
    let spec = match args.output_spec() {
        Ok(spec) => spec,
        Err(_) => return WorkerExit::Config.into(),
    };
    let prepared = match PreparedConfig::from_bytes_for_generation_with_scope(
        bytes,
        spec,
        keyweave_types::ConfigGeneration(1),
        args.execution_mode,
        args.capture_scope(),
    ) {
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
            let result = if let Some(endpoints) = reload {
                let (reader, writer) = endpoints.into_parts();
                let mut controller = match keyweave_worker::reload_control::Controller::new(
                    reader,
                    writer,
                    args.run_id,
                    prepared.snapshot().generation,
                    5_000_000_000,
                    keyweave_worker::reload_preparation::Target::linux_prepared(&prepared),
                ) {
                    Ok(controller) => controller,
                    Err(_) => return WorkerExit::Admission.into(),
                };
                let result =
                    keyweave_linux::worker::run_reporting_console_session_prepared_with_extension(
                        startup,
                        &vt,
                        &args.uinput,
                        inputs,
                        keyweave_linux::worker::ConsoleConfig {
                            snapshot: prepared.snapshot().clone(),
                            output: prepared.output_spec().clone(),
                            mode: prepared.execution_mode(),
                        },
                        &mut controller,
                    );
                // Original owner retained through input recovery/terminal send.
                // A bounded closed-job observation never restarts or activates it.
                if keyweave_worker::reload_control::reap_after_shutdown(&mut controller).is_err() {
                    return WorkerExit::Cleanup.into();
                }
                result
            } else {
                keyweave_linux::worker::run_reporting_console_session_prepared(
                    startup,
                    &vt,
                    &args.uinput,
                    inputs,
                    keyweave_linux::worker::ConsoleConfig {
                        snapshot: prepared.snapshot().clone(),
                        output: prepared.output_spec().clone(),
                        mode: prepared.execution_mode(),
                    },
                )
            };
            match result {
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
        keyweave_worker::linux_arguments::SessionProvider::X11 { helper, .. } => {
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
            let config = match prepared.x11_session_config() {
                Ok(config) => config,
                Err(_) => return WorkerExit::Config.into(),
            };
            let result = if let Some(endpoints) = reload {
                let (reader, writer) = endpoints.into_parts();
                let mut controller = match keyweave_worker::reload_control::Controller::new(
                    reader,
                    writer,
                    args.run_id,
                    prepared.snapshot().generation,
                    5_000_000_000,
                    keyweave_worker::reload_preparation::Target::linux_prepared(&prepared),
                ) {
                    Ok(controller) => controller,
                    Err(_) => return WorkerExit::Admission.into(),
                };
                let result = keyweave_linux::worker::run_reporting_x11_session_with_extension(
                    startup,
                    &args.uinput,
                    &helper,
                    inputs,
                    config,
                    &mut controller,
                );
                if keyweave_worker::reload_control::reap_after_shutdown(&mut controller).is_err() {
                    return WorkerExit::Cleanup.into();
                }
                result
            } else {
                keyweave_linux::worker::run_reporting_x11_session(
                    startup,
                    &args.uinput,
                    &helper,
                    inputs,
                    config,
                )
            };
            x11_session_exit(result).into()
        }
    }
}
#[cfg(target_os = "linux")]
fn x11_session_exit(
    result: Result<
        keyweave_linux::worker::X11SessionResult,
        keyweave_linux::worker::X11SessionError,
    >,
) -> WorkerExit {
    use keyweave_linux::worker::X11SessionResult as R;
    match result {
        Ok(R::StartupStopped(None)) => WorkerExit::Clean,
        Ok(R::StartupStopped(Some(report))) => {
            if !report.startup.owner_dropped
                || report.startup.cancel_error.is_some()
                || report.cancel_error.is_some()
                || report.poll_error.is_some()
                || report.clock_error.is_some()
                || report.wait_error.is_some()
                || report.timing_failure.is_some()
            {
                WorkerExit::Cleanup
            } else {
                WorkerExit::Clean
            }
        }
        Ok(R::StartupFailed { .. }) => WorkerExit::Install,
        Ok(R::StartupDeferred(_) | R::WorkerDeferred(_)) => WorkerExit::Cleanup,
        Ok(R::Worker(report)) => {
            let keyweave_linux::x11_cleanup::ReportedX11Cleanup { cleanup, delivery } = *report;
            if !cleanup.consumer_owner_dropped()
                || cleanup.prior_cancel_error.is_some()
                || cleanup.cancel_error.is_some()
                || cleanup.poll_error.is_some()
                || cleanup.clock_error.is_some()
                || cleanup.wait_error.is_some()
                || cleanup.timing_failure.is_some()
            {
                return WorkerExit::Cleanup;
            }
            linux_report_exit(keyweave_linux::worker::ReportedWorker {
                worker: cleanup.worker,
                delivery,
            })
        }
        Err(_) => WorkerExit::Install,
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
        let loaded = keyweave_worker::preparation::PreparedConfig::read_with_mode(
            &mut reader,
            args.execution_mode,
        );
        if reader.close().is_err() {
            return WorkerExit::Cleanup.into();
        }
        loaded
    } else {
        // Explicit legacy three-handle mode only; complete host must select
        // the immutable four-handle contract instead of reopening user paths.
        std::fs::File::open(&args.config)
            .map_err(keyweave_worker::LoadError::from)
            .map_err(keyweave_worker::preparation::PreparationError::from)
            .and_then(|file| {
                keyweave_worker::preparation::PreparedConfig::read_with_mode(
                    file,
                    args.execution_mode,
                )
            })
    };
    let prepared = match loaded {
        Ok(config) => config,
        Err(_) => return WorkerExit::Config.into(),
    };
    // This constructor allocates only; it does not install or inject input.
    let backend = SendInputBackend::new(args.tag);
    let mut worker = match Worker::install_with_mode(
        startup,
        backend,
        args.tag,
        args.identity,
        prepared.snapshot().clone(),
        prepared.execution_mode(),
    ) {
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

#[cfg(all(test, target_os = "linux"))]
mod x11_exit_tests {
    use super::*;
    use keyweave_linux::{
        bootstrap::{ScheduledX11StartupCleanupReport, X11StartupCleanupReport},
        worker::X11SessionResult,
        x11_cleanup::{CleanupConfirmation, CleanupTimingFailure},
    };
    fn stopped() -> ScheduledX11StartupCleanupReport {
        ScheduledX11StartupCleanupReport {
            startup: X11StartupCleanupReport {
                cancel_error: None,
                observed_exit: false,
                owner_dropped: true,
            },
            confirmation: CleanupConfirmation::NoPendingJob,
            cancel_error: None,
            poll_error: None,
            clock_error: None,
            wait_error: None,
            timing_failure: None,
        }
    }
    #[test]
    fn x11_startup_clean_requires_confirmed_cleanup_without_hidden_errors() {
        assert_eq!(
            x11_session_exit(Ok(X11SessionResult::StartupStopped(None))),
            WorkerExit::Clean
        );
        assert_eq!(
            x11_session_exit(Ok(X11SessionResult::StartupStopped(Some(Box::new(
                stopped()
            ))))),
            WorkerExit::Clean
        );
        for failure in 0..6 {
            let mut report = stopped();
            match failure {
                0 => report.startup.owner_dropped = false,
                1 => report.startup.cancel_error = Some(std::io::Error::other("cancel")),
                2 => report.poll_error = Some(std::io::Error::other("poll")),
                3 => report.clock_error = Some(std::io::Error::other("clock")),
                4 => report.wait_error = Some(std::io::Error::other("wait")),
                _ => report.timing_failure = Some(CleanupTimingFailure::Deadline),
            };
            assert_eq!(
                x11_session_exit(Ok(X11SessionResult::StartupStopped(Some(Box::new(report))))),
                WorkerExit::Cleanup
            );
        }
    }
    #[test]
    fn x11_startup_failure_never_becomes_clean() {
        assert_eq!(
            x11_session_exit(Ok(X11SessionResult::StartupFailed {
                error: keyweave_linux::bootstrap::BootstrapError::Inputs,
                cleanup: None
            })),
            WorkerExit::Install
        );
        assert_eq!(
            x11_session_exit(Err(keyweave_linux::worker::X11SessionError::Bootstrap(
                keyweave_linux::bootstrap::BootstrapError::Inputs
            ))),
            WorkerExit::Install
        );
    }
}
