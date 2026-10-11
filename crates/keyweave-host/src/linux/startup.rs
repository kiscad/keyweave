//! Cold Linux launch transaction. No device probing or automatic activation.
use super::{NativeSession, Session, watcher::ConfigWatcher};
use keyweave_linux::{
    config_artifact::ConfigArtifact,
    host_signals::{HostSignals, StopSignal},
    progress::Policy,
    reactor::MonotonicClock,
    run_identity::RunIdentity,
    supervisor::Supervisor,
    termination::bounded::SystemFailure,
};
use keyweave_worker::{
    linux_arguments::{LinuxArguments, SessionProvider},
    linux_preparation::{PreparedConfig, default_output_spec},
};
use std::{
    ffi::OsString,
    fmt, io,
    path::{Path, PathBuf},
    process::Command,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupStage {
    Arguments,
    WorkerLocation,
    Configuration,
    Watcher,
    RunIdentity,
    Sealing,
    Signals,
    Clock,
    ChildMask,
    Spawn,
}
/// Deliberately excludes private config text, filenames and random run identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StartupError {
    pub stage: StartupStage,
    pub system: Option<SystemFailure>,
    pub signal_close_failure: Option<SystemFailure>,
}
impl StartupError {
    fn plain(stage: StartupStage) -> Self {
        Self {
            stage,
            system: None,
            signal_close_failure: None,
        }
    }
    fn system(stage: StartupStage, error: io::Error) -> Self {
        Self {
            stage,
            system: Some(error.into()),
            signal_close_failure: None,
        }
    }
    fn close(mut self, signals: HostSignals) -> Self {
        self.signal_close_failure = signals.close().err().map(Into::into);
        self
    }
}
impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Linux startup failed: stage={:?}, system={:?}, signal_close_failure={:?}",
            self.stage, self.system, self.signal_close_failure
        )
    }
}
impl std::error::Error for StartupError {}
pub struct LaunchOptions {
    pub config: PathBuf,
    pub worker: Option<PathBuf>,
    pub uinput: PathBuf,
    pub inputs: Vec<PathBuf>,
    pub session_provider: SessionProvider,
}
/// Consumed once by start; no Clone, no config pathname reopen after preparation.
pub struct PreparedLaunch {
    executable: PathBuf,
    argv: Vec<OsString>,
    artifact: ConfigArtifact,
    identity: RunIdentity,
    reload: bool,
}
#[derive(Debug)]
pub struct Cancelled {
    pub signal: StopSignal,
    pub signal_close_failure: Option<SystemFailure>,
}
pub enum StartOutcome {
    Cancelled(Cancelled),
    Running(Box<NativeSession>),
}
impl PreparedLaunch {
    pub fn prepare(host: &Path, options: LaunchOptions) -> Result<Self, StartupError> {
        Self::prepare_inner(host, options, None).map(|(launch, _)| launch)
    }
    /// Cold paired preparation. Keep watcher with the session created by this
    /// launch, and start polling it only after start has returned Running.
    /// Console is required; None remains permanently without reload/devices.
    pub fn prepare_watched(
        host: &Path,
        options: LaunchOptions,
        interval_ns: u64,
    ) -> Result<(Self, ConfigWatcher), StartupError> {
        let (launch, watcher) = Self::prepare_inner(host, options, Some(interval_ns))?;
        watcher
            .map(|watcher| (launch, watcher))
            .ok_or_else(|| StartupError::plain(StartupStage::Watcher))
    }
    fn prepare_inner(
        host: &Path,
        options: LaunchOptions,
        watch_interval: Option<u64>,
    ) -> Result<(Self, Option<ConfigWatcher>), StartupError> {
        if let Some(interval) = watch_interval
            && (!matches!(options.session_provider, SessionProvider::Console { .. })
                || !(100_000_000..=60_000_000_000).contains(&interval))
        {
            return Err(StartupError::plain(StartupStage::Arguments));
        }
        // Bound before cloning any device path array. Validation here is only
        // lexical; it never queries /dev or grants consumer/neutral readiness.
        if options.inputs.is_empty() || options.inputs.len() > keyweave_linux::runtime::MAX_DEVICES
        {
            return Err(StartupError::plain(StartupStage::Arguments));
        }
        LinuxArguments {
            parent: std::process::id(),
            run_id: 1,
            uinput: options.uinput.clone(),
            inputs: options.inputs.clone(),
            session_provider: options.session_provider.clone(),
        }
        .into_argv()
        .map_err(|_| StartupError::plain(StartupStage::Arguments))?;
        let executable = crate::locate_linux_worker(host, options.worker.as_deref())
            .map_err(|e| StartupError::system(StartupStage::WorkerLocation, e))?;
        let source_path = if watch_interval.is_some() {
            Some(
                std::path::absolute(&options.config)
                    .map_err(|e| StartupError::system(StartupStage::Watcher, e))?,
            )
        } else {
            None
        };
        let file = std::fs::File::open(source_path.as_deref().unwrap_or(&options.config))
            .map_err(|e| StartupError::system(StartupStage::Configuration, e))?;
        if !file
            .metadata()
            .map_err(|e| StartupError::system(StartupStage::Configuration, e))?
            .is_file()
        {
            return Err(StartupError::plain(StartupStage::Configuration));
        }
        let output = default_output_spec()
            .map_err(|e| StartupError::system(StartupStage::Configuration, e))?;
        let prepared = PreparedConfig::read(file, output)
            .map_err(|_| StartupError::plain(StartupStage::Configuration))?;
        // Absolute lexical source path preserves symlink identity for later
        // replacements. No canonical target cache and no second config read.
        let watcher = if let Some(interval) = watch_interval {
            let path = source_path.ok_or_else(|| StartupError::plain(StartupStage::Watcher))?;
            Some(
                ConfigWatcher::new(path, std::sync::Arc::from(prepared.bytes()), interval)
                    .map_err(|_| StartupError::plain(StartupStage::Watcher))?,
            )
        } else {
            None
        };
        let identity = RunIdentity::generate()
            .map_err(|e| StartupError::system(StartupStage::RunIdentity, e))?;
        let reload = matches!(options.session_provider, SessionProvider::Console { .. });
        let argv = LinuxArguments {
            parent: std::process::id(),
            run_id: identity.run_id(),
            uinput: options.uinput,
            inputs: options.inputs,
            session_provider: options.session_provider,
        }
        .into_argv()
        .map_err(|_| StartupError::plain(StartupStage::Arguments))?;
        let artifact = prepared
            .seal()
            .map_err(|e| StartupError::system(StartupStage::Sealing, e))?;
        Ok((
            Self {
                executable,
                argv,
                artifact,
                identity,
                reload,
            },
            watcher,
        ))
    }
    /// Actual main thread only. Installs control before spawn, restores child
    /// mask and uses the explicit isolated sealed-config Supervisor mode.
    pub fn start(self) -> Result<StartOutcome, StartupError> {
        let Self {
            executable,
            argv,
            artifact,
            identity,
            reload,
        } = self;
        let mut signals =
            HostSignals::install().map_err(|e| StartupError::system(StartupStage::Signals, e))?;
        match signals.poll() {
            Ok(Some(signal)) => {
                drop(artifact);
                return Ok(StartOutcome::Cancelled(Cancelled {
                    signal,
                    signal_close_failure: signals.close().err().map(Into::into),
                }));
            }
            Ok(None) => {}
            Err(e) => return Err(StartupError::system(StartupStage::Signals, e).close(signals)),
        }
        let mut command = Command::new(executable);
        command.args(argv);
        if let Err(e) = signals.prepare_child(&mut command) {
            return Err(StartupError::system(StartupStage::ChildMask, e).close(signals));
        }
        let mut clock = MonotonicClock::new();
        let now = match clock.now() {
            Ok(now) => now,
            Err(e) => return Err(StartupError::system(StartupStage::Clock, e).close(signals)),
        };
        // Console mode uses the default <2s challenge policy and retains the
        // independent reload endpoints inside the same Supervisor owner.
        let spawn = if reload {
            Supervisor::spawn_reporting_config_reload
        } else {
            Supervisor::spawn_reporting_config_isolated
        };
        let supervisor = match spawn(command, identity.run_id(), Policy::default(), now, artifact) {
            Ok(supervisor) => supervisor,
            Err(e) => return Err(StartupError::system(StartupStage::Spawn, e).close(signals)),
        };
        Ok(StartOutcome::Running(Box::new(NativeSession::new(
            Session::new(supervisor),
            signals,
            clock,
        ))))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    const CONFIG: &[u8] = br#"// private source
{"profiles":[{"name":"private profile","selected":true,"simple_modifications":[]}]}"#;
    struct Fixture {
        root: PathBuf,
        host: PathBuf,
        config: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "keyweave-cold-launch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let host = root.join("host");
            fs::write(&host, b"nonexecuted fixture").unwrap();
            let worker = root.join("keyweave-worker");
            fs::write(&worker, b"nonexecuted fixture").unwrap();
            fs::set_permissions(worker, fs::Permissions::from_mode(0o700)).unwrap();
            let config = root.join("config with spaces.jsonc");
            fs::write(&config, CONFIG).unwrap();
            Self { root, host, config }
        }
        fn options(&self) -> LaunchOptions {
            LaunchOptions {
                config: self.config.clone(),
                worker: None,
                uinput: "/absent-output".into(),
                inputs: vec!["/absent-input".into()],
                session_provider: SessionProvider::None,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn preparation_seals_exact_config_and_binds_validated_native_argv() {
        let f = Fixture::new();
        let p = PreparedLaunch::prepare(&f.host, f.options()).unwrap();
        fs::write(&f.config, b"replacement body").unwrap();
        assert_eq!(p.artifact.read_bytes().unwrap(), CONFIG);
        let args = LinuxArguments::parse(p.argv.clone()).unwrap();
        assert_eq!(args.run_id, p.identity.run_id());
        assert_eq!(args.parent, std::process::id());
        assert_eq!(args.session_provider, SessionProvider::None);
        assert_eq!(args.inputs.len(), 1);
        assert_eq!(
            p.executable,
            f.root.join("keyweave-worker").canonicalize().unwrap()
        );
    }
    #[test]
    fn invalid_device_arguments_fail_before_worker_or_config_lookup() {
        let f = Fixture::new();
        fs::remove_file(&f.config).unwrap();
        fs::remove_file(f.root.join("keyweave-worker")).unwrap();
        let mut options = f.options();
        options.inputs = vec!["relative".into()];
        let error = PreparedLaunch::prepare(&f.host, options).err().unwrap();
        assert_eq!(error.stage, StartupStage::Arguments);
        let mut options = f.options();
        options.inputs = vec!["/absent-input".into(); 65];
        assert_eq!(
            PreparedLaunch::prepare(&f.host, options)
                .err()
                .unwrap()
                .stage,
            StartupStage::Arguments
        );
        let mut options = f.options();
        options.session_provider = SessionProvider::Console {
            vt: "/dev/tty01".into(),
        };
        assert_eq!(
            PreparedLaunch::prepare(&f.host, options)
                .err()
                .unwrap()
                .stage,
            StartupStage::Arguments
        );
    }
    #[test]
    fn config_failure_excludes_private_body_and_rejects_directory() {
        let f = Fixture::new();
        fs::write(&f.config, b"private-secret\xff").unwrap();
        let error = PreparedLaunch::prepare(&f.host, f.options()).err().unwrap();
        assert_eq!(error.stage, StartupStage::Configuration);
        assert!(!error.to_string().contains("private-secret"));
        assert!(!error.to_string().contains("config with spaces"));
        let mut options = f.options();
        options.config = f.root.clone();
        assert_eq!(
            PreparedLaunch::prepare(&f.host, options)
                .err()
                .unwrap()
                .stage,
            StartupStage::Configuration
        );
    }
    #[test]
    fn watched_preparation_rejects_none_or_interval_before_image_and_file_lookup() {
        let f = Fixture::new();
        fs::remove_file(&f.config).unwrap();
        fs::remove_file(f.root.join("keyweave-worker")).unwrap();
        assert_eq!(
            PreparedLaunch::prepare_watched(&f.host, f.options(), 1_000_000_000)
                .err()
                .unwrap()
                .stage,
            StartupStage::Arguments
        );
        for interval in [99_999_999, 60_000_000_001] {
            let mut options = f.options();
            options.session_provider = SessionProvider::Console {
                vt: "/dev/tty2".into(),
            };
            assert_eq!(
                PreparedLaunch::prepare_watched(&f.host, options, interval)
                    .err()
                    .unwrap()
                    .stage,
                StartupStage::Arguments
            );
        }
    }
    #[test]
    fn watched_preparation_is_cold_and_seals_initial_source_through_symlink() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let link = f.root.join("selected.json");
        symlink(&f.config, &link).unwrap();
        let mut options = f.options();
        options.config = link.clone();
        options.session_provider = SessionProvider::Console {
            vt: "/dev/tty2".into(),
        };
        let (prepared, mut watcher) =
            PreparedLaunch::prepare_watched(&f.host, options, 100_000_000).unwrap();
        // Preparing does not open a VT/device, install signals or start a job.
        watcher.close();
        assert!(watcher.reap_closed().unwrap());
        fs::write(&f.config, b"new invalid private body").unwrap();
        fs::remove_file(link).unwrap();
        assert_eq!(prepared.artifact.read_bytes().unwrap(), CONFIG);
        assert!(prepared.reload);
        let args = LinuxArguments::parse(prepared.argv).unwrap();
        assert_eq!(
            args.session_provider,
            SessionProvider::Console {
                vt: "/dev/tty2".into()
            }
        );
    }
}
