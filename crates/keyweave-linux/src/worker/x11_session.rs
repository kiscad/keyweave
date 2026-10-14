//! Native transaction glue. Deferred cleanup retains the same owner and terminal endpoint.
use super::*;
use crate::{
    bootstrap::{
        ScheduledX11StartupCleanup, ScheduledX11StartupCleanupReport, X11Bootstrap, X11Prepared,
        X11StartupCleanup, X11StartupOutcome,
    },
    termination::TerminalWriter,
    x11_cleanup::{ReportedX11Cleanup, X11Cleanup, finalize_x11_cleanup},
};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Debug, thiserror::Error)]
pub enum X11SessionError {
    #[error(transparent)]
    Bootstrap(#[from] crate::bootstrap::BootstrapError),
    #[error(transparent)]
    Worker(#[from] WorkerError),
}
pub struct X11SessionConfig {
    pub snapshot: Arc<keyweave_engine::ConfigSnapshot>,
    pub output: crate::output::DeviceSpec,
}
pub enum X11StartupCause {
    Stopped,
    Failed(crate::bootstrap::BootstrapError),
}
pub enum X11SessionResult {
    StartupStopped(Option<Box<ScheduledX11StartupCleanupReport>>),
    StartupFailed {
        error: crate::bootstrap::BootstrapError,
        cleanup: Option<Box<ScheduledX11StartupCleanupReport>>,
    },
    StartupDeferred(Box<DeferredX11Startup>),
    Worker(Box<ReportedX11Cleanup>),
    WorkerDeferred(Box<DeferredX11Worker>),
}
pub struct DeferredX11Startup {
    pub cause: X11StartupCause,
    cleanup: ScheduledX11StartupCleanup,
    terminal: TerminalWriter<File>,
}
pub struct DeferredX11Worker {
    cleanup: X11Cleanup,
    terminal: TerminalWriter<File>,
}
fn startup_done(
    cause: X11StartupCause,
    cleanup: Option<ScheduledX11StartupCleanupReport>,
    terminal: TerminalWriter<File>,
) -> X11SessionResult {
    drop(terminal); // Startup has no Runtime termination report, even after resource cleanup.
    match cause {
        X11StartupCause::Stopped => X11SessionResult::StartupStopped(cleanup.map(Box::new)),
        X11StartupCause::Failed(error) => X11SessionResult::StartupFailed {
            error,
            cleanup: cleanup.map(Box::new),
        },
    }
}
impl DeferredX11Startup {
    pub fn resume(self) -> X11SessionResult {
        let Self {
            cause,
            cleanup,
            terminal,
        } = self;
        let mut clock = MonotonicClock::new();
        match cleanup.run(&mut clock) {
            Ok(report) => startup_done(cause, Some(report), terminal),
            Err(cleanup) => X11SessionResult::StartupDeferred(Box::new(Self {
                cause,
                cleanup: *cleanup,
                terminal,
            })),
        }
    }
}
impl DeferredX11Worker {
    pub fn resume(self) -> X11SessionResult {
        let Self {
            cleanup,
            mut terminal,
        } = self;
        let mut clock = MonotonicClock::new();
        match cleanup.run(&mut clock) {
            Ok(report) => {
                X11SessionResult::Worker(Box::new(finalize_x11_cleanup(report, &mut terminal)))
            }
            Err(cleanup) => X11SessionResult::WorkerDeferred(Box::new(Self {
                cleanup: *cleanup,
                terminal,
            })),
        }
    }
}
fn settle_startup(
    cause: X11StartupCause,
    cleanup: Option<X11StartupCleanup>,
    terminal: TerminalWriter<File>,
) -> X11SessionResult {
    let Some(cleanup) = cleanup else {
        return startup_done(cause, None, terminal);
    };
    let mut clock = MonotonicClock::new();
    let cleanup = cleanup.schedule(clock.now());
    match cleanup.run(&mut clock) {
        Ok(report) => startup_done(cause, Some(report), terminal),
        Err(cleanup) => X11SessionResult::StartupDeferred(Box::new(DeferredX11Startup {
            cause,
            cleanup: *cleanup,
            terminal,
        })),
    }
}
/// Full guarded X11 transaction. Caller authenticates helper and fixes the mixed output spec.
pub fn run_reporting_x11_session(
    startup: ReportingStartup,
    uinput: &Path,
    helper: &Path,
    inputs: Vec<(keyweave_types::DeviceInstanceId, PathBuf)>,
    config: X11SessionConfig,
) -> Result<X11SessionResult, X11SessionError> {
    run_session_using(
        startup,
        uinput,
        helper,
        inputs,
        config,
        run_x11_worker_controlled_report,
    )
}
pub fn run_reporting_x11_session_with_extension<E>(
    startup: ReportingStartup,
    uinput: &Path,
    helper: &Path,
    inputs: Vec<(keyweave_types::DeviceInstanceId, PathBuf)>,
    config: X11SessionConfig,
    extension: &mut E,
) -> Result<X11SessionResult, X11SessionError>
where
    E: WorkerExtension<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
{
    let result = run_session_using(
        startup,
        uinput,
        helper,
        inputs,
        config,
        |runtime, client, guard, lease, clock| {
            run_x11_worker_with_extension(runtime, client, guard, lease, clock, extension)
        },
    );
    extension.close();
    result
}
fn run_session_using<F>(
    startup: ReportingStartup,
    uinput: &Path,
    helper: &Path,
    inputs: Vec<(keyweave_types::DeviceInstanceId, PathBuf)>,
    config: X11SessionConfig,
    run: F,
) -> Result<X11SessionResult, X11SessionError>
where
    F: FnOnce(
        Runtime<crate::capture::EvdevSource, crate::uinput::UinputDevice>,
        ProgressClient<File, File>,
        &WorkerParentGuard,
        crate::x11_consumer::NativeX11Lease,
        MonotonicClock,
    ) -> X11WorkerReport,
{
    let (guard, mut client, terminal) = startup.into_parts();
    guard.verify_isolated_group().map_err(WorkerError::from)?;
    for request in client.wait_requests_with_control(true) {
        crate::ioctl::nonblocking(request.fd).map_err(WorkerError::from)?;
    }
    if client
        .receive_startup_command()
        .map_err(WorkerError::from)?
        == keyweave_progress::RequestEvent::Stop
    {
        return Ok(startup_done(X11StartupCause::Stopped, None, terminal));
    }
    let boot = X11Bootstrap::new(
        &guard,
        uinput,
        helper,
        inputs,
        config.snapshot,
        config.output,
    )?;
    let prepared = match boot.run(&mut client) {
        Ok(X11StartupOutcome::Ready(prepared)) => *prepared,
        Ok(X11StartupOutcome::Stopped(cleanup)) => {
            return Ok(settle_startup(
                X11StartupCause::Stopped,
                cleanup.map(|value| *value),
                terminal,
            ));
        }
        Err(failure) => {
            return Ok(settle_startup(
                X11StartupCause::Failed(failure.error),
                failure.cleanup,
                terminal,
            ));
        }
    };
    let X11Prepared {
        runtime,
        consumer,
        clock,
    } = prepared;
    let worker = run(runtime, client, &guard, consumer, clock);
    let mut clock = MonotonicClock::new();
    let cleanup = X11Cleanup::new_at(worker, clock.now());
    let mut terminal = terminal;
    Ok(match cleanup.run(&mut clock) {
        Ok(report) => {
            X11SessionResult::Worker(Box::new(finalize_x11_cleanup(report, &mut terminal)))
        }
        Err(cleanup) => X11SessionResult::WorkerDeferred(Box::new(DeferredX11Worker {
            cleanup: *cleanup,
            terminal,
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::fd::OwnedFd};
    #[test]
    fn startup_stop_closes_unused_terminal_without_runtime_frame() -> io::Result<()> {
        let (mut reader, writer) = io::pipe()?;
        let terminal = TerminalWriter::new(File::from(OwnedFd::from(writer)), 7)
            .unwrap_or_else(|error| panic!("{error}"));
        let result = startup_done(X11StartupCause::Stopped, None, terminal);
        assert!(matches!(result, X11SessionResult::StartupStopped(None)));
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        assert!(bytes.is_empty());
        Ok(())
    }
    #[test]
    fn startup_failure_retains_cause_and_sends_no_runtime_completion() -> io::Result<()> {
        let (mut reader, writer) = io::pipe()?;
        let terminal = TerminalWriter::new(File::from(OwnedFd::from(writer)), 7)
            .unwrap_or_else(|error| panic!("{error}"));
        let result = startup_done(
            X11StartupCause::Failed(crate::bootstrap::BootstrapError::Inputs),
            None,
            terminal,
        );
        assert!(matches!(
            result,
            X11SessionResult::StartupFailed {
                error: crate::bootstrap::BootstrapError::Inputs,
                cleanup: None
            }
        ));
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        assert!(bytes.is_empty());
        Ok(())
    }
}
