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
        watch: WatchArgs,
    },
    /// Check Linux configuration against the full supported output table; no input access.
    CheckLinux {
        #[arg(long)]
        config: PathBuf,
    },
    /// Check a Windows configuration without opening input or starting a worker.
    CheckWindows {
        #[arg(long)]
        config: PathBuf,
    },
    /// Run an experimental Windows remapping session. Enter or Ctrl+C stops it.
    RunWindows {
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
    },
}
#[derive(clap::Args)]
struct WatchArgs {
    /// Reload changed configuration contents while the console session is active.
    #[arg(long)]
    watch_config: bool,
    /// File check interval (100..60000 milliseconds); default 1000 when enabled.
    #[arg(long, requires = "watch_config", value_parser = clap::value_parser!(u64).range(100..=60000))]
    watch_interval_ms: Option<u64>,
}
#[derive(Clone, Copy, ValueEnum)]
enum LinuxProvider {
    None,
    Console,
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
            config,
            worker,
            uinput,
            input,
            session_provider,
            vt,
            watch,
        } => run_linux(config, worker, uinput, input, session_provider, vt, watch),
        Command::CheckLinux { config } => match check_linux(config) {
            Ok(config) => {
                println!(
                    "Linux configuration accepted (offline): bytes={}, generation={}; device readiness unverified",
                    config.bytes().len(),
                    config.snapshot().generation.0
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("configuration rejected: {error}");
                ExitCode::from(1)
            }
        },
        Command::CheckWindows { config } => match std::fs::File::open(config)
            .map_err(|e| e.to_string())
            .and_then(|f| PreparedConfig::read(f).map_err(|e| e.to_string()))
        {
            Ok(config) => {
                println!(
                    "configuration accepted: bytes={}, generation={}",
                    config.bytes().len(),
                    config.snapshot().generation.0
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("configuration rejected: {error}");
                ExitCode::from(1)
            }
        },
        Command::RunWindows {
            config,
            identity,
            worker,
            artifact_directory,
        } => run(config, identity.into(), worker, artifact_directory),
    }
}
#[cfg(not(windows))]
fn run(_: PathBuf, _: IdentityPolicy, _: Option<PathBuf>, _: Option<PathBuf>) -> ExitCode {
    eprintln!("run-windows requires Windows");
    ExitCode::from(78)
}
#[cfg(windows)]
fn run(
    config: PathBuf,
    identity: IdentityPolicy,
    worker: Option<PathBuf>,
    directory: Option<PathBuf>,
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
        let prepared =
            PreparedConfig::read(std::fs::File::open(&config).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
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
    let result = keyweave_host::windows::run_with_console(
        &executable,
        &path,
        &directory,
        prepared,
        identity,
        &stop,
        &console,
    );
    // Restore default processing before final stdio; no active worker remains
    // on a successful result. Cleanup failure is independent and fails the host.
    let cleanup = console.close();
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
) -> Result<keyweave_worker::linux_preparation::PreparedConfig, String> {
    keyweave_worker::linux_preparation::PreparedConfig::read(
        std::fs::File::open(path).map_err(|e| e.to_string())?,
        keyweave_worker::linux_preparation::default_output_spec().map_err(|e| e.to_string())?,
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
    _: WatchArgs,
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
    watch: WatchArgs,
) -> ExitCode {
    use keyweave_host::linux::{DriveStatus, LaunchOptions, PreparedLaunch, StartOutcome};
    use keyweave_worker::linux_arguments::SessionProvider;
    let provider = match (provider, vt) {
        (LinuxProvider::None, None) => SessionProvider::None,
        (LinuxProvider::Console, Some(vt)) => SessionProvider::Console { vt },
        _ => {
            eprintln!("console requires --vt; none requires --vt to be omitted");
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
    let outcome = match prepared.start() {
        Ok(outcome) => outcome,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    match outcome {
        StartOutcome::Cancelled(cancelled) => {
            println!(
                "startup_cancelled=true, worker_exit=not_started, stop_signal={:?}, signal_close_failure={:?}",
                cancelled.signal, cancelled.signal_close_failure
            );
            ExitCode::from(if cancelled.signal_close_failure.is_none() {
                0
            } else {
                1
            })
        }
        StartOutcome::Running(mut native) => {
            // No file parsing, stdin thread or stdio on this control loop.
            let mut stats = LinuxWatchStats::default();
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
                if !matches!(status, DriveStatus::Pending(_)) {
                    break status;
                }
            };
            let source_reaped = watcher.as_mut().map(reap_linux_source);
            // Terminal metadata only, outside the active supervision loop.
            if watcher.is_some() {
                println!(
                    "{stats}, config_source_reaped={source_reaped:?}, config_notify_error={:?}",
                    watcher.as_ref().and_then(|w| w.notification_error())
                );
            }
            let watch_success = stats.fault.is_none() && source_reaped != Some(false);
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
