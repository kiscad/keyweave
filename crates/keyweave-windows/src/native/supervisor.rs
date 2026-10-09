//! Independent owned worker supervision. No input APIs; poll never waits.
use super::{
    job_process::{JobError, JobSpawnError, OwnedJobProcess},
    progress_pipe::{PipeCloseReport, PipeReader, PipeWriter},
};
use keyweave_progress::{
    CHALLENGE_BYTES, Policy, ProgressError, ProgressPhase, ProgressWatchdog, RECEIPT_BYTES,
    Receipt, StopRequest,
};
use keyweave_types::MonoTime;
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::Path,
    time::Duration,
};
pub const WORKER_ARGUMENT: &str = "--keyweave-worker";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelDirection {
    Challenge,
    Receipt,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelFailure {
    pub direction: ChannelDirection,
    pub kind: io::ErrorKind,
    pub code: Option<i32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminationReason {
    Progress(ProgressError),
    Channel(ChannelFailure),
    Process(JobError),
    Requested,
    Cooperative,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorStatus {
    Running,
    ShutdownRequested {
        deadline: MonoTime,
        request_sent: bool,
    },
    Stopping {
        reason: TerminationReason,
        stop_sent: bool,
    },
    Exited {
        code: u32,
        reason: Option<TerminationReason>,
    },
}
#[derive(Debug, thiserror::Error)]
#[error("supervisor process operation failed: {cause}; stop failure: {stop:?}")]
pub struct SupervisorError {
    pub cause: JobError,
    pub stop: Option<JobError>,
}
#[derive(Debug, thiserror::Error)]
pub enum SupervisorSpawnError {
    #[error(transparent)]
    Command(JobError),
    #[error(transparent)]
    Protocol(#[from] ProgressError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("Job startup failed: {0}")]
    Job(Box<JobSpawnError>),
    #[error(
        "worker resume failed: {cause}; stop failure: {stop:?}; owner retained for exit observation"
    )]
    Resume {
        cause: JobError,
        stop: Option<JobError>,
        owner: Box<Supervisor>,
    },
}
#[derive(Debug)]
pub struct SupervisorChannelCleanup {
    pub reader: io::Result<PipeCloseReport>,
    pub writer: io::Result<PipeCloseReport>,
}
impl SupervisorChannelCleanup {
    pub fn complete(&self) -> bool {
        self.reader.as_ref().is_ok_and(PipeCloseReport::complete)
            && self.writer.as_ref().is_ok_and(PipeCloseReport::complete)
    }
}
trait Process {
    fn poll_exit(&mut self) -> Result<Option<u32>, JobError>;
    fn stop(&mut self) -> Result<(), JobError>;
}
impl Process for OwnedJobProcess {
    fn poll_exit(&mut self) -> Result<Option<u32>, JobError> {
        self.poll_exit(Duration::ZERO)
    }
    fn stop(&mut self) -> Result<(), JobError> {
        self.request_stop()
    }
}
struct Core<P, R, W> {
    process: P,
    reader: R,
    writer: W,
    watchdog: ProgressWatchdog,
    request: [u8; CHALLENGE_BYTES],
    written: usize,
    response: [u8; RECEIPT_BYTES],
    read: usize,
    reason: Option<TerminationReason>,
    stop_sent: bool,
    exit: Option<u32>,
    channel_closed: bool,
    command_started: bool,
    run_id: u64,
    request_id: u64,
    stop_frame: bool,
    shutdown: Option<CooperativeShutdown>,
}
struct CooperativeShutdown {
    id: u64,
    deadline: MonoTime,
    now: MonoTime,
}
const SHUTDOWN_NS: u64 = 2_000_000_000;
impl<P: Process, R: Read, W: Write> Core<P, R, W> {
    fn new(
        process: P,
        reader: R,
        writer: W,
        watchdog: ProgressWatchdog,
        request: [u8; CHALLENGE_BYTES],
    ) -> Self {
        let initial = watchdog.challenge();
        Self {
            process,
            reader,
            writer,
            watchdog,
            request,
            written: 0,
            response: [0; RECEIPT_BYTES],
            read: 0,
            reason: None,
            stop_sent: false,
            exit: None,
            channel_closed: false,
            command_started: false,
            run_id: initial.map_or(0, |c| c.run_id),
            request_id: initial.map_or(0, |c| c.id),
            stop_frame: false,
            shutdown: None,
        }
    }
    fn status(&self) -> SupervisorStatus {
        if let Some(code) = self.exit {
            SupervisorStatus::Exited {
                code,
                reason: self.reason.or(self
                    .shutdown
                    .as_ref()
                    .map(|_| TerminationReason::Cooperative)),
            }
        } else if let Some(reason) = self.reason {
            SupervisorStatus::Stopping {
                reason,
                stop_sent: self.stop_sent,
            }
        } else if let Some(shutdown) = &self.shutdown {
            SupervisorStatus::ShutdownRequested {
                deadline: shutdown.deadline,
                request_sent: self.stop_frame && self.written == CHALLENGE_BYTES,
            }
        } else {
            SupervisorStatus::Running
        }
    }
    fn stop(&mut self, reason: TerminationReason) -> Result<SupervisorStatus, SupervisorError> {
        if self.exit.is_some() {
            return Ok(self.status());
        }
        self.reason.get_or_insert(reason);
        if !self.stop_sent {
            self.process
                .stop()
                .map_err(|cause| SupervisorError { cause, stop: None })?;
            self.stop_sent = true;
        }
        Ok(self.status())
    }
    fn channel(
        &mut self,
        direction: ChannelDirection,
        error: io::Error,
    ) -> Result<SupervisorStatus, SupervisorError> {
        self.stop(TerminationReason::Channel(ChannelFailure {
            direction,
            kind: error.kind(),
            code: error.raw_os_error(),
        }))
    }
    fn request_shutdown(&mut self, now: MonoTime) -> Result<SupervisorStatus, SupervisorError> {
        if self.exit.is_some() {
            return Ok(self.status());
        }
        if let Some(reason) = self.reason {
            return self.stop(reason);
        }
        if self.shutdown.is_some() {
            return self.check_shutdown(now);
        }
        if let Err(e) = self.watchdog.check(now) {
            return self.stop(TerminationReason::Progress(e));
        }
        let untouched = !self.command_started && self.written == 0;
        let id = if untouched {
            self.request_id
        } else {
            let Some(id) = self.request_id.checked_add(1) else {
                return self.stop(TerminationReason::Progress(ProgressError::Exhausted));
            };
            id
        };
        self.shutdown = Some(CooperativeShutdown {
            id,
            deadline: self
                .watchdog
                .deadline()
                .min(MonoTime(now.0.saturating_add(SHUTDOWN_NS))),
            now,
        });
        if untouched || self.written == CHALLENGE_BYTES {
            self.queue_stop();
        }
        Ok(self.status())
    }
    fn queue_stop(&mut self) {
        if let Some(shutdown) = &self.shutdown {
            self.request = StopRequest {
                run_id: self.run_id,
                id: shutdown.id,
            }
            .encode();
            self.written = 0;
            self.command_started = false;
            self.stop_frame = true;
        }
    }
    fn check_shutdown(&mut self, now: MonoTime) -> Result<SupervisorStatus, SupervisorError> {
        let Some(shutdown) = &mut self.shutdown else {
            return Ok(self.status());
        };
        let error = if now < shutdown.now {
            Some(ProgressError::Clock)
        } else if now >= shutdown.deadline {
            Some(ProgressError::Expired)
        } else {
            None
        };
        if now >= shutdown.now {
            shutdown.now = now;
        }
        if let Some(e) = error {
            return self.stop(TerminationReason::Progress(e));
        }
        Ok(self.status())
    }
    fn poll_shutdown(&mut self, now: MonoTime) -> Result<SupervisorStatus, SupervisorError> {
        self.check_shutdown(now)?;
        if let Some(reason) = self.reason {
            return self.stop(reason);
        }
        if self.written < CHALLENGE_BYTES {
            self.command_started = true;
            match self.writer.write(&self.request[self.written..]) {
                Ok(0) => {
                    return self
                        .channel(ChannelDirection::Challenge, io::ErrorKind::WriteZero.into());
                }
                Ok(n) if n > CHALLENGE_BYTES - self.written => {
                    return self.stop(TerminationReason::Progress(ProgressError::Frame));
                }
                Ok(n) => self.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) => return self.channel(ChannelDirection::Challenge, e),
            }
        }
        // Keep a pending overlapped write's shadow; finish its original frame
        // before queuing Stop for the next bounded write. Never renew/read.
        if !self.stop_frame && self.written == CHALLENGE_BYTES {
            self.queue_stop();
        }
        Ok(self.status())
    }
    fn poll(&mut self, now: MonoTime) -> Result<SupervisorStatus, SupervisorError> {
        if self.exit.is_some() {
            return Ok(self.status());
        }
        match self.process.poll_exit() {
            Ok(Some(code)) => {
                self.exit = Some(code);
                return Ok(self.status());
            }
            Ok(None) => (),
            Err(cause) => {
                let stop = self
                    .stop(TerminationReason::Process(cause))
                    .err()
                    .map(|e| e.cause);
                return Err(SupervisorError { cause, stop });
            }
        }
        if let Some(reason) = self.reason {
            return self.stop(reason);
        }
        if self.shutdown.is_some() {
            return self.poll_shutdown(now);
        }
        if let Err(error) = self.watchdog.check(now) {
            return self.stop(TerminationReason::Progress(error));
        }
        if self.watchdog.challenge().is_some() && self.written < CHALLENGE_BYTES {
            self.command_started = true;
            match self.writer.write(&self.request[self.written..]) {
                Ok(0) => {
                    return self
                        .channel(ChannelDirection::Challenge, io::ErrorKind::WriteZero.into());
                }
                Ok(n) if n > CHALLENGE_BYTES - self.written => {
                    return self.stop(TerminationReason::Progress(ProgressError::Frame));
                }
                Ok(n) => self.written += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
                Err(e) => return self.channel(ChannelDirection::Challenge, e),
            }
        }
        // An overlapped write can return WouldBlock even after the worker has
        // read its bytes and started replying. Do not read/validate the receipt
        // until the challenge write has been acknowledged in full; never renew
        // the lease from an unacknowledged flight or change its write shadow.
        if self.written < CHALLENGE_BYTES || self.channel_closed {
            return Ok(self.status());
        }
        match self.reader.read(&mut self.response[self.read..]) {
            Ok(0) => {
                if self.read == 0 && self.watchdog.phase() == Some(ProgressPhase::Stopped) {
                    self.channel_closed = true;
                    return Ok(self.status());
                }
                return self.channel(
                    ChannelDirection::Receipt,
                    io::ErrorKind::UnexpectedEof.into(),
                );
            }
            Ok(n) if n > RECEIPT_BYTES - self.read => {
                return self.stop(TerminationReason::Progress(ProgressError::Frame));
            }
            Ok(n) => self.read += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(self.status()),
            Err(e) => return self.channel(ChannelDirection::Receipt, e),
        }
        if self.read == RECEIPT_BYTES {
            let receipt = match Receipt::decode(&self.response) {
                Ok(v) => v,
                Err(e) => return self.stop(TerminationReason::Progress(e)),
            };
            if let Err(e) = self.watchdog.observe(receipt, now) {
                return self.stop(TerminationReason::Progress(e));
            }
            self.read = 0;
            if let Some(challenge) = self.watchdog.challenge() {
                self.request = challenge.encode();
                self.written = 0;
                self.command_started = false;
                self.request_id = challenge.id;
            }
        }
        Ok(self.status())
    }
}
pub struct Supervisor {
    core: Core<OwnedJobProcess, PipeReader, PipeWriter>,
}
impl std::fmt::Debug for Supervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Supervisor")
            .field("status", &self.status())
            .field("epoch", &self.progress_epoch())
            .finish_non_exhaustive()
    }
}
fn context() -> Result<(), SupervisorError> {
    if super::outside_callback() {
        Ok(())
    } else {
        Err(SupervisorError {
            cause: JobError::InCallback,
            stop: None,
        })
    }
}
impl Supervisor {
    /// Trusted executable must implement WORKER_ARGUMENT parent/run admission on
    /// its real main, before input/threads/stdio. No shell or arbitrary PID owner.
    /// The creating control thread retains this !Send/!Sync owner until exit.
    pub fn spawn(
        path: &Path,
        args: &[OsString],
        run_id: u64,
        policy: Policy,
        now: MonoTime,
    ) -> Result<Self, SupervisorSpawnError> {
        if !super::outside_callback() {
            return Err(SupervisorSpawnError::Io(io::Error::other(
                "supervisor startup forbidden in keyboard callback",
            )));
        }
        let watchdog = ProgressWatchdog::new(run_id, policy, now)?;
        let request = watchdog
            .challenge()
            .ok_or(ProgressError::Identity)?
            .encode();
        // Validate bounded original arguments before cloning any OsString.
        // The fixed three startup arguments share the existing 128-argument cap.
        if args.len() > 125 {
            return Err(SupervisorSpawnError::Command(JobError::Command));
        }
        super::job_process::command(path, args).map_err(SupervisorSpawnError::Command)?;
        let mut command = vec![
            WORKER_ARGUMENT.into(),
            std::process::id().to_string().into(),
            run_id.to_string().into(),
        ];
        command.extend_from_slice(args);
        super::job_process::command(path, &command).map_err(SupervisorSpawnError::Command)?;
        let (worker_reader, writer) = PipeReader::pair()?;
        let (reader, worker_writer) = PipeReader::pair()?;
        let process = OwnedJobProcess::spawn_suspended_with_progress(
            path,
            &command,
            worker_reader,
            worker_writer,
        )
        .map_err(|e| SupervisorSpawnError::Job(Box::new(e)))?;
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
    pub fn status(&self) -> SupervisorStatus {
        self.core.status()
    }
    pub fn progress_epoch(&self) -> u64 {
        self.core.watchdog.epoch()
    }
    pub fn deadline(&self) -> MonoTime {
        self.core
            .shutdown
            .as_ref()
            .map_or(self.core.watchdog.deadline(), |s| s.deadline)
    }
    /// One process poll with timeout=0, <=one read/write each; no sleep/wait loop.
    pub fn poll(&mut self, now: MonoTime) -> Result<SupervisorStatus, SupervisorError> {
        context()?;
        self.core.poll(now)
    }
    pub fn request_stop(&mut self) -> Result<SupervisorStatus, SupervisorError> {
        context()?;
        self.core.stop(TerminationReason::Requested)
    }
    /// Cooperatively close capture/recover in a known control-capable worker.
    /// Deadline is min(current lease, now+2s), never a progress lease renewal.
    pub fn request_shutdown(&mut self, now: MonoTime) -> Result<SupervisorStatus, SupervisorError> {
        context()?;
        self.core.request_shutdown(now)
    }
    /// Exit must have been observed; closing channels cannot manufacture exit.
    /// Both results are retained, including cancellation/close failures.
    pub fn close_channels(&mut self) -> Result<SupervisorChannelCleanup, JobError> {
        if !super::outside_callback() {
            return Err(JobError::InCallback);
        }
        if self.core.exit.is_none() {
            return Err(JobError::State);
        }
        Ok(SupervisorChannelCleanup {
            reader: self.core.reader.close(),
            writer: self.core.writer.close(),
        })
    }
}
#[cfg(test)]
mod tests;
