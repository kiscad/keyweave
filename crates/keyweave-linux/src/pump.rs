//! One owner connects bounded semantic scheduling to native frame submission.
//! Runtime must stop physical capture whenever this pump leaves Normal phase.
use crate::{
    events::{FrameWrite, NativeKey},
    keys::{self, KeyError},
    output::{DeviceSpec, WriteError},
};
use keyweave_output::{OutputError, OutputQueue};
use keyweave_types::{KeyEdge, MonoTime, OutputIntent};
use std::io;

/// Implementations own their backend; dropping it must destroy its virtual device.
/// Err(io::Error) means rejection BEFORE writing; errors after writing must be
/// WriteError::UnknownAcceptance. Complete receipts include the native REPORT.
pub trait FrameBackend {
    fn spec(&self) -> &DeviceSpec;
    /// Canonical virtual input identity. Runtime requires this; codecs alone do not.
    fn input_syspath(&self) -> Option<&std::path::Path> {
        None
    }
    fn submit(
        &mut self,
        edges: &[NativeKey],
        recovery: bool,
    ) -> io::Result<Result<FrameWrite, WriteError>>;
    fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Normal,
    ReportPending,
    Recovery,
    Quarantined,
    Detached,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PumpStep {
    Idle,
    Submitted { accepted: usize },
    ReportPending { accepted: usize },
    ReportFlushed,
}
#[derive(Debug, thiserror::Error)]
pub enum PumpError {
    #[error(transparent)]
    Queue(#[from] OutputError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("backend rejected output before submission: {0}")]
    Rejected(#[source] io::Error),
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error("backend receipt violates frame contract")]
    InvalidReceipt,
    #[error("backend is paused or detached")]
    NotReady,
}
pub struct OutputPump<B: FrameBackend> {
    backend: Option<B>,
    queue: OutputQueue,
    phase: Phase,
    now: MonoTime,
}
#[cfg(target_os = "linux")]
impl OutputPump<crate::uinput::UinputDevice> {
    pub(crate) fn wait_fd(&self) -> Option<std::os::fd::BorrowedFd<'_>> {
        self.backend
            .as_ref()
            .map(crate::uinput::UinputDevice::wait_fd)
    }
}
impl<B: FrameBackend> OutputPump<B> {
    #[cfg(target_os = "linux")]
    pub(crate) fn backend(&self) -> Option<&B> {
        self.backend.as_ref()
    }
    pub fn new(backend: B) -> Self {
        Self {
            backend: Some(backend),
            queue: OutputQueue::new(),
            phase: Phase::Normal,
            now: MonoTime(0),
        }
    }
    pub fn spec(&self) -> Option<&DeviceSpec> {
        self.backend.as_ref().map(FrameBackend::spec)
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    /// Read-only last known submitted state; quarantined state may be incomplete.
    pub fn ledger(&self) -> &OutputQueue {
        &self.queue
    }
    /// Runtime must independently prove output consumer readiness, own-device
    /// exclusion and physical neutral before activating capture.
    pub fn resume(&mut self) -> Result<(), PumpError> {
        if self.phase != Phase::Normal {
            return Err(PumpError::NotReady);
        }
        self.queue.resume()?;
        Ok(())
    }
    fn native(edges: &[OutputIntent]) -> Result<Vec<NativeKey>, PumpError> {
        edges
            .iter()
            .map(|edge| {
                Ok(NativeKey {
                    code: keys::output_code(edge.key)?,
                    value: match edge.edge {
                        KeyEdge::Up => 0,
                        KeyEdge::Down => 1,
                        KeyEdge::Repeat => 2,
                    },
                })
            })
            .collect()
    }
    /// Validate native capability/repeat policy before mutating semantic state.
    pub fn enqueue(&mut self, edges: &[OutputIntent]) -> Result<(), PumpError> {
        if self.phase != Phase::Normal {
            return Err(PumpError::NotReady);
        }
        if edges.len() > keyweave_output::MAX_ENQUEUE_EDGES {
            return Err(OutputError::Capacity.into());
        }
        let native = Self::native(edges)?;
        let backend = self.backend.as_ref().ok_or(PumpError::NotReady)?;
        for chunk in native.chunks(crate::events::MAX_FRAME_EVENTS) {
            backend
                .spec()
                .validate(chunk)
                .map_err(PumpError::Rejected)?;
        }
        self.queue.enqueue(edges)?;
        Ok(())
    }
    /// Stop capture first. This discards future intents and schedules known ups.
    pub fn begin_recovery(&mut self, now: MonoTime) -> Result<(), PumpError> {
        if !matches!(self.phase, Phase::Normal | Phase::Recovery) {
            return Err(PumpError::NotReady);
        }
        if now < self.now {
            return Err(OutputError::TimeReversed.into());
        }
        self.queue.begin_recovery(now)?;
        self.now = now;
        self.phase = Phase::Recovery;
        Ok(())
    }
    fn quarantine(&mut self) {
        self.queue.quarantine();
        self.phase = Phase::Quarantined;
    }
    /// One bounded operation per call. REPORT completion always precedes recovery
    /// ups; an empty submitted ledger alone cannot bypass the pending report.
    pub fn poll(&mut self, now: MonoTime) -> Result<PumpStep, PumpError> {
        if matches!(self.phase, Phase::Quarantined | Phase::Detached) {
            return Err(PumpError::NotReady);
        }
        if now < self.now {
            return Err(OutputError::TimeReversed.into());
        }
        self.now = now;
        if self.phase == Phase::ReportPending {
            let result = self
                .backend
                .as_mut()
                .ok_or(PumpError::NotReady)?
                .flush_report();
            return match result {
                Ok(Ok(true)) => {
                    self.phase = Phase::Recovery;
                    Ok(PumpStep::ReportFlushed)
                }
                Ok(Ok(false)) => Ok(PumpStep::Idle),
                Ok(Err(error)) => {
                    self.quarantine();
                    Err(error.into())
                }
                Err(error) => {
                    self.quarantine();
                    Err(PumpError::Rejected(error))
                }
            };
        }
        let Some(batch) = self.queue.begin_due(now)? else {
            return Ok(PumpStep::Idle);
        };
        let native = match Self::native(&batch.edges) {
            Ok(native) => native,
            Err(error) => {
                self.queue.acknowledge_incomplete(batch.id, 0)?;
                self.phase = Phase::Recovery;
                return Err(error);
            }
        };
        let result = self
            .backend
            .as_mut()
            .ok_or(PumpError::NotReady)?
            .submit(&native, self.phase == Phase::Recovery);
        match result {
            Ok(Ok(FrameWrite::Complete { accepted })) if accepted == native.len() => {
                self.queue.acknowledge(batch.id, accepted)?;
                Ok(PumpStep::Submitted { accepted })
            }
            Ok(Ok(FrameWrite::Incomplete {
                accepted,
                report_pending: true,
            })) if accepted <= native.len() => {
                self.queue.acknowledge_incomplete(batch.id, accepted)?;
                self.phase = Phase::ReportPending;
                Ok(PumpStep::ReportPending { accepted })
            }
            Ok(Ok(_)) => {
                self.quarantine();
                Err(PumpError::InvalidReceipt)
            }
            Ok(Err(WriteError::InvalidFrame(error))) => {
                self.queue.acknowledge_incomplete(batch.id, 0)?;
                self.phase = Phase::Recovery;
                Err(WriteError::InvalidFrame(error).into())
            }
            Ok(Err(error)) => {
                self.quarantine();
                Err(error.into())
            }
            Err(error) => {
                self.queue.acknowledge_incomplete(batch.id, 0)?;
                self.phase = Phase::Recovery;
                Err(PumpError::Rejected(error))
            }
        }
    }
    /// Drop the unique backend BEFORE clearing uncertain state. Runtime releases
    /// physical capture first. No attach/resume exists on a destroyed pump.
    pub fn destroy_backend(&mut self) -> Result<(), PumpError> {
        self.queue.quarantine();
        drop(self.backend.take());
        self.queue.backend_destroyed()?;
        self.phase = Phase::Detached;
        Ok(())
    }
}
#[cfg(target_os = "linux")]
impl FrameBackend for crate::uinput::UinputDevice {
    fn input_syspath(&self) -> Option<&std::path::Path> {
        Some(self.input_syspath())
    }
    fn spec(&self) -> &DeviceSpec {
        self.spec()
    }
    fn submit(
        &mut self,
        edges: &[NativeKey],
        recovery: bool,
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        if recovery {
            self.release_submitted(edges)
        } else {
            self.emit(edges)
        }
    }
    fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        self.flush_report()
    }
}
