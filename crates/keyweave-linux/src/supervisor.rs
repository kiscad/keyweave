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
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorStatus {
    Running,
    KillRequested(TerminationReason),
    Exited {
        status: ExitStatus,
        reason: Option<TerminationReason>,
    },
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
}
impl Supervisor {
    /// Command must be a fixed trusted worker, with no inherited device FDs.
    /// Its stdin/stderr are replaced by a private nonblocking binary channel.
    /// The creating thread must live until the worker exits: Linux parent-death
    /// SIGKILL is installed before exec. Worker main must verify its inherited
    /// guard before opening devices; credential-changing exec is unsupported.
    pub fn spawn(
        mut command: Command,
        run_id: u64,
        policy: Policy,
        now: MonoTime,
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
        command.env("KEYWEAVE_PROGRESS_RUN_ID", run_id.to_string());
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
        })
    }
    pub fn progress_epoch(&self) -> u64 {
        self.watchdog.epoch()
    }
    pub fn deadline(&self) -> MonoTime {
        self.watchdog.deadline()
    }
    pub fn status(&self) -> SupervisorStatus {
        if let Some(status) = self.exit {
            SupervisorStatus::Exited {
                status,
                reason: self.reason,
            }
        } else if let Some(reason) = self.reason {
            SupervisorStatus::KillRequested(reason)
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
        if let Err(error) = self.watchdog.check(now) {
            return self.terminate(TerminationReason::Progress(error));
        }
        if self.watchdog.challenge().is_some() && self.written < CHALLENGE_BYTES {
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
}
