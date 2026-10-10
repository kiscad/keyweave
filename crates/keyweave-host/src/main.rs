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
    eprintln!("run-windows requires Windows; Linux capture bootstrap is not yet connected");
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
