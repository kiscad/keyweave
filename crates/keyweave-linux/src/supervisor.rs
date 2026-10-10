//! Linux process supervisor; only its own spawned child can be terminated.
use crate::{
    progress::{CHALLENGE_BYTES, Policy, ProgressError, ProgressWatchdog, RECEIPT_BYTES, Receipt},
    runtime::RuntimePhase,
};
use keyweave_types::MonoTime;
use std::{
    io::{self, PipeReader, PipeWriter, Read, Write},
    os::fd::AsFd,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminationReason {
    Progress(ProgressError),
    Channel,
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
    KillRequested(TerminationReason),
    Exited {
        status: ExitStatus,
        reason: Option<TerminationReason>,
    },
}
const SHUTDOWN_NS: u64 = 2_000_000_000;
struct CooperativeShutdown {
    id: u64,
    deadline: MonoTime,
    now: MonoTime,
}
pub struct Supervisor {
    child: Child,
    reader: PipeReader,
    writer: PipeWriter,
    watchdog: ProgressWatchdog,
    request: [u8; CHALLENGE_BYTES],
    written: usize,
    response: [u8; RECEIPT_BYTES],
    read: usize,
    reason: Option<TerminationReason>,
    kill_sent: bool,
    exit: Option<ExitStatus>,
    channel_closed: bool,
    run_id: u64,
    command_started: bool,
    stop_frame: bool,
    shutdown: Option<CooperativeShutdown>,
    terminal: Option<crate::termination::TerminalReader<PipeReader>>,
}
impl Supervisor {
    /// Command must be a fixed trusted worker, with no inherited device FDs.
    /// Its stdin/stderr are replaced by a private nonblocking binary channel.
    /// The creating thread must live until the worker exits: Linux parent-death
    /// SIGKILL is installed before exec. Worker main must verify its inherited
    /// guard before opening devices; credential-changing exec is unsupported.
    pub fn spawn(command: Command, run_id: u64, policy: Policy, now: MonoTime) -> io::Result<Self> {
        Self::spawn_mode(command, run_id, policy, now, false)
    }
    /// Explicit third private terminal pipe. Only a reporting worker may use it.
    pub fn spawn_reporting(
        command: Command,
        run_id: u64,
        policy: Policy,
        now: MonoTime,
    ) -> io::Result<Self> {
        Self::spawn_mode(command, run_id, policy, now, true)
    }
    fn spawn_mode(
        mut command: Command,
        run_id: u64,
        policy: Policy,
        now: MonoTime,
        reporting: bool,
    ) -> io::Result<Self> {
        let watchdog = ProgressWatchdog::new(run_id, policy, now)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let request = watchdog
            .challenge()
            .ok_or_else(|| io::Error::other("missing initial challenge"))?
            .encode();
        let (worker_reader, writer) = io::pipe()?;
        let (reader, worker_writer) = io::pipe()?;
        for fd in [
            worker_reader.as_fd(),
            writer.as_fd(),
            reader.as_fd(),
            worker_writer.as_fd(),
        ] {
            crate::ioctl::nonblocking(fd)?;
        }
        command
            .stdin(Stdio::from(worker_reader))
            .stderr(Stdio::from(worker_writer));
        command.env(crate::worker::RUN_ENV, run_id.to_string());
        let terminal = if reporting {
            let (reader, writer) = io::pipe()?;
            crate::ioctl::nonblocking(reader.as_fd())?;
            crate::ioctl::nonblocking(writer.as_fd())?;
            command
                .stdout(Stdio::from(writer))
                .env(crate::worker::REPORT_ENV, "1");
            Some(
                crate::termination::TerminalReader::new(reader, run_id)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
            )
        } else {
            command.env_remove(crate::worker::REPORT_ENV);
            None
        };
        let parent = std::process::id();
        command.env(crate::worker::PARENT_ENV, parent.to_string());
        crate::ioctl::command_parent_death(&mut command, parent)?;
        let child = command.spawn()?;
        // Drop Command here so the parent retains no worker endpoint or duplicate stdio.
        drop(command);

        Ok(Self {
            child,
            reader,
            writer,
            watchdog,
            request,
            written: 0,
            response: [0; RECEIPT_BYTES],
            read: 0,
            reason: None,
            kill_sent: false,
            exit: None,
            channel_closed: false,
            run_id,
            command_started: false,
            stop_frame: false,
            shutdown: None,
            terminal,
        })
    }
    /// Never reads terminal bytes while the owned child may still be alive.
    /// This does not change exit status, stop reason, epoch or lease.
    pub fn poll_terminal(&mut self) -> crate::termination::ReadStatus {
        let Some(reader) = self.terminal.as_mut() else {
            return crate::termination::ReadStatus::Unsupported;
        };
        if self.exit.is_none() {
            return crate::termination::ReadStatus::NotExited;
        }
        reader.poll()
    }
    pub fn progress_epoch(&self) -> u64 {
        self.watchdog.epoch()
    }
    pub fn deadline(&self) -> MonoTime {
        self.shutdown
            .as_ref()
            .map_or(self.watchdog.deadline(), |v| v.deadline)
    }
    pub fn status(&self) -> SupervisorStatus {
        if let Some(status) = self.exit {
            SupervisorStatus::Exited {
                status,
                reason: self.reason.or(self
                    .shutdown
                    .as_ref()
                    .map(|_| TerminationReason::Cooperative)),
            }
        } else if let Some(reason) = self.reason {
            SupervisorStatus::KillRequested(reason)
        } else if let Some(shutdown) = &self.shutdown {
            SupervisorStatus::ShutdownRequested {
                deadline: shutdown.deadline,
                request_sent: self.stop_frame && self.written == CHALLENGE_BYTES,
            }
        } else {
            SupervisorStatus::Running
        }
    }
    /// Signal acceptance is not exit proof. Keep polling until actual try_wait succeeds.
    pub fn request_stop(&mut self) -> io::Result<()> {
        if self.exit.is_some() {
            return Ok(());
        }
        self.reason.get_or_insert(TerminationReason::Requested);
        self.kill()
    }
    /// Only for a known controlled worker. No receipt read/lease renewal after
    /// this request; write acknowledgement and cooperative intent are not exit.
    pub fn request_shutdown(&mut self, now: MonoTime) -> io::Result<SupervisorStatus> {
        if self.exit.is_some() {
            return Ok(self.status());
        }
        if let Some(reason) = self.reason {
            return self.terminate(reason);
        }
        if self.shutdown.is_some() {
            return self.check_shutdown(now);
        }
        if let Err(error) = self.watchdog.check(now) {
            return self.terminate(TerminationReason::Progress(error));
        }
        let untouched = !self.command_started && self.written == 0;
        let challenge = crate::progress::Challenge::decode(&self.request)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let id = if untouched {
            challenge.id
        } else {
            let Some(id) = challenge.id.checked_add(1) else {
                return self.terminate(TerminationReason::Progress(ProgressError::Exhausted));
            };
            id
        };
        self.shutdown = Some(CooperativeShutdown {
            id,
            now,
            deadline: self
                .watchdog
                .deadline()
                .min(MonoTime(now.0.saturating_add(SHUTDOWN_NS))),
        });
        if untouched || self.written == CHALLENGE_BYTES {
            self.queue_stop();
        }
        Ok(self.status())
    }
    fn queue_stop(&mut self) {
        if let Some(shutdown) = &self.shutdown {
            self.request = crate::progress::StopRequest {
                run_id: self.run_id,
                id: shutdown.id,
            }
            .encode();
            self.written = 0;
            self.command_started = false;
            self.stop_frame = true;
        }
    }
    fn check_shutdown(&mut self, now: MonoTime) -> io::Result<SupervisorStatus> {
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
        if let Some(error) = error {
            return self.terminate(TerminationReason::Progress(error));
        }
        Ok(self.status())
    }
    fn poll_shutdown(&mut self, now: MonoTime) -> io::Result<SupervisorStatus> {
        self.check_shutdown(now)?;
        if self.reason.is_some() {
            return Ok(self.status());
        }
        if self.written < CHALLENGE_BYTES {
            self.command_started = true;
            match self.writer.write(&self.request[self.written..]) {
                Ok(0) => return self.terminate(TerminationReason::Channel),
                Ok(count) if count > CHALLENGE_BYTES - self.written => {
                    return self.terminate(TerminationReason::Progress(ProgressError::Frame));
                }
                Ok(count) => self.written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => (),
                Err(_) => return self.terminate(TerminationReason::Channel),
            }
        }
        if !self.stop_frame && self.written == CHALLENGE_BYTES {
            self.queue_stop();
        }
        Ok(self.status())
    }
    fn kill(&mut self) -> io::Result<()> {
        if !self.kill_sent {
            self.child.kill()?;
            self.kill_sent = true;
        }
        Ok(())
    }
    fn terminate(&mut self, reason: TerminationReason) -> io::Result<SupervisorStatus> {
        self.reason.get_or_insert(reason);
        self.kill()?;
        Ok(self.status())
    }
    pub fn poll(&mut self, now: MonoTime) -> io::Result<SupervisorStatus> {
        if self.exit.is_some() {
            return Ok(self.status());
        }
        if let Some(status) = self.child.try_wait()? {
            self.exit = Some(status);
            return Ok(self.status());
        }
        if self.reason.is_some() {
            self.kill()?;
            return Ok(self.status());
        }
        if self.shutdown.is_some() {
            return self.poll_shutdown(now);
        }
        if let Err(error) = self.watchdog.check(now) {
            return self.terminate(TerminationReason::Progress(error));
        }
        if self.watchdog.challenge().is_some() && self.written < CHALLENGE_BYTES {
            self.command_started = true;
            match self.writer.write(&self.request[self.written..]) {
                Ok(0) => return self.terminate(TerminationReason::Channel),
                Ok(count) => self.written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => return self.terminate(TerminationReason::Channel),
            }
        }
        if self.channel_closed {
            return Ok(self.status());
        }
        match self.reader.read(&mut self.response[self.read..]) {
            Ok(0) => {
                self.channel_closed = true;
                if self.watchdog.phase() == Some(RuntimePhase::Stopped) {
                    return Ok(self.status());
                }
                return self.terminate(TerminationReason::Channel);
            }
            Ok(count) => self.read += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(self.status()),
            Err(_) => return self.terminate(TerminationReason::Channel),
        }
        if self.read == RECEIPT_BYTES {
            let receipt = match Receipt::decode(&self.response) {
                Ok(receipt) => receipt,
                Err(error) => return self.terminate(TerminationReason::Progress(error)),
            };
            if self.written != CHALLENGE_BYTES {
                return self.terminate(TerminationReason::Progress(ProgressError::Identity));
            }
            if let Err(error) = self.watchdog.observe(receipt, now) {
                return self.terminate(TerminationReason::Progress(error));
            }
            self.read = 0;
            if let Some(challenge) = self.watchdog.challenge() {
                self.request = challenge.encode();
                self.written = 0;
                self.command_started = false;
            }
        }
        Ok(self.status())
    }
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        if self.exit.is_some() {
            return;
        }
        let _ = self.child.kill();
        // Emergency fallback is bounded. Normal owners must poll to Exited;
        // an uninterruptible kernel task cannot be promised terminated here.
        let until = Instant::now() + Duration::from_millis(25);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => {}
            }
            if Instant::now() >= until {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::progress::Challenge;
    use std::os::unix::process::ExitStatusExt;
    fn command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "supervisor::tests::worker_fixture",
                "--nocapture",
            ])
            .env("KEYWEAVE_SUPERVISOR_FIXTURE_MODE", mode)
            .stdout(Stdio::null());
        command
    }
    fn wait_until(supervisor: &mut Supervisor, now: MonoTime, ready: impl Fn(&Supervisor) -> bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while !ready(supervisor) {
            supervisor.poll(now).unwrap();
            assert!(
                Instant::now() < end,
                "owned test worker did not reach expected state: {:?}, {:?}",
                supervisor.status(),
                supervisor.response
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn worker_fixture() {
        let Ok(mode) = std::env::var("KEYWEAVE_SUPERVISOR_FIXTURE_MODE") else {
            return;
        };
        if mode == "startup_stall" {
            loop {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let mut bytes = [0; CHALLENGE_BYTES];
        let mut count = 0;
        let end = Instant::now() + Duration::from_secs(3);
        while count < bytes.len() {
            match io::stdin().read(&mut bytes[count..]) {
                Ok(0) => panic!("test progress channel closed"),
                Ok(read) => count += read,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(error) => panic!("{error}"),
            }
            assert!(Instant::now() < end);
        }
        if mode == "cooperative" {
            let mut last = 0;
            loop {
                if let Ok(stop) = crate::progress::StopRequest::decode(&bytes) {
                    assert_eq!(
                        stop.run_id,
                        std::env::var(crate::worker::RUN_ENV)
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                    );
                    assert_eq!(stop.id, last + 1);
                    return;
                }
                let challenge = Challenge::decode(&bytes).unwrap();
                assert_eq!(challenge.id, last + 1);
                last = challenge.id;
                io::stderr()
                    .write_all(
                        &Receipt {
                            challenge,
                            baseline: last - 1,
                            epoch: last,
                            phase: RuntimePhase::Active,
                        }
                        .encode(),
                    )
                    .unwrap();
                count = 0;
                while count < bytes.len() {
                    match io::stdin().read(&mut bytes[count..]) {
                        Ok(0) => panic!("controlled channel closed"),
                        Ok(n) => count += n,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(1))
                        }
                        Err(e) => panic!("{e}"),
                    }
                    assert!(Instant::now() < end);
                }
            }
        }
        let mut challenge = Challenge::decode(&bytes).unwrap();
        if mode == "wrong_run" {
            challenge.run_id += 1;
        }
        let receipt = Receipt {
            challenge,
            baseline: 0,
            epoch: 1,
            phase: if mode == "exit" {
                RuntimePhase::Stopped
            } else {
                RuntimePhase::Active
            },
        };
        io::stderr().write_all(&receipt.encode()).unwrap();
        if mode == "exit" {
            return;
        }
        // The background thread remains alive, while the worker event loop
        // stops answering subsequent challenges. No input device is opened.
        let _heartbeat = std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        loop {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn stalled_event_loop_is_killed_even_with_a_live_background_thread() {
        let mut supervisor =
            Supervisor::spawn(command("stall"), 7, Policy::default(), MonoTime(0)).unwrap();
        wait_until(&mut supervisor, MonoTime(0), |value| {
            value.progress_epoch() == 1
        });
        assert_eq!(supervisor.status(), SupervisorStatus::Running);
        let deadline = supervisor.deadline();
        assert_eq!(
            supervisor.poll(deadline).unwrap(),
            SupervisorStatus::KillRequested(TerminationReason::Progress(ProgressError::Expired))
        );
        wait_until(&mut supervisor, deadline, |value| {
            matches!(value.status(), SupervisorStatus::Exited { .. })
        });
        assert!(
            matches!(supervisor.status(),SupervisorStatus::Exited {status,reason:Some(TerminationReason::Progress(ProgressError::Expired))} if !status.success())
        );
    }
    #[test]
    fn no_startup_progress_has_a_finite_lease_and_actual_exit_is_reaped() {
        let mut supervisor =
            Supervisor::spawn(command("startup_stall"), 8, Policy::default(), MonoTime(0)).unwrap();
        let deadline = supervisor.deadline();
        assert!(matches!(
            supervisor.poll(deadline).unwrap(),
            SupervisorStatus::KillRequested(_)
        ));
        wait_until(&mut supervisor, deadline, |value| {
            matches!(value.status(), SupervisorStatus::Exited { .. })
        });
        assert!(matches!(
            supervisor.poll(deadline).unwrap(),
            SupervisorStatus::Exited { .. }
        ));
    }
    #[test]
    fn wrong_run_id_terminates_only_the_owned_child() {
        let mut supervisor =
            Supervisor::spawn(command("wrong_run"), 9, Policy::default(), MonoTime(0)).unwrap();
        wait_until(&mut supervisor, MonoTime(0), |value| {
            value.status() != SupervisorStatus::Running
        });
        wait_until(&mut supervisor, MonoTime(0), |value| {
            matches!(value.status(), SupervisorStatus::Exited { .. })
        });
        assert!(
            matches!(
                supervisor.status(),
                SupervisorStatus::Exited {
                    reason: Some(TerminationReason::Progress(ProgressError::Identity)),
                    ..
                }
            ),
            "{:?}, {:?}",
            supervisor.status(),
            supervisor.response
        );
    }
    #[test]
    fn clean_worker_exit_is_authoritative_and_requested_stop_does_not_signal_again() {
        let mut supervisor =
            Supervisor::spawn(command("exit"), 10, Policy::default(), MonoTime(0)).unwrap();
        wait_until(&mut supervisor, MonoTime(0), |value| {
            matches!(value.status(), SupervisorStatus::Exited { .. })
        });
        assert!(
            matches!(supervisor.status(),SupervisorStatus::Exited {status,reason:None} if status.success())
        );
        supervisor.request_stop().unwrap();
        assert!(matches!(
            supervisor.status(),
            SupervisorStatus::Exited { reason: None, .. }
        ));
        let mut supervisor =
            Supervisor::spawn(command("startup_stall"), 11, Policy::default(), MonoTime(0))
                .unwrap();
        supervisor.request_stop().unwrap();
        wait_until(&mut supervisor, MonoTime(0), |value| {
            matches!(value.status(), SupervisorStatus::Exited { .. })
        });
        assert!(matches!(
            supervisor.status(),
            SupervisorStatus::Exited {
                reason: Some(TerminationReason::Requested),
                ..
            }
        ));
    }
    #[test]
    fn creator_thread_exit_kills_worker_even_while_parent_process_lives() {
        let mut supervisor = std::thread::spawn(|| {
            let mut supervisor =
                Supervisor::spawn(command("stall"), 12, Policy::default(), MonoTime(0)).unwrap();
            wait_until(&mut supervisor, MonoTime(0), |value| {
                value.progress_epoch() == 1
            });
            supervisor
        })
        .join()
        .unwrap();
        // Ownership was moved out of the creator: Supervisor::drop did not run
        // there. Reap directly, avoiding poll's channel-failure kill fallback.
        let end = Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = supervisor.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < end,
                "kernel parent-death guard did not terminate worker"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        supervisor.exit = Some(status);
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        assert_eq!(supervisor.reason, None);
        assert!(!supervisor.kill_sent);
    }
    #[test]
    fn mismatched_parent_rejects_spawn_before_worker_exec() {
        let mut command = command("startup_stall");
        crate::ioctl::command_parent_death(&mut command, 0).unwrap_err();
        crate::ioctl::command_parent_death(&mut command, std::process::id() + 1).unwrap();
        let error = command.spawn().unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ECANCELED));
    }
    #[test]
    fn cooperative_stop_replaces_untouched_challenge_and_waits_for_actual_exit() {
        let mut s =
            Supervisor::spawn(command("cooperative"), 7, Policy::default(), MonoTime(0)).unwrap();
        assert_eq!(
            s.request_shutdown(MonoTime(1)).unwrap(),
            SupervisorStatus::ShutdownRequested {
                deadline: MonoTime(2_000_000_001),
                request_sent: false
            }
        );
        assert_eq!(
            crate::progress::StopRequest::decode(&s.request).unwrap().id,
            1
        );
        wait_until(&mut s, MonoTime(1), |s| s.exit.is_some());
        assert!(
            matches!(s.status(),SupervisorStatus::Exited{status,reason:Some(TerminationReason::Cooperative)} if status.success())
        );
        assert!(!s.kill_sent);
        assert_eq!(s.progress_epoch(), 0);
        assert_eq!(s.read, 0);
    }
    #[test]
    fn cooperative_stop_completes_partial_challenge_before_next_id_without_reading_reply() {
        let mut s =
            Supervisor::spawn(command("cooperative"), 8, Policy::default(), MonoTime(0)).unwrap();
        // Produce a real fragmented frame into the private child pipe. Native
        // small writes are normally atomic; do not depend on that for framing.
        assert_eq!(s.writer.write(&s.request[..7]).unwrap(), 7);
        s.written = 7;
        s.command_started = true;
        s.request_shutdown(MonoTime(1)).unwrap();
        assert!(!s.stop_frame);
        s.poll(MonoTime(1)).unwrap();
        assert!(s.stop_frame);
        assert_eq!(s.written, 0);
        assert_eq!(
            crate::progress::StopRequest::decode(&s.request).unwrap().id,
            2
        );
        wait_until(&mut s, MonoTime(1), |s| s.exit.is_some());
        assert!(s.exit.unwrap().success());
        assert!(!s.kill_sent);
        assert_eq!(s.progress_epoch(), 0);
        assert_eq!(s.read, 0);
    }
    #[test]
    fn cooperative_stop_after_completed_challenge_keeps_its_successor_id() {
        let mut s =
            Supervisor::spawn(command("cooperative"), 9, Policy::default(), MonoTime(0)).unwrap();
        assert_eq!(s.writer.write(&s.request).unwrap(), CHALLENGE_BYTES);
        s.written = CHALLENGE_BYTES;
        s.command_started = true;
        let epoch = s.progress_epoch();
        let read = s.read;
        s.request_shutdown(MonoTime(1)).unwrap();
        assert_eq!(
            crate::progress::StopRequest::decode(&s.request).unwrap().id,
            2
        );
        wait_until(&mut s, MonoTime(1), |s| s.exit.is_some());
        assert!(s.exit.unwrap().success());
        assert_eq!(s.progress_epoch(), epoch);
        assert_eq!(s.read, read);
    }
    #[test]
    fn cooperative_requests_do_not_extend_old_lease_and_timeout_is_reaped() {
        let mut s = Supervisor::spawn(
            command("startup_stall"),
            10,
            Policy {
                startup_ns: 20,
                lease_ns: 10,
            },
            MonoTime(0),
        )
        .unwrap();
        s.request_shutdown(MonoTime(1)).unwrap();
        assert_eq!(s.deadline(), MonoTime(20));
        s.request_shutdown(MonoTime(19)).unwrap();
        assert_eq!(s.deadline(), MonoTime(20));
        assert_eq!(
            s.poll(MonoTime(20)).unwrap(),
            SupervisorStatus::KillRequested(TerminationReason::Progress(ProgressError::Expired))
        );
        wait_until(&mut s, MonoTime(20), |s| s.exit.is_some());
        assert_eq!(s.exit.unwrap().signal(), Some(libc::SIGKILL));
    }
    #[test]
    fn cooperative_clock_and_id_exhaustion_preserve_hard_failure() {
        for exhausted in [false, true] {
            let mut s =
                Supervisor::spawn(command("startup_stall"), 11, Policy::default(), MonoTime(0))
                    .unwrap();
            let expected = if exhausted {
                s.request = Challenge {
                    run_id: 11,
                    id: u64::MAX,
                }
                .encode();
                s.command_started = true;
                s.request_shutdown(MonoTime(1)).unwrap();
                ProgressError::Exhausted
            } else {
                s.request_shutdown(MonoTime(2)).unwrap();
                s.request_shutdown(MonoTime(1)).unwrap();
                ProgressError::Clock
            };
            assert_eq!(s.reason, Some(TerminationReason::Progress(expected)));
            wait_until(&mut s, MonoTime(2), |s| s.exit.is_some());
            assert_eq!(s.exit.unwrap().signal(), Some(libc::SIGKILL));
        }
    }
    #[test]
    fn explicit_hard_stop_overrides_cooperative_intent() {
        let mut s = Supervisor::spawn(command("startup_stall"), 12, Policy::default(), MonoTime(0))
            .unwrap();
        s.request_shutdown(MonoTime(1)).unwrap();
        s.request_stop().unwrap();
        wait_until(&mut s, MonoTime(1), |s| s.exit.is_some());
        assert_eq!(s.reason, Some(TerminationReason::Requested));
        assert_eq!(s.exit.unwrap().signal(), Some(libc::SIGKILL));
    }
}
