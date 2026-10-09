//! Real process main; binary stdin/stderr must never be read/formatted by stdio.
use keyweave_worker::{Arguments, WorkerExit};
use std::process::ExitCode;

fn main() -> ExitCode {
    // Ordinary Rust panic diagnostics must not corrupt the binary receipt pipe.
    // A panic still fails the process; it cannot certify output recovery.
    std::panic::set_hook(Box::new(|_| {}));
    let args = match Arguments::parse(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(_) => return ExitCode::from(WorkerExit::Arguments as u8),
    };
    ExitCode::from(execute(args) as u8)
}

#[cfg(not(windows))]
fn execute(_args: Arguments) -> WorkerExit {
    // Linux bootstrap remains a separate task; never silently run without it.
    WorkerExit::UnsupportedPlatform
}

#[cfg(windows)]
fn execute(args: Arguments) -> WorkerExit {
    use keyweave_windows::{
        NotificationCleanup, SendInputBackend, Worker, WorkerStartup, WorkerStopReason,
        runtime::ActorPhase,
    };
    // Initial main thread: no other thread, logging, input API, or synchronous
    // stdio before this claim. Rejection is final; never retry/install directly.
    let mut startup = match WorkerStartup::claim(args.parent, args.run_id) {
        Ok(startup) => startup,
        Err(_) => return WorkerExit::Admission,
    };
    let loaded = if let Some(value) = args.config_handle {
        let mut reader = match startup.claim_config_reader(value) {
            Ok(reader) => reader,
            Err(_) => return WorkerExit::Admission,
        };
        // Consume the inherited file object, never reopen the diagnostic path.
        let loaded = keyweave_worker::load_config(&mut reader);
        if reader.close().is_err() {
            return WorkerExit::Cleanup;
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
        Err(_) => return WorkerExit::Config,
    };
    // This constructor allocates only; it does not install or inject input.
    let backend = SendInputBackend::new(args.tag);
    let mut worker = match Worker::install(startup, backend, args.tag, args.identity, config) {
        Ok(worker) => worker,
        Err(_) => return WorkerExit::Install,
    };
    if worker.run().is_err() {
        let _ = worker.request_stop();
        return WorkerExit::Loop;
    }
    let Some(report) = worker.report() else {
        return WorkerExit::Loop;
    };
    let Some(shutdown) = worker.shutdown_report() else {
        return WorkerExit::Cleanup;
    };
    let outcome = keyweave_worker::TerminalFacts {
        fault: report.reason == WorkerStopReason::Fault || report.primary.is_some(),
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
    .exit();
    // Explicit Session shutdown already ran. Drop does not inject. Returning
    // exits the real process and releases original borrowed binary std handles.
    // A scalar exit code is classification, not a full error/ledger report.
    drop(worker);
    outcome
}
