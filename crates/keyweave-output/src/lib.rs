//! Bounded, deterministic output scheduling and a submitted-state ledger.
//! The caller owns the clock, backend, capture policy, and neutral activation gate.
use keyweave_types::{KeyEdge, KeyId, MonoTime, OutputIntent};
use std::collections::{BTreeSet, VecDeque};
use thiserror::Error;

pub const MAX_QUEUED_EDGES: usize = 8192;
pub const MAX_ENQUEUE_EDGES: usize = 1024;
pub const MAX_SUBMISSION_EDGES: usize = 256;
pub const MAX_PRESSED_KEYS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Paused,
    Running,
    Recovering,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmissionId(u64);

/// A single ordered backend submission. Acknowledge its known accepted prefix.
/// One edge is one semantic event, not one byte or one native packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    pub id: SubmissionId,
    pub edges: Vec<OutputIntent>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acknowledgment {
    Complete,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum OutputError {
    #[error("output is paused or recovering")]
    NotRunning,
    #[error("a backend submission is still in flight")]
    InFlight,
    #[error("output state is not neutral")]
    NotNeutral,
    #[error("bounded output capacity exceeded")]
    Capacity,
    #[error("invalid desired key transition")]
    InvalidTransition,
    #[error("clock moved backwards")]
    TimeReversed,
    #[error("stale submission identity or invalid accepted prefix")]
    InvalidAcknowledgment,
    #[error("submission identity space exhausted")]
    IdentityExhausted,
}

/// This ledger tracks API-accepted events, never delivery to a target application.
/// A short normal submission discards all unsubmitted plans and enters recovery.
/// Recovery retries only known still-submitted downs, then pauses for reactivation.
pub struct OutputQueue {
    mode: Mode,
    queue: VecDeque<OutputIntent>,
    desired: BTreeSet<KeyId>,
    submitted: BTreeSet<KeyId>,
    flight: Option<(SubmissionId, usize)>,
    next_id: u64,
    now: MonoTime,
}
impl Default for OutputQueue {
    fn default() -> Self {
        Self::new()
    }
}
impl OutputQueue {
    pub fn new() -> Self {
        Self {
            mode: Mode::Paused,
            queue: VecDeque::new(),
            desired: BTreeSet::new(),
            submitted: BTreeSet::new(),
            flight: None,
            next_id: 1,
            now: MonoTime(0),
        }
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn queued_edges(&self) -> usize {
        self.queue.len()
    }
    pub fn desired(&self) -> &BTreeSet<KeyId> {
        &self.desired
    }
    pub fn submitted(&self) -> &BTreeSet<KeyId> {
        &self.submitted
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.queue.front().map(|edge| edge.at_ns)
    }
    /// Call only after the runtime's physical neutral/capture gate succeeds.
    pub fn resume(&mut self) -> Result<(), OutputError> {
        if self.flight.is_some() {
            return Err(OutputError::InFlight);
        }
        if !self.queue.is_empty() || !self.submitted.is_empty() || !self.desired.is_empty() {
            return Err(OutputError::NotNeutral);
        }
        self.mode = Mode::Running;
        Ok(())
    }
    /// Atomic append. Timestamps are earliest dispatch times; FIFO never overtakes
    /// an earlier queued edge. Later physical events wait behind scheduled taps.
    pub fn enqueue(&mut self, edges: &[OutputIntent]) -> Result<(), OutputError> {
        if self.mode != Mode::Running {
            return Err(OutputError::NotRunning);
        }
        if edges.len() > MAX_ENQUEUE_EDGES
            || self.queue.len().saturating_add(edges.len()) > MAX_QUEUED_EDGES
        {
            return Err(OutputError::Capacity);
        }
        let mut desired = self.desired.clone();
        for edge in edges {
            match edge.edge {
                KeyEdge::Down if desired.insert(edge.key) => {}
                KeyEdge::Up if desired.remove(&edge.key) => {}
                KeyEdge::Repeat if desired.contains(&edge.key) => {}
                _ => return Err(OutputError::InvalidTransition),
            }
            if desired.len() > MAX_PRESSED_KEYS {
                return Err(OutputError::Capacity);
            }
        }
        let mut at = self.queue.back().map_or(self.now, |edge| edge.at_ns);
        for edge in edges {
            at = at.max(edge.at_ns);
            self.queue.push_back(OutputIntent {
                at_ns: at,
                ..edge.clone()
            });
        }
        self.desired = desired;
        Ok(())
    }
    fn advance(&mut self, now: MonoTime) -> Result<(), OutputError> {
        if now < self.now {
            return Err(OutputError::TimeReversed);
        }
        self.now = now;
        Ok(())
    }
    pub fn begin_due(&mut self, now: MonoTime) -> Result<Option<Submission>, OutputError> {
        if self.flight.is_some() {
            return Err(OutputError::InFlight);
        }
        if now < self.now {
            return Err(OutputError::TimeReversed);
        }
        let edges: Vec<_> = self
            .queue
            .iter()
            .take_while(|edge| edge.at_ns <= now)
            .take(MAX_SUBMISSION_EDGES)
            .cloned()
            .collect();
        if edges.is_empty() {
            self.advance(now)?;
            return Ok(None);
        }
        let next = self
            .next_id
            .checked_add(1)
            .ok_or(OutputError::IdentityExhausted)?;
        let id = SubmissionId(self.next_id);
        self.next_id = next;
        self.advance(now)?;
        self.flight = Some((id, edges.len()));
        Ok(Some(Submission { id, edges }))
    }
    /// Backends must report only a known, ordered prefix. An ambiguous write is
    /// a separate backend quarantine condition and cannot be guessed here.
    pub fn acknowledge(
        &mut self,
        id: SubmissionId,
        accepted: usize,
    ) -> Result<Acknowledgment, OutputError> {
        let Some((current, count)) = self.flight else {
            return Err(OutputError::InvalidAcknowledgment);
        };
        if current != id || accepted > count {
            return Err(OutputError::InvalidAcknowledgment);
        }
        for _ in 0..accepted {
            if let Some(edge) = self.queue.pop_front() {
                match edge.edge {
                    KeyEdge::Down => {
                        self.submitted.insert(edge.key);
                    }
                    KeyEdge::Up => {
                        self.submitted.remove(&edge.key);
                    }
                    KeyEdge::Repeat => {}
                }
            }
        }
        self.flight = None;
        if accepted < count {
            if self.mode == Mode::Running {
                self.recover();
            }
            return Ok(Acknowledgment::RecoveryRequired);
        }
        if self.mode == Mode::Recovering && self.queue.is_empty() {
            self.mode = Mode::Paused;
        }
        Ok(Acknowledgment::Complete)
    }
    /// Stop capture before requesting recovery. A live submission must first be
    /// reconciled; no reset may pretend unknown API acceptance was zero.
    pub fn begin_recovery(&mut self, now: MonoTime) -> Result<(), OutputError> {
        if self.flight.is_some() {
            return Err(OutputError::InFlight);
        }
        self.advance(now)?;
        self.recover();
        Ok(())
    }
    fn recover(&mut self) {
        self.queue.clear();
        self.desired.clear();
        for key in &self.submitted {
            self.queue.push_back(OutputIntent {
                at_ns: self.now,
                key: *key,
                edge: KeyEdge::Up,
            });
        }
        self.mode = if self.queue.is_empty() {
            Mode::Paused
        } else {
            Mode::Recovering
        };
    }
}
