//! Four-handle Supervisor startup; preserves every successfully returned owner.
use super::super::config_artifact::{ArtifactCleanup, ArtifactError, ConfigArtifact};
use super::*;

/// Failed preparation only: no process is stored here. After Job handoff the
/// writer stays in the independent Job error owner, never in these resources.
#[derive(Default)]
pub struct SupervisorConfigResources {
    config: Option<Box<ConfigArtifact>>,
    reader: Option<PipeReader>,
    writer: Option<PipeWriter>,
    worker_reader: Option<PipeReader>,
    worker_writer: Option<PipeWriter>,
}
impl std::fmt::Debug for SupervisorConfigResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupervisorConfigResources")
            .field("config_retained", &self.config.is_some())
            .field("parent_reader_retained", &self.reader.is_some())
            .field("parent_writer_retained", &self.writer.is_some())
            .field("worker_reader_retained", &self.worker_reader.is_some())
            .field("worker_writer_retained", &self.worker_writer.is_some())
            .finish()
    }
}
#[derive(Debug, Default)]
pub struct SupervisorConfigCleanup {
    pub reader: Option<io::Result<PipeCloseReport>>,
    pub writer: Option<io::Result<PipeCloseReport>>,
    pub worker_reader: Option<io::Result<PipeCloseReport>>,
    pub worker_writer: Option<io::Result<PipeCloseReport>>,
    pub config: Option<Result<ArtifactCleanup, ArtifactError>>,
}
impl SupervisorConfigCleanup {
    pub fn complete(&self) -> bool {
        [
            &self.reader,
            &self.writer,
            &self.worker_reader,
            &self.worker_writer,
        ]
        .into_iter()
        .all(|result| {
            result
                .as_ref()
                .is_none_or(|r| r.as_ref().is_ok_and(PipeCloseReport::complete))
        }) && self
            .config
            .as_ref()
            .is_none_or(|r| r.as_ref().is_ok_and(|report| report.complete()))
    }
}
impl SupervisorConfigResources {
    /// Attempt all existing owners independently; retain failed resources for
    /// explicit retry. No child writer lives here after the Job transaction.
    pub fn close(&mut self) -> Result<SupervisorConfigCleanup, JobError> {
        if !super::super::outside_callback() {
            return Err(JobError::InCallback);
        }
        let report = SupervisorConfigCleanup {
            reader: self.reader.as_mut().map(PipeReader::close),
            writer: self.writer.as_mut().map(PipeWriter::close),
            worker_reader: self.worker_reader.as_mut().map(PipeReader::close),
            worker_writer: self.worker_writer.as_mut().map(PipeWriter::close),
            config: self.config.as_mut().map(|c| c.close()),
        };
        if report
            .reader
            .as_ref()
            .is_some_and(|r| r.as_ref().is_ok_and(PipeCloseReport::complete))
        {
            self.reader = None;
        }
        if report
            .writer
            .as_ref()
            .is_some_and(|r| r.as_ref().is_ok_and(PipeCloseReport::complete))
        {
            self.writer = None;
        }
        if report
            .worker_reader
            .as_ref()
            .is_some_and(|r| r.as_ref().is_ok_and(PipeCloseReport::complete))
        {
            self.worker_reader = None;
        }
        if report
            .worker_writer
            .as_ref()
            .is_some_and(|r| r.as_ref().is_ok_and(PipeCloseReport::complete))
        {
            self.worker_writer = None;
        }
        if report
            .config
            .as_ref()
            .is_some_and(|r| r.as_ref().is_ok_and(|report| report.complete()))
        {
            self.config = None;
        }
        Ok(report)
    }
    fn reject(self, cause: SupervisorSpawnError) -> SupervisorSpawnError {
        SupervisorSpawnError::Config {
            cause: Box::new(cause),
            resources: Box::new(self),
        }
    }
}

