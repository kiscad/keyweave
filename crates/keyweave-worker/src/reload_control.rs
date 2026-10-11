//! Independent reload transaction driver. Control thread only, never callbacks.
//! Native loop must supply current verified Stop/parent/session/clock evidence.
use crate::reload_preparation::{
    PreparationError, PreparationSlot, PreparationStep, ReloadActor, Target,
};
use keyweave_progress::reload::{
    MAX_TIMEOUT_NS, ReloadError, Worker,
    transport::{AckWriter, ReceiveStep, Receiver, TransferError},
};
use keyweave_types::{ConfigGeneration, MonoTime};
use std::io::{self, Read, Write};

/// Trusted outer control observation, not a token minted by this component.
/// Neither preparation nor reload IO can extend either independent deadline.
pub struct Evidence {
    pub at: MonoTime,
    pub parent_until: MonoTime,
    pub session_until: MonoTime,
    pub stop: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Receiving,
    Received,
    Preparing,
    Ready,
    Acknowledging,
    Closed,
}
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("reload control was stopped or revoked")]
    Revoked,
    #[error("reload control clock failed")]
    Clock(#[source] io::Error),
    #[error(transparent)]
    Protocol(#[from] ReloadError),
    #[error(transparent)]
    Transfer(#[from] TransferError),
    #[error(transparent)]
    Preparation(#[from] PreparationError),
}
pub struct Controller<R: Read, W: Write> {
    receiver: Option<Receiver<R>>,
    writer: Option<W>,
    ack: Option<AckWriter<W>>,
    slot: PreparationSlot,
    target: Target,
    step: Step,
    timeout: u64,
    deadline: Option<MonoTime>,
    last: Option<MonoTime>,
}
impl<R: Read, W: Write> Controller<R, W> {
    pub fn new(
        reader: R,
        writer: W,
        run: u64,
        active: ConfigGeneration,
        timeout: u64,
        target: Target,
    ) -> Result<Self, ControlError> {
        if !(1..=MAX_TIMEOUT_NS).contains(&timeout) {
            return Err(ReloadError::Deadline.into());
        }
        Ok(Self {
            receiver: Some(Receiver::new(reader, Worker::new(run, active)?)),
            writer: Some(writer),
            ack: None,
            slot: PreparationSlot::new(),
            target,
            step: Step::Receiving,
            timeout,
            deadline: None,
            last: None,
        })
    }
    /// Closes physical owned copies; original inherited FDs stay until exit.
    /// Running cold job is retained, never waited for or replaced on Stop.
    pub fn close(&mut self) {
        self.step = Step::Closed;
        self.slot.close();
        self.receiver = None;
        self.writer = None;
        self.ack = None;
    }
    pub fn step(&self) -> Step {
        self.step
    }
    pub fn transaction_deadline(&self) -> Option<MonoTime> {
        self.deadline
    }
    /// Recovery-time only. true means no retained cold job; not process exit,
    /// global recovery, or input/output proof. No evidence/IO/actor operation.
    pub fn reap_closed(&mut self) -> Result<bool, ControlError> {
        if self.step != Step::Closed {
            return Err(ReloadError::Busy.into());
        }
        self.slot.poll()?;
        Ok(!self.slot.job_pending())
    }
    fn observe(&mut self, evidence: io::Result<Evidence>) -> Result<MonoTime, ControlError> {
        let value = evidence.map_err(ControlError::Clock)?;
        if value.stop || value.at >= value.parent_until || value.at >= value.session_until {
            return Err(ControlError::Revoked);
        }
        if self.last.is_some_and(|last| value.at < last) {
            return Err(ReloadError::Clock.into());
        }
        if self.deadline.is_some_and(|deadline| value.at >= deadline) {
            return Err(ReloadError::Expired.into());
        }
        self.last = Some(value.at);
        Ok(value.at)
    }
    /// One state operation and at most one transport IO. Evidence brackets the
    /// operation, including last header/body byte, thread spawn, activate, ack.
    /// A post-activate fault preserves the actual generation but sends no new
    /// ack; a post-write fault cannot retract bytes already accepted by OS.
    pub fn tick<A: ReloadActor>(
        &mut self,
        actor: &mut A,
        mut evidence: impl FnMut() -> io::Result<Evidence>,
    ) -> Result<Step, ControlError> {
        if self.step == Step::Closed {
            return Err(ReloadError::Closed.into());
        }
        keyweave_windows::startup::ensure_preparation_context().map_err(PreparationError::from)?;
        let result = self.tick_inner(actor, &mut evidence);
        if result.is_err() {
            self.close();
        }
        result
    }
    fn tick_inner<A: ReloadActor>(
        &mut self,
        actor: &mut A,
        evidence: &mut impl FnMut() -> io::Result<Evidence>,
    ) -> Result<Step, ControlError> {
        let before = self.observe(evidence())?;
        let mut sent = None;
        match self.step {
            Step::Receiving => {
                let receiver = self.receiver.as_mut().ok_or(ReloadError::Closed)?;
                let received = receiver.poll()?;
                // The first consumed header byte anchors the transaction at
                // PRE-IO time. Idle WouldBlock never creates/renews a budget.
                if receiver.receiving() && self.deadline.is_none() {
                    self.deadline = Some(MonoTime(
                        before
                            .0
                            .checked_add(self.timeout)
                            .ok_or(ReloadError::Deadline)?,
                    ));
                }
                if received == ReceiveStep::Ready {
                    self.step = Step::Received;
                }
            }
            Step::Received => {
                let receiver = self.receiver.as_mut().ok_or(ReloadError::Closed)?;
                self.slot.start(
                    receiver.take().ok_or(ReloadError::NoPending)?,
                    self.target.clone(),
                )?;
                self.step = Step::Preparing;
            }
            Step::Preparing => {
                if self.slot.poll()? == PreparationStep::Ready {
                    self.step = Step::Ready;
                }
            }
            Step::Ready => {
                let receiver = self.receiver.as_mut().ok_or(ReloadError::Closed)?;
                let ack = self.slot.deliver(receiver, actor)?;
                self.ack = Some(AckWriter::new(
                    self.writer.take().ok_or(ReloadError::Closed)?,
                    ack,
                )?);
                self.step = Step::Acknowledging;
            }
            Step::Acknowledging => {
                sent = self.ack.as_mut().ok_or(ReloadError::Closed)?.poll()?;
            }
            Step::Closed => return Err(ReloadError::Closed.into()),
        }
        self.observe(evidence())?;
        if let Some(sent) = sent {
            self.receiver
                .as_mut()
                .ok_or(ReloadError::Closed)?
                .ack_sent(sent)?;
            self.writer = self.ack.take().and_then(AckWriter::into_completed_writer);
            if self.writer.is_none() {
                return Err(ReloadError::Closed.into());
            }
            self.deadline = None;
            self.step = Step::Receiving;
        }
        Ok(self.step)
    }
}

#[cfg(target_os = "linux")]
impl<
    R: Read,
    W: Write,
    S: keyweave_linux::capture::CaptureSource,
    B: keyweave_linux::pump::FrameBackend,
> keyweave_linux::worker::WorkerExtension<S, B> for Controller<R, W>
{
    fn tick(
        &mut self,
        runtime: &mut keyweave_linux::runtime::Runtime<S, B>,
        observe: &mut dyn FnMut() -> io::Result<keyweave_linux::worker::ControlObservation>,
    ) -> io::Result<()> {
        Controller::tick(self, runtime, || {
            observe().map(|value| Evidence {
                at: value.at,
                parent_until: value.parent_until,
                session_until: value.session_until,
                stop: value.stop,
            })
        })
        .map(|_| ())
        .map_err(io::Error::other)
    }
    fn close(&mut self) {
        Controller::close(self);
    }
}
