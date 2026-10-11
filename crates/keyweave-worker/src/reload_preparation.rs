//! One cold preparation job per control owner. Never call from input callbacks.
//! No native input, file IO, progress, clock or acknowledgement writes.
use crate::{linux_preparation as linux, preparation as windows};
use keyweave_engine::{ConfigSnapshot, Engine};
use keyweave_progress::reload::{
    Ack, Outcome, ReloadError, Request,
    transport::{Received, Receiver, TransferError},
};
use keyweave_types::ConfigGeneration;
use std::{io::Read, sync::Arc, thread::JoinHandle};

pub enum Target {
    Linux(keyweave_linux::output::DeviceSpec),
    Windows,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Platform {
    Linux,
    Windows,
}
struct Candidate {
    platform: Platform,
    snapshot: Arc<ConfigSnapshot>,
}
struct Completion {
    request: Request,
    candidate: Option<Candidate>,
}

#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    #[error(transparent)]
    Protocol(#[from] ReloadError),
    #[error(transparent)]
    Context(#[from] keyweave_windows::startup::ProbeError),
    #[error("cold preparation thread could not start")]
    Spawn,
    #[error("cold preparation thread panicked")]
    Panicked,
    #[error(transparent)]
    Transfer(#[from] TransferError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationStep {
    Idle,
    Preparing,
    Ready,
    Closed,
}

/// Keep the same owner after Stop until its running job is reaped. Close never
/// waits for compilation or starts a replacement. Drop detaches an unfinished
/// job; native integration must retain/poll this owner during bounded recovery.
/// Thread/allocator/OS scheduling costs are not hard realtime bounds.
#[derive(Default)]
pub struct PreparationSlot {
    job: Option<JoinHandle<Completion>>,
    ready: Option<Completion>,
    closed: bool,
}
impl PreparationSlot {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.ready = None;
    }
    pub fn start(&mut self, received: Received, target: Target) -> Result<(), PreparationError> {
        keyweave_windows::startup::ensure_preparation_context()?;
        if self.closed {
            return Err(ReloadError::Closed.into());
        }
        if self.job.is_some() || self.ready.is_some() {
            return Err(ReloadError::Busy.into());
        }
        let request = received.request;
        if let Err(error) = request.validate() {
            self.close();
            return Err(error.into());
        }
        if received.body.len() as u64 != request.byte_length {
            self.close();
            return Err(ReloadError::Length.into());
        }
        match std::thread::Builder::new()
            .name("keyweave-config".into())
            .spawn(move || {
                let candidate = match target {
                    Target::Linux(spec) => linux::PreparedConfig::from_bytes_for_generation(
                        received.body,
                        spec,
                        request.target,
                    )
                    .ok()
                    .map(|p| Candidate {
                        platform: Platform::Linux,
                        snapshot: p.snapshot().clone(),
                    }),
                    Target::Windows => windows::PreparedConfig::from_bytes_for_generation(
                        received.body,
                        request.target,
                    )
                    .ok()
                    .map(|p| Candidate {
                        platform: Platform::Windows,
                        snapshot: p.snapshot().clone(),
                    }),
                };
                // Exact bytes and cold diagnostics are dropped here, not on the
                // actor's input path. Only immutable IR crosses back to control.
                Completion { request, candidate }
            }) {
            Ok(job) => {
                self.job = Some(job);
                Ok(())
            }
            Err(_) => {
                self.close();
                Err(PreparationError::Spawn)
            }
        }
    }
    /// No join while the job is running. Closed owners still reap a completed
    /// job and discard its result; a late success never reopens the lane.
    pub fn poll(&mut self) -> Result<PreparationStep, PreparationError> {
        keyweave_windows::startup::ensure_preparation_context()?;
        if self.job.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(job) = self.job.take()
        {
            match job.join() {
                Ok(result) if !self.closed => self.ready = Some(result),
                Ok(_) => {}
                Err(_) => {
                    self.close();
                    return Err(PreparationError::Panicked);
                }
            }
        }
        Ok(if self.closed {
            PreparationStep::Closed
        } else if self.ready.is_some() {
            PreparationStep::Ready
        } else if self.job.is_some() {
            PreparationStep::Preparing
        } else {
            PreparationStep::Idle
        })
    }
    /// Control-thread only, after independent Stop/lease/deadline checks. This
    /// applies on the actual actor, then builds ack from its current generation.
    /// No output IO or progress is performed. AckWriter/AckSent remain required.
    pub fn deliver<R: Read, A: ReloadActor>(
        &mut self,
        receiver: &mut Receiver<R>,
        actor: &mut A,
    ) -> Result<Ack, PreparationError> {
        if let Err(error) = keyweave_windows::startup::ensure_preparation_context() {
            self.close();
            receiver.close();
            return Err(error.into());
        }
        if self.closed {
            receiver.close();
            return Err(ReloadError::Closed.into());
        }
        let Some(result) = self.ready.take() else {
            return Err(ReloadError::NoPending.into());
        };
        if !receiver.awaiting_commit(result.request) || actor.generation() != result.request.base {
            self.close();
            receiver.close();
            return Err(ReloadError::Generation.into());
        }
        let accepted = result.candidate.is_some_and(|candidate| {
            candidate.snapshot.generation == result.request.target
                && actor.supports(candidate.platform == Platform::Linux)
                && actor.apply(candidate.snapshot)
        });
        let outcome = if accepted {
            Outcome::Accepted
        } else {
            Outcome::Rejected
        };
        match receiver.finish(outcome, actor.generation()) {
            Ok(ack) => Ok(ack),
            Err(error) => {
                self.close();
                Err(error.into())
            }
        }
    }
}

mod sealed {
    use super::*;
    pub trait Actor {
        fn generation(&self) -> ConfigGeneration;
        fn supports(&self, linux: bool) -> bool;
        fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool;
    }
}
/// Sealed to engine and actual platform actors, not caller-supplied ack claims.
pub trait ReloadActor: sealed::Actor {}
impl sealed::Actor for Engine {
    fn generation(&self) -> ConfigGeneration {
        self.config_generation()
    }
    fn supports(&self, _: bool) -> bool {
        true
    }
    fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
        self.activate(snapshot).is_ok()
    }
}
impl ReloadActor for Engine {}
impl<S: keyweave_linux::capture::CaptureSource, B: keyweave_linux::pump::FrameBackend> sealed::Actor
    for keyweave_linux::runtime::Runtime<S, B>
{
    fn generation(&self) -> ConfigGeneration {
        self.config_generation()
    }
    fn supports(&self, linux: bool) -> bool {
        linux
    }
    fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
        self.activate(snapshot).is_ok()
    }
}
impl<S: keyweave_linux::capture::CaptureSource, B: keyweave_linux::pump::FrameBackend> ReloadActor
    for keyweave_linux::runtime::Runtime<S, B>
{
}
impl<B: keyweave_windows::pump::SendBackend> sealed::Actor
    for keyweave_windows::runtime::SessionActor<B>
{
    fn generation(&self) -> ConfigGeneration {
        self.config_generation()
    }
    fn supports(&self, linux: bool) -> bool {
        !linux
    }
    fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
        self.activate(snapshot).is_ok()
    }
}
impl<B: keyweave_windows::pump::SendBackend> ReloadActor
    for keyweave_windows::runtime::SessionActor<B>
{
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn stop_keeps_same_running_job_until_reaped_and_never_reopens() {
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let job = std::thread::spawn(move || {
            wait.recv().unwrap();
            Completion {
                request: Request {
                    run_id: 1,
                    id: 1,
                    base: ConfigGeneration(1),
                    target: ConfigGeneration(2),
                    byte_length: 1,
                },
                candidate: None,
            }
        });
        let mut slot = PreparationSlot {
            job: Some(job),
            ready: None,
            closed: false,
        };
        assert_eq!(slot.poll().unwrap(), PreparationStep::Preparing);
        slot.close();
        assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
        assert!(slot.job.is_some());
        release.send(()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while slot.job.is_some() {
            assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(slot.ready.is_none());
    }
    #[test]
    fn panicked_job_closes_without_a_candidate() {
        let mut slot = PreparationSlot {
            job: Some(std::thread::spawn(|| panic!("test cold panic"))),
            ready: None,
            closed: false,
        };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match slot.poll() {
                Err(PreparationError::Panicked) => break,
                Ok(PreparationStep::Preparing) => {}
                other => panic!("unexpected {other:?}"),
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
    }
}
