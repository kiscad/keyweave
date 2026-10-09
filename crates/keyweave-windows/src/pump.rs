//! Ordered API acceptance; global Windows key state outlives the backend.
use crate::keys::{self, InjectionTag, KeyError, KeyboardPacket};
use keyweave_output::{Mode, OutputError, OutputQueue};
use keyweave_types::{MonoTime, OutputIntent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insertion {
    /// Ordered accepted prefix, never evidence of target application delivery.
    pub accepted: usize,
    /// Diagnostic only: neither zero nor an error code identifies UIPI reliably.
    pub last_error: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError {
    #[error("output rejected before calling the native API")]
    Rejected,
    #[error("output acceptance is unknown")]
    Unknown,
}
pub trait SendBackend {
    /// Rejected is allowed only BEFORE native invocation. Native zero returns
    /// Insertion with accepted=0. Unknown must never be guessed as zero.
    fn submit(&mut self, packets: &[KeyboardPacket]) -> Result<Insertion, SubmitError>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Paused,
    Normal,
    StopRequired,
    Recovery,
    Quarantined,
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpStep {
    Idle,
    Inserted(Insertion),
}
#[derive(Debug, thiserror::Error)]
pub enum PumpError {
    #[error(transparent)]
    Queue(#[from] OutputError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error(transparent)]
    Submit(#[from] SubmitError),
    #[error("invalid accepted prefix from backend")]
    InvalidReceipt,
    #[error("output requires a runtime gate or is closed")]
    NotReady,
}
pub struct OutputPump<B: SendBackend> {
    backend: Option<B>,
    tag: InjectionTag,
    queue: OutputQueue,
    phase: Phase,
}
impl<B: SendBackend> OutputPump<B> {
    pub fn new(backend: B, tag: InjectionTag) -> Self {
        Self {
            backend: Some(backend),
            tag,
            queue: OutputQueue::new(),
            phase: Phase::Paused,
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    /// In quarantine this is only the last known prefix. It can be incomplete.
    pub fn ledger(&self) -> &OutputQueue {
        &self.queue
    }
    /// Caller must establish session/output readiness and physical neutral first.
    pub fn resume(&mut self) -> Result<(), PumpError> {
        if self.phase != Phase::Paused {
            return Err(PumpError::NotReady);
        }
        self.queue.resume()?;
        self.phase = Phase::Normal;
        Ok(())
    }
    pub fn enqueue(&mut self, edges: &[OutputIntent]) -> Result<(), PumpError> {
        if self.phase != Phase::Normal {
            return Err(PumpError::NotReady);
        }
        if edges.len() > keyweave_output::MAX_ENQUEUE_EDGES {
            return Err(OutputError::Capacity.into());
        }
        // Entire append is validated before desired state changes. No API calls.
        for chunk in edges.chunks(keys::MAX_BATCH) {
            keys::encode_batch(chunk, self.tag)?;
        }
        self.queue.enqueue(edges)?;
        Ok(())
    }
    /// Stop capture BEFORE this call. Do not recover from inside an active Hook.
    pub fn begin_recovery(&mut self, now: MonoTime) -> Result<(), PumpError> {
        if !matches!(
            self.phase,
            Phase::Normal | Phase::StopRequired | Phase::Recovery | Phase::Paused
        ) {
            return Err(PumpError::NotReady);
        }
        self.queue.begin_recovery(now)?;
        self.phase = if self.queue.mode() == Mode::Paused {
            Phase::Paused
        } else {
            Phase::Recovery
        };
        Ok(())
    }
    fn quarantine(&mut self) {
        self.queue.quarantine();
        self.phase = Phase::Quarantined;
    }
    /// Capture must have stopped. Unowned input may have changed global state;
    /// preserve the accepted ledger without injecting releases or dropping it.
    pub(crate) fn quarantine_untracked_input(&mut self) {
        if self.phase != Phase::Closed {
            self.quarantine();
        }
    }
    /// One bounded submission; a short normal return forbids further calls until
    /// capture has stopped and begin_recovery is explicitly invoked.
    pub fn poll(&mut self, now: MonoTime) -> Result<PumpStep, PumpError> {
        if !matches!(self.phase, Phase::Normal | Phase::Recovery | Phase::Paused) {
            return Err(PumpError::NotReady);
        }
        let Some(batch) = self.queue.begin_due(now)? else {
            return Ok(PumpStep::Idle);
        };
        let packets = match keys::encode_batch(&batch.edges, self.tag) {
            Ok(packets) => packets,
            Err(error) => {
                self.queue.acknowledge_incomplete(batch.id, 0)?;
                self.phase = Phase::StopRequired;
                return Err(error.into());
            }
        };
        let result = match self.backend.as_mut() {
            Some(backend) => backend.submit(&packets),
            None => {
                self.quarantine();
                return Err(PumpError::NotReady);
            }
        };
        match result {
            Ok(receipt) if receipt.accepted <= packets.len() => {
                self.queue.acknowledge(batch.id, receipt.accepted)?;
                if self.phase == Phase::Normal && receipt.accepted < packets.len() {
                    self.phase = Phase::StopRequired;
                } else if self.phase == Phase::Recovery && self.queue.mode() == Mode::Paused {
                    self.phase = Phase::Paused;
                }
                Ok(PumpStep::Inserted(receipt))
            }
            Ok(_) => {
                self.quarantine();
                Err(PumpError::InvalidReceipt)
            }
            Err(SubmitError::Rejected) => {
                self.queue.acknowledge_incomplete(batch.id, 0)?;
                if self.phase == Phase::Normal {
                    self.phase = Phase::StopRequired;
                } else if self.queue.mode() == Mode::Paused {
                    self.phase = Phase::Paused;
                }
                Err(SubmitError::Rejected.into())
            }
            Err(error) => {
                self.quarantine();
                Err(error.into())
            }
        }
    }
    /// Caller stops capture first. Dropping a SendInput backend DOES NOT reset
    /// global key state: preserve ledger and never call backend_destroyed here.
    pub fn close(&mut self) {
        self.queue.quarantine();
        drop(self.backend.take());
        self.phase = Phase::Closed;
    }
}

#[cfg(test)]
mod tests;