/// Actual exited Supervisor resource cleanup. Each outcome remains independent.
#[derive(Debug)]
pub struct SupervisorCleanup {
    pub channels: SupervisorChannelCleanup,
    pub config: Result<Option<ArtifactCleanup>, JobError>,
}
impl SupervisorCleanup {
    pub fn complete(&self) -> bool {
        self.channels.complete()
            && self
                .config
                .as_ref()
                .is_ok_and(|r| r.is_none_or(ArtifactCleanup::complete))
    }
}
impl Supervisor {
    /// Cold startup with already prepared bytes, never active-loop file IO.
    /// Trusted real-main worker must implement the four-handle contract.
    /// Failure retains the writer/pipe owners; resume errors retain Supervisor.
    pub fn spawn_with_config(
        path: &Path,
        args: &[OsString],
        run_id: u64,
        policy: Policy,
        now: MonoTime,
        config: ConfigArtifact,
    ) -> Result<Self, SupervisorSpawnError> {
        let mut resources = SupervisorConfigResources {
            config: Some(Box::new(config)),
            ..Default::default()
        };
        let (watchdog, request, command) =
            match prepare_command(path, args, run_id, policy, now, true) {
                Ok(prepared) => prepared,
                Err(cause) => return Err(resources.reject(cause)),
            };
        let (worker_reader, writer) = match PipeReader::pair() {
            Ok(pair) => pair,
            Err(cause) => return Err(resources.reject(SupervisorSpawnError::Io(cause))),
        };
        resources.worker_reader = Some(worker_reader);
        resources.writer = Some(writer);
        let (reader, worker_writer) = match PipeReader::pair() {
            Ok(pair) => pair,
            Err(cause) => return Err(resources.reject(SupervisorSpawnError::Io(cause))),
        };
        resources.reader = Some(reader);
        resources.worker_writer = Some(worker_writer);
        // Linear extraction only after all resources have been returned. The
        // private construction above establishes presence; no unwrap or clone.
        let ready = (
            resources.config.take(),
            resources.reader.take(),
            resources.writer.take(),
            resources.worker_reader.take(),
            resources.worker_writer.take(),
        );
        let (config, reader, writer, worker_reader, worker_writer) = match ready {
            (
                Some(config),
                Some(reader),
                Some(writer),
                Some(worker_reader),
                Some(worker_writer),
            ) => (config, reader, writer, worker_reader, worker_writer),
            (config, reader, writer, worker_reader, worker_writer) => {
                // Preserve partial owners even on a private construction bug;
                // reject before creating a child, never a resumable failed Job.
                resources.config = config;
                resources.reader = reader;
                resources.writer = writer;
                resources.worker_reader = worker_reader;
                resources.worker_writer = worker_writer;
                return Err(resources.reject(SupervisorSpawnError::Command(JobError::State)));
            }
        };
        let process = match OwnedJobProcess::spawn_suspended_with_config(
            path,
            &command,
            worker_reader,
            worker_writer,
            *config,
        ) {
            Ok(process) => process,
            Err(error) => {
                resources.reader = Some(reader);
                resources.writer = Some(writer);
                return Err(resources.reject(SupervisorSpawnError::Job(Box::new(error))));
            }
        };
        let mut owner = Self {
            core: Core::new(process, reader, writer, watchdog, request),
        };
        if let Err(cause) = owner.core.process.resume() {
            let stop = owner
                .core
                .stop(TerminationReason::Process(cause))
                .err()
                .map(|e| e.cause);
            return Err(SupervisorSpawnError::Resume {
                cause,
                stop,
                owner: Box::new(owner),
            });
        }
        Ok(owner)
    }
    /// Requires cached actual process exit. Attempts all outcomes, even when a
    /// pipe cancellation/close fails. Doesn't prove last-close deletion by itself.
    pub fn close_resources(&mut self) -> Result<SupervisorCleanup, JobError> {
        if !super::super::outside_callback() {
            return Err(JobError::InCallback);
        }
        if self.core.exit.is_none() {
            return Err(JobError::State);
        }
        Ok(SupervisorCleanup {
            channels: SupervisorChannelCleanup {
                reader: self.core.reader.close(),
                writer: self.core.writer.close(),
            },
            config: self.core.process.close_config(),
        })
    }
}
#[cfg(test)]
mod tests;
