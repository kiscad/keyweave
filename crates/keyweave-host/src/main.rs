use clap::{Parser, Subcommand, ValueEnum};
use keyweave_windows::keys::IdentityPolicy;
use keyweave_worker::preparation::PreparedConfig;
use std::{path::PathBuf, process::ExitCode};
#[derive(Parser)]
#[command(version, about = "KeyWeave session host")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run an experimental Linux session. Ctrl+C, SIGTERM or SIGHUP requests stop.
    RunLinux {
        #[arg(long, value_enum, default_value = "legacy")]
        execution_mode: LinuxExecution,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        worker: Option<PathBuf>,
        #[arg(long, default_value = "/dev/uinput")]
        uinput: PathBuf,
        #[arg(long, required = true)]
        input: Vec<PathBuf>,
        #[arg(long, value_enum)]
        session_provider: LinuxProvider,
        /// Explicit text VT, required with console; omitted with none.
        #[arg(long)]
        vt: Option<PathBuf>,
        #[command(flatten)]
        x11: X11Args,
        #[command(flatten)]
        watch: WatchArgs,
        /// Explicit local same-user status/stop socket in an existing private directory.
        #[arg(long)]
        control_socket: Option<PathBuf>,
    },
    /// List bounded evdev metadata only; no event read, grab or worker creation.
    ListLinuxDevices,
    /// Query a Linux host; no input access or worker creation.
    StatusLinux {
        #[arg(long)]
        socket: PathBuf,
    },
    /// Request stop of a previously observed public session; does not imply exit.
    StopLinux {
        #[arg(long)]
        socket: PathBuf,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        session: u64,
    },
    /// Query a selected Windows host instance; no worker creation or input access.
    StatusWindows {
        #[arg(long)]
        endpoint: keyweave_host::public_control::WindowsEndpoint,
    },
    /// Request stop of the observed Windows public session; does not imply exit.
    StopWindows {
        #[arg(long)]
        endpoint: keyweave_host::public_control::WindowsEndpoint,
        #[arg(long,value_parser=clap::value_parser!(u64).range(1..))]
        session: u64,
    },
    /// Check Linux configuration against the full supported output table; no input access.
    CheckLinux {
        #[arg(long, value_enum, default_value = "legacy")]
        execution_mode: LinuxExecution,
        #[arg(long)]
        config: PathBuf,
        #[command(flatten)]
        relative: RelativeArgs,
    },
    /// Check a Windows configuration without opening input or starting a worker.
    CheckWindows {
        #[arg(long, value_enum, default_value = "legacy")]
        execution_mode: WindowsExecution,
        #[arg(long, value_enum, default_value = "keyboard")]
        capture_scope: WindowsScope,
        #[arg(long)]
        config: PathBuf,
    },
    /// Run an experimental Windows remapping session. Enter or Ctrl+C stops it.
    RunWindows {
        #[arg(long, value_enum, default_value = "legacy")]
        execution_mode: WindowsExecution,
        #[arg(long, value_enum, default_value = "keyboard")]
        capture_scope: WindowsScope,
        #[arg(long)]
        config: PathBuf,
        #[arg(long, value_enum)]
        identity: Identity,
        /// Explicit trusted developer executable; otherwise use the packaged sibling.
        #[arg(long)]
        worker: Option<PathBuf>,
        /// Existing local directory for owned temporary configuration files.
        #[arg(long)]
        artifact_directory: Option<PathBuf>,
        /// Explicit local named-pipe endpoint; publishes cold host instance metadata.
        #[arg(long)]
        control_pipe: Option<String>,
    },
}
#[derive(clap::Args)]
struct WatchArgs {
    /// Reload changed configuration contents while a console or X11 session is active.
    #[arg(long)]
    watch_config: bool,
    /// File check interval (100..60000 milliseconds); default 1000 when enabled.
    #[arg(long, requires = "watch_config", value_parser = clap::value_parser!(u64).range(100..=60000))]
    watch_interval_ms: Option<u64>,
}
#[derive(clap::Args, Default)]
struct X11Args {
    /// Absolute path to the paired keyweave-x11-probe executable (X11 only).
    #[arg(long)]
    x11_probe: Option<PathBuf>,
    #[command(flatten)]
    relative: RelativeArgs,
}
#[derive(clap::Args, Default)]
struct RelativeArgs {
    /// Explicit evdev relative axes: 0,1,6,8,11,12; comma separated.
    #[arg(long, value_delimiter = ',')]
    relative_axes: Vec<u16>,
    /// Explicit evdev buttons: 272..279; comma separated, omitted for none.
    #[arg(long, value_delimiter = ',')]
    pointing_buttons: Vec<u16>,
}
type RelativeCapabilities = (
    keyweave_linux::relative::RelativeAxes,
    Option<keyweave_linux::keys::PointingButtons>,
);
impl RelativeArgs {
    fn capabilities(&self) -> Result<Option<RelativeCapabilities>, String> {
        if self.relative_axes.is_empty() && self.pointing_buttons.is_empty() {
            return Ok(None);
        }
        let axes = keyweave_linux::relative::RelativeAxes::new(&self.relative_axes)
            .map_err(|e| e.to_string())?;
        let buttons = if self.pointing_buttons.is_empty() {
            None
        } else {
            if !axes.contains(0) || !axes.contains(1) {
                return Err("pointing buttons require relative axes 0 and 1".into());
            }
            Some(
                keyweave_linux::keys::PointingButtons::new(&self.pointing_buttons)
                    .map_err(|e| e.to_string())?,
            )
        };
        Ok(Some((axes, buttons)))
    }
}
#[cfg(target_os = "linux")]
fn linux_provider(
    provider: LinuxProvider,
    vt: Option<PathBuf>,
    mode: LinuxExecution,
    x11: X11Args,
) -> Result<keyweave_worker::linux_arguments::SessionProvider, String> {
    use keyweave_worker::linux_arguments::SessionProvider;
    if !matches!(provider, LinuxProvider::X11)
        && (x11.x11_probe.is_some()
            || !x11.relative.relative_axes.is_empty()
            || !x11.relative.pointing_buttons.is_empty())
    {
        return Err("X11 options require --session-provider x11".into());
    }
    match (provider, vt) {
        (LinuxProvider::None, None) => Ok(SessionProvider::None),
        (LinuxProvider::Console, Some(vt)) => Ok(SessionProvider::Console { vt }),
        (LinuxProvider::X11, None) => {
            if !matches!(mode, LinuxExecution::StagedKeyboard) {
                return Err("x11 requires --execution-mode staged-keyboard".into());
            }
            let helper = x11
                .x11_probe
                .filter(|path| path.is_absolute())
                .ok_or("x11 requires an absolute --x11-probe path")?;
            let (axes, buttons) = x11
                .relative
                .capabilities()?
                .ok_or("x11 requires explicit --relative-axes")?;
            Ok(SessionProvider::X11 {
                helper,
                axes,
                buttons,
            })
        }
        _ => Err("console requires --vt; none and x11 require --vt to be omitted".into()),
    }
}
#[derive(Clone, Copy, clap::ValueEnum)]
enum WindowsScope {
    Keyboard,
    StagedMixed,
}
impl WindowsScope {
    fn runtime(self) -> keyweave_worker::preparation::CaptureScope {
        match self {
            Self::Keyboard => keyweave_worker::preparation::CaptureScope::Keyboard,
            Self::StagedMixed => keyweave_worker::preparation::CaptureScope::StagedMixed,
        }
    }
}
fn prepare_windows_reader(
    reader: impl std::io::Read,
    mode: WindowsExecution,
    scope: WindowsScope,
) -> Result<PreparedConfig, String> {
    PreparedConfig::read_for_generation_with_scope(
        reader,
        keyweave_types::ConfigGeneration(1),
        mode.runtime(),
        scope.runtime(),
    )
    .map_err(|e| e.to_string())
}
#[derive(Clone, Copy, clap::ValueEnum)]
enum WindowsExecution {
    Legacy,
    StagedKeyboard,
}
impl WindowsExecution {
    fn runtime(self) -> keyweave_worker::preparation::ExecutionMode {
        match self {
            Self::Legacy => keyweave_worker::preparation::ExecutionMode::Legacy,
            Self::StagedKeyboard => keyweave_worker::preparation::ExecutionMode::StagedKeyboard,
        }
    }
}
#[derive(Clone, Copy, clap::ValueEnum)]
enum LinuxExecution {
    Legacy,
    StagedKeyboard,
}
impl LinuxExecution {
    fn runtime(self) -> keyweave_worker::linux_preparation::ExecutionMode {
        match self {
            Self::Legacy => keyweave_worker::linux_preparation::ExecutionMode::Legacy,
            Self::StagedKeyboard => {
                keyweave_worker::linux_preparation::ExecutionMode::StagedKeyboard
            }
        }
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum LinuxProvider {
    None,
    Console,
    X11,
}
#[derive(Clone, Copy, ValueEnum)]
enum Identity {
    Strict,
    CanonicalScan,
}
impl From<Identity> for IdentityPolicy {
    fn from(value: Identity) -> Self {
        match value {
            Identity::Strict => Self::StrictUnique,
            Identity::CanonicalScan => Self::CanonicalScan,
        }
    }
}
fn main() -> ExitCode {
    match Cli::parse().command {
        Command::RunLinux {
            execution_mode,
            config,
            worker,
            uinput,
            input,
            session_provider,
            vt,
            x11,
            watch,
            control_socket,
        } => run_linux(
            config,
            worker,
            uinput,
            input,
            session_provider,
            vt,
            (watch, execution_mode, x11, control_socket),
        ),
        Command::StatusWindows { endpoint } => windows_local_control(endpoint, None),
        Command::StopWindows { endpoint, session } => {
            windows_local_control(endpoint, Some(session))
        }
        Command::ListLinuxDevices => list_linux_devices(),
        Command::StatusLinux { socket } => local_control(socket, None),
        Command::StopLinux { socket, session } => local_control(socket, Some(session)),
        Command::CheckLinux {
            config,
            execution_mode,
            relative,
        } => match check_linux(config, execution_mode, relative) {
            Ok(config) => {
                println!(
                    "Linux configuration accepted (offline): bytes={}, generation={}, mode={:?}, scope={:?}; device readiness unverified",
                    config.bytes().len(),
                    config.snapshot().generation.0,
                    config.execution_mode(),
                    config.capture_scope()
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("configuration rejected: {error}");
                ExitCode::from(1)
            }
        },
        Command::CheckWindows {
            config,
            execution_mode,
            capture_scope,
        } => match std::fs::File::open(config)
            .map_err(|e| e.to_string())
            .and_then(|f| prepare_windows_reader(f, execution_mode, capture_scope))
        {
            Ok(config) => {
                println!(
                    "configuration accepted offline: bytes={}, generation={}, mode={:?}, scope={:?}; device readiness unverified",
                    config.bytes().len(),
                    config.snapshot().generation.0,
                    config.execution_mode(),
                    config.capture_scope()
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("configuration rejected: {error}");
                ExitCode::from(1)
            }
        },
        Command::RunWindows {
            execution_mode,
            capture_scope,
            config,
            identity,
            worker,
            artifact_directory,
            control_pipe,
        } => run(
            config,
            identity.into(),
            worker,
            artifact_directory,
            execution_mode,
            capture_scope,
            control_pipe,
        ),
    }
}
#[cfg(not(windows))]
fn run(
    _: PathBuf,
    _: IdentityPolicy,
    _: Option<PathBuf>,
    _: Option<PathBuf>,
    _: WindowsExecution,
    _: WindowsScope,
    _: Option<String>,
) -> ExitCode {
    eprintln!("run-windows requires Windows");
    ExitCode::from(78)
}
#[cfg(windows)]
fn run(
    config: PathBuf,
    identity: IdentityPolicy,
    worker: Option<PathBuf>,
    directory: Option<PathBuf>,
    execution_mode: WindowsExecution,
    capture_scope: WindowsScope,
    control_pipe: Option<String>,
) -> ExitCode {
    use std::{
        io::Read,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    let preparation = (|| -> Result<_, String> {
        let executable = keyweave_host::locate_worker(
            &std::env::current_exe().map_err(|e| e.to_string())?,
            worker.as_deref(),
        )
        .map_err(|e| format!("worker location: {e}"))?;
        let config = config
            .canonicalize()
            .map_err(|e| format!("config path: {e}"))?;
        let prepared = prepare_windows_reader(
            std::fs::File::open(&config).map_err(|e| e.to_string())?,
            execution_mode,
            capture_scope,
        )?;
        let directory = directory.unwrap_or_else(std::env::temp_dir);
        if !directory.is_absolute() {
            return Err("artifact directory must be absolute".into());
        }
        Ok((executable, config, prepared, directory))
    })();
    let (executable, path, prepared, directory) = match preparation {
        Ok(v) => v,
        Err(e) => {
            eprintln!("startup rejected: {e}");
            return ExitCode::from(1);
        }
    };
    let mut console = match keyweave_windows::ConsoleControl::install() {
        Ok(control) => control,
        Err(error) => {
            eprintln!("console control startup failed: {error}");
            return ExitCode::from(1);
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let input_stop = stop.clone();
    // This bounded read is on an independent thread, never the control loop.
    // Any input/EOF/read failure requests stop. Don't join a blocked console read.
    if let Err(error) = std::thread::Builder::new()
        .name("keyweave-user-stop".into())
        .spawn(move || {
            let mut byte = [0u8; 1];
            let _ = std::io::stdin().read(&mut byte);
            input_stop.store(true, Ordering::Release);
        })
    {
        let cleanup = console.close();
        eprintln!("control input startup failed: {error}");
        if let Err(error) = cleanup {
            eprintln!("console control cleanup failed: {error}");
        }
        return ExitCode::from(1);
    }
    if console.available() {
        eprintln!("Starting Windows session; Enter, Ctrl+C or Ctrl+Break stops it.");
    } else {
        eprintln!("Starting Windows session; input or EOF requests stop.");
    }
    let mut public_control = match control_pipe
        .as_deref()
        .map(keyweave_host::windows_control::Control::listen)
        .transpose()
    {
        Ok(service) => service,
        Err(error) => {
            let cleanup = console.close();
            eprintln!(
                "public control setup failed before worker start: {error}; console_cleanup={cleanup:?}"
            );
            return ExitCode::from(1);
        }
    };
    // Cold output only: a blocked consumer cannot stall an active worker lease.
    if let Some(service) = &public_control {
        println!(
            "control_endpoint={} public_session={}",
            service.endpoint(),
            service.session()
        );
    }
    let result = if let Some(service) = public_control.as_mut() {
        keyweave_host::windows::run_with_console_control(
            &executable,
            &path,
            &directory,
            prepared,
            identity,
            &stop,
            &console,
            service,
        )
    } else {
        keyweave_host::windows::run_with_console(
            &executable,
            &path,
            &directory,
            prepared,
            identity,
            &stop,
            &console,
        )
    };
    let public_cleanup = public_control
        .as_mut()
        .map(keyweave_host::windows_control::Control::close)
        .transpose();
    // Restore default processing before final stdio; no active worker remains
    // on a successful result. Cleanup failure is independent and fails the host.
    let cleanup = console.close();
    if let Err(error) = public_cleanup {
        eprintln!(
            "public control cleanup failed: {error}; host_result={result:?}; console_cleanup={cleanup:?}"
        );
        return ExitCode::from(1);
    }
    match result {
        Ok(report) => {
            println!("{report}");
            if let Err(error) = cleanup {
                eprintln!("console control cleanup failed: {error}");
                return ExitCode::from(1);
            }
            ExitCode::from(if report.successful() { 0 } else { 1 })
        }
        Err(error) => {
            eprintln!("{error}");
            if let Err(error) = cleanup {
                eprintln!("console control cleanup failed: {error}");
            }
            ExitCode::from(1)
        }
    }
}

fn check_linux(
    path: PathBuf,
    mode: LinuxExecution,
    relative: RelativeArgs,
) -> Result<keyweave_worker::linux_preparation::PreparedConfig, String> {
    check_linux_reader(
        std::fs::File::open(path).map_err(|e| e.to_string())?,
        mode,
        relative,
    )
}
fn check_linux_reader(
    reader: impl std::io::Read,
    mode: LinuxExecution,
    relative: RelativeArgs,
) -> Result<keyweave_worker::linux_preparation::PreparedConfig, String> {
    use keyweave_worker::linux_preparation::{CaptureScope, PreparedConfig, default_output_spec};
    let mut output = default_output_spec().map_err(|e| e.to_string())?;
    let scope = if let Some((axes, buttons)) = relative.capabilities()? {
        if !matches!(mode, LinuxExecution::StagedKeyboard) {
            return Err("relative scope requires --execution-mode staged-keyboard".into());
        }
        output = output.with_relative_axes(axes);
        if let Some(buttons) = buttons {
            output = output
                .with_pointing_buttons(buttons)
                .map_err(|e| e.to_string())?;
        }
        CaptureScope::StagedRelative
    } else {
        CaptureScope::Keyboard
    };
    PreparedConfig::read_for_generation_with_scope(
        reader,
        output,
        keyweave_types::ConfigGeneration(1),
        mode.runtime(),
        scope,
    )
    .map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn run_linux(
    _: PathBuf,
    _: Option<PathBuf>,
    _: PathBuf,
    _: Vec<PathBuf>,
    _: LinuxProvider,
    _: Option<PathBuf>,
    _: (WatchArgs, LinuxExecution, X11Args, Option<PathBuf>),
) -> ExitCode {
    eprintln!("run-linux requires Linux");
    ExitCode::from(78)
}
#[cfg(target_os = "linux")]
fn run_linux(
    config: PathBuf,
    worker: Option<PathBuf>,
    uinput: PathBuf,
    input: Vec<PathBuf>,
    provider: LinuxProvider,
    vt: Option<PathBuf>,
    (watch, execution_mode, x11, control_socket): (
        WatchArgs,
        LinuxExecution,
        X11Args,
        Option<PathBuf>,
    ),
) -> ExitCode {
    use keyweave_host::linux::{DriveStatus, LaunchOptions, PreparedLaunch, StartOutcome};
    let provider = match linux_provider(provider, vt, execution_mode, x11) {
        Ok(provider) => provider,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let host = match std::env::current_exe() {
        Ok(host) => host,
        Err(_) => {
            eprintln!("host executable location failed");
            return ExitCode::from(1);
        }
    };
    let options = LaunchOptions {
        execution_mode: execution_mode.runtime(),
        config,
        worker,
        uinput,
        inputs: input,
        session_provider: provider,
    };
    let preparation = if watch.watch_config {
        PreparedLaunch::prepare_watched(
            &host,
            options,
            watch.watch_interval_ms.unwrap_or(1000) * 1_000_000,
        )
        .map(|(launch, watcher)| (launch, Some(watcher)))
    } else {
        PreparedLaunch::prepare(&host, options).map(|launch| (launch, None))
    };
    let (prepared, mut watcher) = match preparation {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{e}; use check-linux for offline configuration diagnostics");
            return ExitCode::from(1);
        }
    };
    let mut control = match control_socket
        .as_ref()
        .map(|path| keyweave_linux::session_control::Server::bind(path))
        .transpose()
    {
        Ok(server) => server,
        Err(e) => {
            eprintln!("local control setup failed before worker start: {e}");
            return ExitCode::from(1);
        }
    };
    let outcome = match prepared.start() {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("{e}");
            if let Some(server) = control.as_mut()
                && let Err(error) = server.close()
            {
                eprintln!("control cleanup failed: {error}");
            }
            return ExitCode::from(1);
        }
    };
    match outcome {
        StartOutcome::Cancelled(cancelled) => {
            println!(
                "startup_cancelled=true, worker_exit=not_started, stop_signal={:?}, signal_close_failure={:?}",
                cancelled.signal, cancelled.signal_close_failure
            );
            let control_closed = control.as_mut().is_none_or(|server| match server.close() {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("control cleanup failed: {e}");
                    false
                }
            });
            ExitCode::from(
                if cancelled.signal_close_failure.is_none() && control_closed {
                    0
                } else {
                    1
                },
            )
        }
        StartOutcome::Running(mut native) => {
            // No file parsing, stdin thread or stdio on this control loop.
            let mut stats = LinuxWatchStats::default();
            let mut control_error = None;
            let status = loop {
                let status = if let Some(watcher) = watcher.as_mut() {
                    let result = watcher.poll(&mut native);
                    if let Some(event) = watcher.take_event() {
                        stats.observe(event);
                    }
                    match result {
                        Ok(status) => status,
                        Err(error) => {
                            stats.fault.get_or_insert(error);
                            // The watcher has closed and requested the original
                            // owner's stop; keep observing that same child.
                            continue;
                        }
                    }
                } else {
                    native.poll_once()
                };
                // Supervisor deadline/progress/actual-exit observation takes
                // priority over each bounded public IPC batch.
                if control_error.is_none()
                    && let Some(server) = control.as_mut()
                {
                    let result = server.poll(native.control_snapshot(server.session()));
                    if server.stop_requested() {
                        native.request_shutdown();
                    }
                    if let Err(e) = result {
                        control_error = Some(e.to_string());
                        native.fail_control(e);
                    }
                }
                if !matches!(status, DriveStatus::Pending(_)) {
                    break status;
                }
            };
            if let Some(server) = control.as_mut() {
                let mut snapshot = native.control_snapshot(server.session());
                if matches!(status, DriveStatus::ObservationExpired) {
                    snapshot.phase = keyweave_progress::session_control::Phase::Unobserved;
                    snapshot.exit = None;
                    snapshot.recovery_confirmed = false;
                    snapshot.fault = true;
                }
                // Actual exit is already observed before this bounded final-status
                // service window. Unobserved owners never wait here or fake exit.
                let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
                loop {
                    if control_error.is_none()
                        && let Err(e) = server.poll(snapshot)
                    {
                        control_error = Some(e.to_string());
                    }
                    if !matches!(status, DriveStatus::Exited(_))
                        || std::time::Instant::now() >= until
                    {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                if let Err(e) = server.close() {
                    control_error.get_or_insert_with(|| e.to_string());
                }
            }
            let source_reaped = watcher.as_mut().map(reap_linux_source);
            // Terminal metadata only, outside the active supervision loop.
            if watcher.is_some() {
                println!(
                    "{stats}, config_source_reaped={source_reaped:?}, config_notify_error={:?}",
                    watcher.as_ref().and_then(|w| w.notification_error())
                );
            }
            if let Some(error) = control_error.as_ref() {
                eprintln!("local control failed: {error}");
            }
            let watch_success =
                stats.fault.is_none() && source_reaped != Some(false) && control_error.is_none();
            match status {
                DriveStatus::Exited(report) => {
                    println!("{report}");
                    ExitCode::from(if report.successful() && watch_success {
                        0
                    } else {
                        1
                    })
                }
                DriveStatus::ObservationExpired => {
                    eprintln!(
                        "worker_exit=unobserved, recovery_confirmed=false, supervisor={:?}, host_fault={:?}",
                        native.status(),
                        native.fault()
                    );
                    // Emergency owner Drop runs before mask restoration. Its finite
                    // kill/reap fallback is not counted as an observed actual exit.
                    ExitCode::from(1)
                }
                DriveStatus::Pending(_) => {
                    eprintln!("worker_exit=unobserved, recovery_confirmed=false");
                    ExitCode::from(1)
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Default)]
struct LinuxWatchStats {
    submitted: u64,
    accepted: u64,
    rejected: u64,
    cancelled: u64,
    read_failed: u64,
    generation: Option<keyweave_types::ConfigGeneration>,
    fault: Option<keyweave_host::linux::watcher::WatchError>,
}
#[cfg(target_os = "linux")]
impl LinuxWatchStats {
    fn observe(&mut self, event: keyweave_host::linux::watcher::WatchEvent) {
        use keyweave_host::linux::watcher::WatchEvent;
        let counter = match event {
            WatchEvent::Submitted => &mut self.submitted,
            WatchEvent::Accepted(generation) => {
                self.generation = Some(generation);
                &mut self.accepted
            }
            WatchEvent::Rejected(generation) => {
                self.generation = Some(generation);
                &mut self.rejected
            }
            WatchEvent::Cancelled(generation) => {
                self.generation = Some(generation);
                &mut self.cancelled
            }
            WatchEvent::ReadFailed(_) => &mut self.read_failed,
            WatchEvent::Unchanged | WatchEvent::Closed => return,
        };
        *counter = counter.saturating_add(1);
    }
}
#[cfg(target_os = "linux")]
impl std::fmt::Display for LinuxWatchStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "config_watch=true, reload_submitted={}, reload_accepted={}, reload_rejected={}, reload_cancelled={}, config_read_failed={}, reload_confirmed_generation={:?}, config_watch_fault={:?}",
            self.submitted,
            self.accepted,
            self.rejected,
            self.cancelled,
            self.read_failed,
            self.generation.map(|g| g.0),
            self.fault
        )
    }
}
#[cfg(target_os = "linux")]
fn reap_linux_source(watcher: &mut keyweave_host::linux::watcher::ConfigWatcher) -> bool {
    use std::time::{Duration, Instant};
    watcher.close();
    let until = Instant::now() + Duration::from_millis(100);
    loop {
        match watcher.reap_closed() {
            Ok(true) => return true,
            Err(_) => return false,
            Ok(false) => {}
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::unwrap_used)]
mod x11_cli_tests {
    use super::*;
    use keyweave_worker::linux_arguments::{LinuxArguments, SessionProvider};

    fn parse(extra: &[&str]) -> Result<SessionProvider, String> {
        let mut argv = vec![
            "keyweave-host",
            "run-linux",
            "--config",
            "/cfg",
            "--input",
            "/dev/input/event1",
        ];
        argv.extend_from_slice(extra);
        let cli = Cli::try_parse_from(argv).map_err(|e| e.to_string())?;
        let Command::RunLinux {
            session_provider,
            vt,
            execution_mode,
            x11,
            ..
        } = cli.command
        else {
            unreachable!()
        };
        linux_provider(session_provider, vt, execution_mode, x11)
    }
    const X11: &[&str] = &[
        "--session-provider",
        "x11",
        "--execution-mode",
        "staged-keyboard",
        "--x11-probe",
        "/package/keyweave-x11-probe",
        "--relative-axes",
        "12,0,1,8,11,6",
    ];

    #[test]
    fn public_x11_parameters_roundtrip_to_canonical_worker_arguments() {
        let mut args = X11.to_vec();
        args.extend(["--pointing-buttons", "279,272"]);
        let provider = parse(&args).unwrap();
        let argv = LinuxArguments {
            parent: 42,
            run_id: 7,
            uinput: "/dev/uinput".into(),
            inputs: vec!["/dev/input/event1".into()],
            session_provider: provider.clone(),
            execution_mode: LinuxExecution::StagedKeyboard.runtime(),
        }
        .into_argv()
        .unwrap();
        let parsed = LinuxArguments::parse(argv).unwrap();
        assert_eq!(parsed.session_provider, provider);
        let SessionProvider::X11 { axes, buttons, .. } = parsed.session_provider else {
            panic!()
        };
        assert_eq!(axes.codes().collect::<Vec<_>>(), [0, 1, 6, 8, 11, 12]);
        assert_eq!(buttons.unwrap().codes().collect::<Vec<_>>(), [272, 279]);
    }
    #[test]
    fn x11_requires_explicit_scope_and_absolute_helper() {
        for extra in [
            vec!["--session-provider", "x11"],
            vec![
                "--session-provider",
                "x11",
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "0,1",
            ],
            vec![
                "--session-provider",
                "x11",
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "0,1",
                "--x11-probe",
                "relative",
            ],
        ] {
            assert!(parse(&extra).is_err());
        }
        let mut args = X11.to_vec();
        args.extend(["--vt", "/dev/tty1"]);
        assert!(parse(&args).is_err());
        let mut args = X11.to_vec();
        args[3] = "legacy";
        assert!(parse(&args).is_err());
    }
    #[test]
    fn invalid_axes_buttons_and_other_provider_options_are_rejected() {
        for axes in ["0,0", "2", "65536", "bad", "0,,1"] {
            let mut args = X11.to_vec();
            *args.last_mut().unwrap() = axes;
            assert!(parse(&args).is_err(), "{axes}");
        }
        for buttons in ["272,272", "271", "280", "bad"] {
            let mut args = X11.to_vec();
            args.extend(["--pointing-buttons", buttons]);
            assert!(parse(&args).is_err(), "{buttons}");
        }
        let mut args = X11.to_vec();
        *args.last_mut().unwrap() = "8";
        args.extend(["--pointing-buttons", "272"]);
        assert!(parse(&args).is_err());
        for extra in [
            vec!["--relative-axes", "0,1"],
            vec!["--pointing-buttons", "272"],
            vec!["--x11-probe", "/helper"],
        ] {
            let mut args = vec!["--session-provider", "none"];
            args.extend(extra);
            assert!(parse(&args).is_err());
        }
    }
    #[test]
    fn existing_providers_and_no_button_x11_remain_explicit() {
        assert_eq!(
            parse(&["--session-provider", "none"]).unwrap(),
            SessionProvider::None
        );
        assert!(matches!(
            parse(&["--session-provider", "console", "--vt", "/dev/tty1"]).unwrap(),
            SessionProvider::Console { .. }
        ));
        assert!(parse(&["--session-provider", "console"]).is_err());
        assert!(matches!(
            parse(X11).unwrap(),
            SessionProvider::X11 { buttons: None, .. }
        ));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod linux_offline_cli_tests {
    use super::*;
    use keyweave_worker::linux_preparation::CaptureScope;
    const KEYBOARD: &[u8] = br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
    const POINTER: &[u8] = br#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"pointing_button":"button1"},"to":[{"pointing_button":"button2"}]}]}]}}]}"#;
    fn check(
        extra: &[&str],
        bytes: &[u8],
    ) -> Result<keyweave_worker::linux_preparation::PreparedConfig, String> {
        let mut argv = vec!["keyweave-host", "check-linux", "--config", "unused.json"];
        argv.extend_from_slice(extra);
        let cli = Cli::try_parse_from(argv).map_err(|e| e.to_string())?;
        let Command::CheckLinux {
            execution_mode,
            relative,
            ..
        } = cli.command
        else {
            unreachable!()
        };
        check_linux_reader(bytes, execution_mode, relative)
    }
    #[test]
    fn explicit_mixed_check_compiles_actual_button_rule_and_preserves_bytes() {
        let config = check(
            &[
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "1,0",
                "--pointing-buttons",
                "273,272",
            ],
            POINTER,
        )
        .unwrap();
        assert_eq!(config.capture_scope(), CaptureScope::StagedRelative);
        assert_eq!(config.bytes(), POINTER);
        assert_eq!(
            config
                .output_spec()
                .relative_axes()
                .unwrap()
                .codes()
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            config
                .output_spec()
                .pointing_buttons()
                .unwrap()
                .codes()
                .collect::<Vec<_>>(),
            [272, 273]
        );
    }
    #[test]
    fn actual_pointer_outputs_require_declared_buttons_and_target_coverage() {
        for buttons in [None, Some("272"), Some("274")] {
            let mut args = vec![
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "0,1",
            ];
            if let Some(buttons) = buttons {
                args.extend(["--pointing-buttons", buttons]);
            }
            assert!(check(&args, POINTER).is_err());
        }
        assert!(check(&[], POINTER).is_err());
        assert!(check(&["--execution-mode", "staged-keyboard"], POINTER).is_err());
    }
    #[test]
    fn keyboard_defaults_and_explicit_relative_without_buttons_compile() {
        for args in [vec![], vec!["--execution-mode", "staged-keyboard"]] {
            let config = check(&args, KEYBOARD).unwrap();
            assert_eq!(config.capture_scope(), CaptureScope::Keyboard);
            assert!(config.output_spec().relative_axes().is_none());
        }
        let config = check(
            &[
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "8",
            ],
            KEYBOARD,
        )
        .unwrap();
        assert_eq!(config.capture_scope(), CaptureScope::StagedRelative);
        assert!(config.output_spec().pointing_buttons().is_none());
    }
    #[test]
    fn invalid_offline_shape_mode_and_execution_options_are_rejected() {
        for args in [
            vec!["--relative-axes", "0,1"],
            vec![
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "0,0",
            ],
            vec![
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "2",
            ],
            vec![
                "--execution-mode",
                "staged-keyboard",
                "--pointing-buttons",
                "272",
            ],
            vec![
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "8",
                "--pointing-buttons",
                "272",
            ],
            vec![
                "--execution-mode",
                "staged-keyboard",
                "--relative-axes",
                "0,1",
                "--pointing-buttons",
                "280",
            ],
            vec!["--x11-probe", "/helper"],
            vec!["--session-provider", "x11"],
        ] {
            assert!(check(&args, KEYBOARD).is_err(), "{args:?}");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod windows_mixed_cli_tests {
    use super::*;
    use keyweave_worker::{Arguments, preparation::CaptureScope};
    const KEYBOARD: &[u8] = br#"{"profiles":[{"selected":true}]}"#;
    const MIXED:&[u8]=br#"// retain bytes
{"profiles":[{"selected":true,"simple_modifications":[{"from":{"pointing_button":"button1"},"to":[{"pointing_button":"button2"}]}]}]}"#;
    fn check(extra: &[&str], bytes: &[u8]) -> Result<PreparedConfig, String> {
        let mut args = vec!["keyweave-host", "check-windows", "--config", "unused.json"];
        args.extend_from_slice(extra);
        let Command::CheckWindows {
            execution_mode,
            capture_scope,
            ..
        } = Cli::try_parse_from(args)
            .map_err(|e| e.to_string())?
            .command
        else {
            unreachable!()
        };
        prepare_windows_reader(bytes, execution_mode, capture_scope)
    }
    #[test]
    fn explicit_windows_mixed_cli_compiles_buttons_and_preserves_config_image() {
        let prepared = check(
            &[
                "--execution-mode",
                "staged-keyboard",
                "--capture-scope",
                "staged-mixed",
            ],
            MIXED,
        )
        .unwrap();
        assert_eq!(prepared.capture_scope(), CaptureScope::StagedMixed);
        assert_eq!(prepared.bytes(), MIXED);
        assert_eq!(
            prepared.snapshot().simple[0].from,
            keyweave_types::KeyId::new(keyweave_types::BUTTON_PAGE, 1)
        );
    }
    #[test]
    fn default_keyboard_and_mixed_mode_errors_are_explicit() {
        assert_eq!(
            check(&[], KEYBOARD).unwrap().capture_scope(),
            CaptureScope::Keyboard
        );
        assert_eq!(
            check(&["--execution-mode", "staged-keyboard"], KEYBOARD)
                .unwrap()
                .capture_scope(),
            CaptureScope::Keyboard
        );
        for args in [
            vec![],
            vec!["--execution-mode", "staged-keyboard"],
            vec!["--capture-scope", "staged-mixed"],
            vec!["--capture-scope", "mixed"],
        ] {
            assert!(check(&args, MIXED).is_err(), "{args:?}");
        }
        let bad = std::str::from_utf8(MIXED)
            .unwrap()
            .replace("button2", "button6");
        assert!(
            check(
                &[
                    "--execution-mode",
                    "staged-keyboard",
                    "--capture-scope",
                    "staged-mixed"
                ],
                bad.as_bytes()
            )
            .is_err()
        );
    }
    #[test]
    fn frozen_preparation_generates_matching_private_worker_scope() {
        for scope in ["keyboard", "staged-mixed"] {
            let prepared = check(
                &[
                    "--execution-mode",
                    "staged-keyboard",
                    "--capture-scope",
                    scope,
                ],
                KEYBOARD,
            )
            .unwrap();
            let path = std::env::temp_dir().join("diagnostic file with spaces.json");
            let extra = prepared
                .worker_arguments(
                    &path,
                    keyweave_windows::keys::InjectionTag::new(29).unwrap(),
                    IdentityPolicy::CanonicalScan,
                )
                .unwrap();
            let mut args = vec![
                keyweave_worker::WORKER_ARGUMENT.into(),
                "42".into(),
                "7".into(),
            ];
            args.extend(extra);
            args.extend(["--keyweave-config-handle".into(), "512".into()]);
            let worker = Arguments::parse(args).unwrap();
            assert_eq!(worker.config, path);
            assert_eq!(worker.capture_scope, prepared.capture_scope());
            assert_eq!(worker.execution_mode, prepared.execution_mode());
            assert_eq!(worker.config_handle, Some(512));
        }
    }
    #[test]
    fn run_windows_scope_selection_uses_the_same_cold_preparation_contract() {
        for scope in ["keyboard", "staged-mixed"] {
            let cli = Cli::try_parse_from([
                "keyweave-host",
                "run-windows",
                "--config",
                "unused.json",
                "--identity",
                "canonical-scan",
                "--execution-mode",
                "staged-keyboard",
                "--capture-scope",
                scope,
            ])
            .unwrap();
            let Command::RunWindows {
                execution_mode,
                capture_scope,
                ..
            } = cli.command
            else {
                unreachable!()
            };
            let prepared = prepare_windows_reader(KEYBOARD, execution_mode, capture_scope).unwrap();
            assert_eq!(prepared.capture_scope(), capture_scope.runtime());
        }
        let cli = Cli::try_parse_from([
            "keyweave-host",
            "run-windows",
            "--config",
            "unused.json",
            "--identity",
            "strict",
        ])
        .unwrap();
        let Command::RunWindows {
            execution_mode,
            capture_scope,
            ..
        } = cli.command
        else {
            unreachable!()
        };
        assert_eq!(
            prepare_windows_reader(KEYBOARD, execution_mode, capture_scope)
                .unwrap()
                .capture_scope(),
            CaptureScope::Keyboard
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn local_control(_: PathBuf, _: Option<u64>) -> ExitCode {
    eprintln!("local Linux control requires Linux");
    ExitCode::from(78)
}
#[cfg(target_os = "linux")]
fn local_control(socket: PathBuf, session: Option<u64>) -> ExitCode {
    use keyweave_progress::session_control::{Command, Outcome, Request};
    let request = Request {
        id: 1,
        command: session.map_or(Command::Status, |session| Command::Stop { session }),
    };
    let result = (|| {
        let mut client = keyweave_linux::session_control::Client::connect(
            &socket,
            request,
            std::time::Duration::from_secs(2),
        )?;
        loop {
            if let Some(reply) = client.poll()? {
                return Ok::<_, std::io::Error>(reply);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    })();
    match result {
        Ok(reply) => {
            let s = reply.snapshot;
            println!(
                "outcome={:?}, public_session={}, phase={:?}, progress_epoch={}, last_observed_generation={:?}, worker_exit={:?}, recovery_confirmed={}, host_fault={}",
                reply.outcome,
                s.session,
                s.phase,
                s.progress_epoch,
                s.generation,
                s.exit,
                s.recovery_confirmed,
                s.fault
            );
            ExitCode::from(if reply.outcome == Outcome::StaleSession {
                1
            } else {
                0
            })
        }
        Err(e) => {
            eprintln!("local control unavailable; worker exit/recovery unconfirmed: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod control_cli_tests {
    use super::*;
    #[test]
    fn public_control_cli_requires_explicit_socket_and_nonzero_expected_stop_session() {
        assert!(Cli::try_parse_from(["host", "status-linux"]).is_err());
        assert!(
            Cli::try_parse_from(["host", "stop-linux", "--socket", "/private/control"]).is_err()
        );
        assert!(
            Cli::try_parse_from([
                "host",
                "stop-linux",
                "--socket",
                "/private/control",
                "--session",
                "0"
            ])
            .is_err()
        );
        assert!(matches!(
            Cli::try_parse_from(["host", "status-linux", "--socket", "/private/control"])
                .unwrap()
                .command,
            Command::StatusLinux { .. }
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "host",
                "stop-linux",
                "--socket",
                "/private/control",
                "--session",
                "123"
            ])
            .unwrap()
            .command,
            Command::StopLinux { session: 123, .. }
        ));
    }
}

#[cfg(not(windows))]
fn windows_local_control(
    _: keyweave_host::public_control::WindowsEndpoint,
    _: Option<u64>,
) -> ExitCode {
    eprintln!("local Windows control requires Windows");
    ExitCode::from(78)
}
#[cfg(windows)]
fn windows_local_control(
    endpoint: keyweave_host::public_control::WindowsEndpoint,
    session: Option<u64>,
) -> ExitCode {
    use keyweave_progress::session_control::{Command, Outcome, Request};
    let request = Request {
        id: 1,
        command: session.map_or(Command::Status, |session| Command::Stop { session }),
    };
    match keyweave_host::windows_control::request(&endpoint, request) {
        Ok(reply) => {
            let s = reply.snapshot;
            println!(
                "outcome={:?}, public_session={}, phase={:?}, progress_epoch={}, last_observed_generation={:?}, worker_exit={:?}, recovery_confirmed={}, host_fault={}",
                reply.outcome,
                s.session,
                s.phase,
                s.progress_epoch,
                s.generation,
                s.exit,
                s.recovery_confirmed,
                s.fault
            );
            ExitCode::from(u8::from(reply.outcome == Outcome::StaleSession))
        }
        Err(error) => {
            eprintln!(
                "local control unavailable; request may have taken effect; worker exit/recovery unconfirmed: {error}"
            );
            ExitCode::from(1)
        }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod windows_control_cli_tests {
    use super::*;
    #[test]
    fn endpoint_and_positive_stop_session_are_required_without_worker_arguments() {
        let endpoint = r"\\.\pipe\keyweave-control-0123456789abcdef0123456789abcdef|42|123456";
        assert!(Cli::try_parse_from(["host", "status-windows"]).is_err());
        assert!(Cli::try_parse_from(["host", "status-windows", "--endpoint", "bad"]).is_err());
        assert!(matches!(
            Cli::try_parse_from(["host", "status-windows", "--endpoint", endpoint])
                .unwrap()
                .command,
            Command::StatusWindows { .. }
        ));
        assert!(
            Cli::try_parse_from([
                "host",
                "stop-windows",
                "--endpoint",
                endpoint,
                "--session",
                "0"
            ])
            .is_err()
        );
        assert!(matches!(
            Cli::try_parse_from([
                "host",
                "stop-windows",
                "--endpoint",
                endpoint,
                "--session",
                "77"
            ])
            .unwrap()
            .command,
            Command::StopWindows { session: 77, .. }
        ));
        let cli = Cli::try_parse_from([
            "host",
            "run-windows",
            "--config",
            "unused.json",
            "--identity",
            "strict",
            "--control-pipe",
            r"\\.\pipe\keyweave-control-0123456789abcdef0123456789abcdef",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::RunWindows {
                control_pipe: Some(_),
                ..
            }
        ));
    }
}

#[cfg(not(target_os = "linux"))]
fn list_linux_devices() -> ExitCode {
    eprintln!("Linux evdev metadata inventory requires Linux");
    ExitCode::from(78)
}
#[cfg(target_os = "linux")]
fn list_linux_devices() -> ExitCode {
    let report = keyweave_linux::inventory::scan();
    let complete = report.scan_complete && report.probes_complete;
    match serde_json::to_string_pretty(&report) {
        Ok(text) => {
            println!("{text}");
            ExitCode::from(if complete { 0 } else { 1 })
        }
        Err(error) => {
            eprintln!("inventory serialization failed: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod inventory_cli_tests {
    use super::*;
    #[test]
    fn metadata_inventory_is_a_standalone_command_without_capture_parameters() {
        assert!(matches!(
            Cli::try_parse_from(["host", "list-linux-devices"])
                .unwrap()
                .command,
            Command::ListLinuxDevices
        ));
        assert!(
            Cli::try_parse_from(["host", "list-linux-devices", "--input", "/dev/input/event0"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["host", "list-linux-devices", "--worker", "unused"]).is_err());
    }
}
