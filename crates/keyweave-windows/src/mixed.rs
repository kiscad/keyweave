//! Opt-in mixed output FIFO. Caller owns capture stop and recovery budget.
use crate::{
    keys::{self, InjectionTag, KeyboardPacket, MAX_BATCH},
    pointer::{self, MouseAction, MousePacket, PointerError},
    pump::{Insertion, Phase, SubmitError},
};
use keyweave_output::{MAX_ENQUEUE_EDGES, MAX_QUEUED_EDGES, Mode, OutputError, OutputQueue};
use keyweave_types::{BUTTON_PAGE, MonoTime, OutputIntent};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativePacket {
    Keyboard(KeyboardPacket),
    Mouse(MousePacket),
}
pub trait SendMixedBackend {
    /// Rejected means no API call. Unknown acceptance must not be treated as 0.
    fn submit_mixed(&mut self, packets: &[NativePacket]) -> Result<Insertion, SubmitError>;
}
#[derive(Clone, Debug)]
pub enum TimedOutput {
    Key(OutputIntent),
    Mouse { at: MonoTime, action: MouseAction },
}
#[derive(Debug, thiserror::Error)]
pub enum MixedError {
    #[error(transparent)]
    Queue(#[from] OutputError),
    #[error(transparent)]
    Key(#[from] keys::KeyError),
    #[error(transparent)]
    Pointer(#[from] PointerError),
    #[error(transparent)]
    Submit(#[from] SubmitError),
    #[error("mixed output is paused, stopped or closed")]
    NotReady,
    #[error("invalid mixed output receipt")]
    Receipt,
}
enum Work {
    Edges(usize),
    Impulse { at: MonoTime, packet: MousePacket },
}
pub struct MixedOutputPump<B: SendMixedBackend> {
    backend: Option<B>,
    tag: InjectionTag,
    queue: OutputQueue,
    phase: Phase,
    work: VecDeque<Work>,
    impulses: usize,
    now: MonoTime,
    tail: MonoTime,
}
fn encode_edge(edge: &OutputIntent, tag: InjectionTag) -> Result<NativePacket, MixedError> {
    if edge.key.page == BUTTON_PAGE {
        Ok(NativePacket::Mouse(pointer::encode(
            MouseAction::Button {
                key: edge.key,
                edge: edge.edge,
            },
            tag,
        )?))
    } else {
        let packet = keys::encode_batch(std::slice::from_ref(edge), tag)?;
        Ok(NativePacket::Keyboard(packet[0]))
    }
}
impl<B: SendMixedBackend> MixedOutputPump<B> {
    pub fn new(backend: B, tag: InjectionTag) -> Self {
        Self {
            backend: Some(backend),
            tag,
            queue: OutputQueue::new(),
            phase: Phase::Paused,
            work: VecDeque::new(),
            impulses: 0,
            now: MonoTime(0),
            tail: MonoTime(0),
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    /// Quarantine/closed keeps the last known accepted prefix, not full state.
    pub fn ledger(&self) -> &OutputQueue {
        &self.queue
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        match self.work.front() {
            Some(Work::Impulse { at, .. }) => Some(*at),
            _ => self.queue.next_deadline(),
        }
    }
    /// Caller proves session/delivery access and physical neutral first.
    pub fn resume(&mut self) -> Result<(), MixedError> {
        if self.phase != Phase::Paused || !self.work.is_empty() {
            return Err(MixedError::NotReady);
        }
        self.queue.resume()?;
        self.phase = Phase::Normal;
        Ok(())
    }
    pub fn enqueue(&mut self, outputs: &[TimedOutput]) -> Result<(), MixedError> {
        if self.phase != Phase::Normal {
            return Err(MixedError::NotReady);
        }
        if outputs.len() > MAX_ENQUEUE_EDGES
            || self.queue.queued_edges() + self.impulses + outputs.len() > MAX_QUEUED_EDGES
        {
            return Err(OutputError::Capacity.into());
        }
        let mut edges = Vec::new();
        let mut work = VecDeque::new();
        let mut tail = self.tail.max(self.now);
        let mut impulses = 0;
        for output in outputs {
            let (at, edge, impulse) = match output {
                TimedOutput::Key(edge) => (edge.at_ns, Some(edge.clone()), None),
                TimedOutput::Mouse {
                    at,
                    action: MouseAction::Button { key, edge },
                } => (
                    *at,
                    Some(OutputIntent {
                        at_ns: *at,
                        key: *key,
                        edge: *edge,
                    }),
                    None,
                ),
                TimedOutput::Mouse { at, action } => {
                    (*at, None, Some(pointer::encode(*action, self.tag)?))
                }
            };
            tail = tail.max(at);
            if let Some(mut edge) = edge {
                edge.at_ns = tail;
                encode_edge(&edge, self.tag)?;
                edges.push(edge);
                if let Some(Work::Edges(count)) = work.back_mut() {
                    *count += 1;
                } else {
                    work.push_back(Work::Edges(1));
                }
            } else if let Some(packet) = impulse {
                work.push_back(Work::Impulse { at: tail, packet });
                impulses += 1;
            }
        }
        self.queue.enqueue(&edges)?;
        self.work.extend(work);
        self.impulses += impulses;
        self.tail = tail;
        Ok(())
    }
    fn clear_work(&mut self) {
        self.work.clear();
        self.impulses = 0;
    }
    fn quarantine(&mut self) {
        self.queue.quarantine();
        self.clear_work();
        self.phase = Phase::Quarantined;
    }
    /// Stop capture first; discard impulses and recover only accepted keys/buttons.
    pub fn begin_recovery(&mut self, now: MonoTime) -> Result<(), MixedError> {
        if !matches!(
            self.phase,
            Phase::Normal | Phase::StopRequired | Phase::Recovery | Phase::Paused
        ) {
            return Err(MixedError::NotReady);
        }
        if now < self.now {
            return Err(OutputError::TimeReversed.into());
        }
        self.queue.begin_recovery(now)?;
        self.now = now;
        self.tail = now;
        self.clear_work();
        self.phase = if self.queue.mode() == Mode::Paused {
            Phase::Paused
        } else {
            Phase::Recovery
        };
        Ok(())
    }
    pub fn poll(&mut self, now: MonoTime) -> Result<Option<Insertion>, MixedError> {
        if !matches!(self.phase, Phase::Normal | Phase::Recovery | Phase::Paused) {
            return Err(MixedError::NotReady);
        }
        if now < self.now {
            return Err(OutputError::TimeReversed.into());
        }
        self.now = now;
        if self.phase == Phase::Normal && matches!(self.work.front(), Some(Work::Impulse { .. })) {
            let Some(Work::Impulse { at, packet }) = self.work.front() else {
                unreachable!()
            };
            if *at > now {
                return Ok(None);
            }
            let packet = NativePacket::Mouse(*packet);
            let result = self
                .backend
                .as_mut()
                .ok_or(MixedError::NotReady)?
                .submit_mixed(&[packet]);
            self.work.pop_front();
            self.impulses -= 1;
            return match result {
                Ok(receipt) if receipt.accepted <= 1 => {
                    if receipt.accepted == 0 {
                        self.phase = Phase::StopRequired;
                    }
                    Ok(Some(receipt))
                }
                Ok(_) => {
                    self.quarantine();
                    Err(MixedError::Receipt)
                }
                Err(SubmitError::Rejected) => {
                    self.phase = Phase::StopRequired;
                    Err(SubmitError::Rejected.into())
                }
                Err(e) => {
                    self.quarantine();
                    Err(e.into())
                }
            };
        }
        let limit = if self.phase == Phase::Normal {
            match self.work.front() {
                Some(Work::Edges(n)) => *n,
                _ => 0,
            }
        } else {
            MAX_BATCH
        };
        let Some(batch) = self.queue.begin_due_limited(now, limit.min(MAX_BATCH))? else {
            return Ok(None);
        };
        let packets = match batch
            .edges
            .iter()
            .map(|e| encode_edge(e, self.tag))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(p) => p,
            Err(e) => {
                self.queue.acknowledge_incomplete(batch.id, 0)?;
                self.phase = Phase::StopRequired;
                return Err(e);
            }
        };
        let result = self
            .backend
            .as_mut()
            .ok_or(MixedError::NotReady)?
            .submit_mixed(&packets);
        match result {
            Ok(receipt) if receipt.accepted <= packets.len() => {
                self.queue.acknowledge(batch.id, receipt.accepted)?;
                if self.phase == Phase::Normal {
                    if let Some(Work::Edges(n)) = self.work.front_mut() {
                        *n -= receipt.accepted;
                        if *n == 0 {
                            self.work.pop_front();
                        }
                    }
                    if receipt.accepted < packets.len() {
                        self.phase = Phase::StopRequired;
                    }
                } else if self.queue.mode() == Mode::Paused {
                    self.phase = Phase::Paused;
                }
                Ok(Some(receipt))
            }
            Ok(_) => {
                self.quarantine();
                Err(MixedError::Receipt)
            }
            Err(SubmitError::Rejected) => {
                self.queue.acknowledge_incomplete(batch.id, 0)?;
                if self.phase == Phase::Normal {
                    self.phase = Phase::StopRequired;
                }
                Err(SubmitError::Rejected.into())
            }
            Err(e) => {
                self.quarantine();
                Err(e.into())
            }
        }
    }
    /// Backend destruction cannot release global Windows input state.
    pub fn close(&mut self) {
        self.queue.quarantine();
        self.clear_work();
        drop(self.backend.take());
        self.phase = Phase::Closed;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use keyweave_types::{KeyEdge, KeyId};
    use std::{cell::RefCell, rc::Rc};
    type Log = Rc<RefCell<Vec<Vec<NativePacket>>>>;
    struct Backend {
        log: Log,
        replies: VecDeque<Result<Insertion, SubmitError>>,
    }
    impl SendMixedBackend for Backend {
        fn submit_mixed(&mut self, packets: &[NativePacket]) -> Result<Insertion, SubmitError> {
            self.log.borrow_mut().push(packets.to_vec());
            self.replies.pop_front().unwrap_or(Ok(Insertion {
                accepted: packets.len(),
                last_error: None,
            }))
        }
    }
    fn pump(replies: Vec<Result<Insertion, SubmitError>>) -> (MixedOutputPump<Backend>, Log) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut pump = MixedOutputPump::new(
            Backend {
                log: log.clone(),
                replies: replies.into(),
            },
            InjectionTag::new(19).unwrap(),
        );
        pump.resume().unwrap();
        (pump, log)
    }
    fn button(usage: u16, edge: KeyEdge, at: u64) -> TimedOutput {
        TimedOutput::Mouse {
            at: MonoTime(at),
            action: MouseAction::Button {
                key: KeyId::new(BUTTON_PAGE, usage),
                edge,
            },
        }
    }
    fn motion(at: u64) -> TimedOutput {
        TimedOutput::Mouse {
            at: MonoTime(at),
            action: MouseAction::Motion { dx: 7, dy: -9 },
        }
    }
    fn key(edge: KeyEdge, at: u64) -> TimedOutput {
        TimedOutput::Key(OutputIntent {
            at_ns: MonoTime(at),
            key: KeyId::new(7, 4),
            edge,
        })
    }
    #[test]
    fn key_button_impulse_order_and_deadlines_share_one_fifo() {
        let (mut p, log) = pump(vec![]);
        p.enqueue(&[
            key(KeyEdge::Down, 10),
            button(1, KeyEdge::Down, 5),
            motion(2),
            key(KeyEdge::Up, 1),
            button(1, KeyEdge::Up, 1),
        ])
        .unwrap();
        assert_eq!(p.next_deadline(), Some(MonoTime(10)));
        assert!(p.poll(MonoTime(9)).unwrap().is_none());
        p.poll(MonoTime(10)).unwrap();
        p.poll(MonoTime(10)).unwrap();
        p.poll(MonoTime(10)).unwrap();
        let calls = log.borrow();
        assert_eq!(calls.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1, 2]);
        assert!(matches!(calls[0][0], NativePacket::Keyboard(_)));
        assert!(matches!(calls[0][1],NativePacket::Mouse(packet) if packet.flags()==2));
        assert!(matches!(calls[1][0],NativePacket::Mouse(packet) if packet.motion()==(7,-9)));
        assert!(p.ledger().submitted().is_empty());
        assert_eq!(p.next_deadline(), None);
    }
    #[test]
    fn short_prefix_recovers_only_accepted_button_and_discards_impulses() {
        let (mut p, log) = pump(vec![Ok(Insertion {
            accepted: 1,
            last_error: Some(5),
        })]);
        p.enqueue(&[
            button(1, KeyEdge::Down, 0),
            key(KeyEdge::Down, 0),
            motion(0),
        ])
        .unwrap();
        p.poll(MonoTime(0)).unwrap();
        assert_eq!(p.phase(), Phase::StopRequired);
        assert_eq!(
            p.ledger().submitted().iter().copied().collect::<Vec<_>>(),
            [KeyId::new(BUTTON_PAGE, 1)]
        );
        assert!(p.poll(MonoTime(0)).is_err());
        p.begin_recovery(MonoTime(0)).unwrap();
        p.poll(MonoTime(0)).unwrap();
        assert_eq!(p.phase(), Phase::Paused);
        assert!(p.ledger().submitted().is_empty());
        assert!(matches!(log.borrow()[1][0],NativePacket::Mouse(packet) if packet.flags()==4));
        assert_eq!(log.borrow().len(), 2);
    }
    #[test]
    fn zero_impulse_acceptance_never_replays_motion_or_wheel() {
        for action in [
            MouseAction::Motion { dx: 1, dy: 2 },
            MouseAction::Wheel {
                horizontal: true,
                delta: -120,
            },
        ] {
            let (mut p, log) = pump(vec![
                Ok(Insertion {
                    accepted: 1,
                    last_error: None,
                }),
                Ok(Insertion {
                    accepted: 0,
                    last_error: Some(0),
                }),
            ]);
            p.enqueue(&[
                button(5, KeyEdge::Down, 0),
                TimedOutput::Mouse {
                    at: MonoTime(0),
                    action,
                },
                motion(10),
            ])
            .unwrap();
            p.poll(MonoTime(0)).unwrap();
            p.poll(MonoTime(0)).unwrap();
            p.begin_recovery(MonoTime(0)).unwrap();
            p.poll(MonoTime(0)).unwrap();
            assert_eq!(log.borrow().len(), 3);
            assert!(
                matches!(log.borrow()[2][0],NativePacket::Mouse(packet) if packet.flags()==0x100&&packet.data()==2)
            );
            assert_eq!(p.phase(), Phase::Paused);
        }
    }
    #[test]
    fn unknown_and_invalid_receipts_preserve_known_ledger_without_recovery() {
        for reply in [
            Err(SubmitError::Unknown),
            Ok(Insertion {
                accepted: 2,
                last_error: None,
            }),
        ] {
            let (mut p, log) = pump(vec![
                Ok(Insertion {
                    accepted: 1,
                    last_error: None,
                }),
                reply,
            ]);
            p.enqueue(&[button(2, KeyEdge::Down, 0), motion(0)])
                .unwrap();
            p.poll(MonoTime(0)).unwrap();
            assert!(p.poll(MonoTime(0)).is_err());
            assert_eq!(p.phase(), Phase::Quarantined);
            assert_eq!(p.ledger().submitted().len(), 1);
            assert!(p.begin_recovery(MonoTime(0)).is_err());
            p.close();
            assert_eq!(p.ledger().submitted().len(), 1);
            assert_eq!(log.borrow().len(), 2);
        }
    }
    #[test]
    fn append_failure_is_atomic_and_total_capacity_is_bounded() {
        let (mut p, log) = pump(vec![]);
        assert!(
            p.enqueue(&[button(1, KeyEdge::Down, 0), button(6, KeyEdge::Down, 0)])
                .is_err()
        );
        assert!(p.ledger().desired().is_empty());
        assert_eq!(p.next_deadline(), None);
        assert!(log.borrow().is_empty());
        for _ in 0..8 {
            p.enqueue(&vec![motion(0); MAX_ENQUEUE_EDGES]).unwrap();
        }
        assert!(p.enqueue(&[key(KeyEdge::Down, 0)]).is_err());
        assert!(p.ledger().desired().is_empty());
        p.begin_recovery(MonoTime(0)).unwrap();
        assert_eq!(p.phase(), Phase::Paused);
        assert_eq!(p.next_deadline(), None);
        p.resume().unwrap();
        assert!(p.enqueue(&[button(1, KeyEdge::Up, 0)]).is_err());
        assert!(p.ledger().desired().is_empty());
    }
    #[test]
    fn recovery_short_prefix_and_rejection_keep_only_remaining_ups() {
        let (mut p, log) = pump(vec![
            Ok(Insertion {
                accepted: 2,
                last_error: None,
            }),
            Ok(Insertion {
                accepted: 1,
                last_error: Some(5),
            }),
            Err(SubmitError::Rejected),
        ]);
        p.enqueue(&[button(1, KeyEdge::Down, 0), button(2, KeyEdge::Down, 0)])
            .unwrap();
        p.poll(MonoTime(0)).unwrap();
        p.begin_recovery(MonoTime(1)).unwrap();
        p.poll(MonoTime(1)).unwrap();
        assert_eq!(p.ledger().submitted().len(), 1);
        assert!(p.poll(MonoTime(1)).is_err());
        assert_eq!(p.ledger().submitted().len(), 1);
        p.poll(MonoTime(1)).unwrap();
        assert_eq!(p.phase(), Phase::Paused);
        assert!(matches!(log.borrow()[3][0],NativePacket::Mouse(packet) if packet.flags()==16));
        assert!(p.poll(MonoTime(0)).is_err());
    }
}
