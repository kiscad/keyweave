use super::*;
use crate::{
    Condition, ConfigSnapshot, Truth,
    rule_selection::{MappingStage, RuleSelection, freeze_header},
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};
pub const MAX_ARBITER_RULES: usize = 4096;
pub const MAX_ARBITRATION_WORK: usize = 64;
pub enum ArbitrationChoice {
    Yield {
        deadline: Option<MonoTime>,
        buffered: usize,
    },
    Pending {
        deadline: MonoTime,
    },
    Matched {
        selection: RuleSelection,
        group: ChordMatch,
        tail: Vec<DueEvent<PlannedAction>>,
    },
    Ordinary {
        selection: RuleSelection,
        inputs: Vec<DueEvent<PlannedAction>>,
    },
    Pass {
        inputs: Vec<DueEvent<PlannedAction>>,
    },
}
/// `at` is the current generating clock; returned raw receipt histories are unchanged.
pub struct Arbitration {
    pub at: MonoTime,
    pub choice: ArbitrationChoice,
}
pub struct ChordArbiter {
    config: Arc<ConfigSnapshot>,
    first_key: KeyId,
    cursor: usize,
    window: Option<ChordWindow>,
    pending: VecDeque<DueEvent<PlannedAction>>,
    now: MonoTime,
    last_earliest: MonoTime,
    finished: bool,
}
impl ChordArbiter {
    /// The original first receipt's Arc is authoritative, including across reload.
    pub fn new(first: DueEvent<PlannedAction>) -> Result<Self, Box<RejectedChordEvent>> {
        if !valid(&first) || first.event.payload.edge != KeyEdge::Down {
            return Err(Box::new(RejectedChordEvent {
                error: ChordError::Input,
                event: first,
            }));
        }
        let config: Arc<ConfigSnapshot> = first.event.config.clone();
        if config.complex.len() > MAX_ARBITER_RULES
            || config
                .complex
                .iter()
                .filter_map(|mapping| mapping.extras.simultaneous.as_ref())
                .any(|spec| spec.validate().is_err())
        {
            return Err(Box::new(RejectedChordEvent {
                error: ChordError::Definition,
                event: first,
            }));
        }
        let Some(first_key) = key(&first) else {
            return Err(Box::new(RejectedChordEvent {
                error: ChordError::Input,
                event: first,
            }));
        };
        let now = first.delivered_at;
        let last_earliest = first.event.at_ns;
        Ok(Self {
            config,
            first_key,
            cursor: 0,
            window: None,
            pending: VecDeque::from([first]),
            now,
            last_earliest,
            finished: false,
        })
    }
    pub fn buffered(&self) -> usize {
        self.pending.len() + self.window.as_ref().map_or(0, ChordWindow::buffered)
    }
    /// Queue only; resolution errors keep accepted inputs in this owner for into_inputs().
    pub fn observe(
        &mut self,
        event: DueEvent<PlannedAction>,
    ) -> Result<(), Box<RejectedChordEvent>> {
        let error = if self.finished {
            Some(ChordError::Finished)
        } else if !valid(&event) {
            Some(ChordError::Input)
        } else if event.delivered_at < self.now || event.event.at_ns < self.last_earliest {
            Some(ChordError::Time)
        } else if self.buffered() >= MAX_CHORD_EVENTS {
            Some(ChordError::Capacity)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(Box::new(RejectedChordEvent { error, event }));
        }
        self.now = event.delivered_at;
        self.last_earliest = event.event.at_ns;
        self.pending.push_back(event);
        Ok(())
    }
    /// One fresh consistent view for rule eligibility and finally selected main actions.
    /// Uncommitted waiting candidates are rechecked; committed selections never rematch.
    pub fn advance(
        &mut self,
        now: MonoTime,
        frontier: Option<MonoTime>,
        modifiers: &BTreeSet<KeyId>,
        evaluate: impl FnMut(&Condition) -> Truth,
    ) -> Result<Arbitration, ChordError> {
        self.advance_with_budget(now, frontier, modifiers, evaluate, MAX_ARBITRATION_WORK)
    }
    pub fn advance_with_budget(
        &mut self,
        now: MonoTime,
        frontier: Option<MonoTime>,
        modifiers: &BTreeSet<KeyId>,
        mut evaluate: impl FnMut(&Condition) -> Truth,
        budget: usize,
    ) -> Result<Arbitration, ChordError> {
        if budget == 0 || budget > MAX_ARBITRATION_WORK {
            return Err(ChordError::Definition);
        }
        if self.finished {
            return Err(ChordError::Finished);
        }
        if now < self.now {
            return Err(ChordError::Time);
        }
        self.now = now;
        let mut work = 0;
        loop {
            if work == budget {
                return Ok(self.yield_work());
            }
            // A resumed window's eligibility check belongs to its next receipt step.
            // Charging a separate search step here would starve budget=1 forever.
            if self.window.is_none() || self.pending.is_empty() {
                work += 1;
            }
            let Some(mapping) = self.config.complex.get(self.cursor) else {
                self.finished = true;
                return Ok(Arbitration {
                    at: self.now,
                    choice: ArbitrationChoice::Pass {
                        inputs: self.pending.drain(..).collect(),
                    },
                });
            };
            let relevant = mapping
                .extras
                .simultaneous
                .as_ref()
                .map_or(mapping.from == self.first_key, |spec| {
                    spec.keys.contains(&self.first_key)
                });
            let header = if relevant {
                freeze_header(
                    self.config.clone(),
                    MappingStage::Complex,
                    self.cursor,
                    modifiers,
                    &mut evaluate,
                )
            } else {
                None
            };
            let Some(header) = header else {
                self.restore_window();
                self.cursor += 1;
                continue;
            };
            if header.mapping().extras.simultaneous.is_none() {
                self.finished = true;
                return Ok(Arbitration {
                    at: self.now,
                    choice: ArbitrationChoice::Ordinary {
                        selection: header.select(&mut evaluate),
                        inputs: self.pending.drain(..).collect(),
                    },
                });
            }
            if self.window.is_none() {
                let first = self.pending.pop_front().ok_or(ChordError::Input)?;
                let spec = header
                    .mapping()
                    .extras
                    .simultaneous
                    .as_ref()
                    .ok_or(ChordError::Definition)?
                    .clone();
                match ChordWindow::new(spec, first) {
                    Ok(window) => self.window = Some(window),
                    Err(rejected) => {
                        self.pending.push_front(rejected.event);
                        return Err(rejected.error);
                    }
                }
            }
            let mut decision = None;
            while !self.pending.is_empty() {
                if work == budget {
                    return Ok(self.yield_work());
                }
                work += 1;
                let event = self.pending.pop_front().ok_or(ChordError::Input)?;
                let window = self.window.as_mut().ok_or(ChordError::Finished)?;
                match window.observe(event) {
                    Ok(next) => {
                        if !matches!(next, ChordDecision::Pending { .. }) {
                            decision = Some(next);
                            break;
                        }
                    }
                    Err(rejected) => {
                        let boundary = rejected.error == ChordError::Boundary;
                        let at = rejected.event.event.at_ns;
                        self.pending.push_front(rejected.event);
                        if !boundary {
                            return Err(rejected.error);
                        }
                        decision = Some(window.advance(self.now, Some(at))?);
                        break;
                    }
                }
            }
            let next = match decision {
                Some(next) => next,
                None => self
                    .window
                    .as_mut()
                    .ok_or(ChordError::Finished)?
                    .advance(self.now, frontier)?,
            };
            match next {
                ChordDecision::Pending { deadline, .. } => {
                    return Ok(Arbitration {
                        at: self.now,
                        choice: ArbitrationChoice::Pending { deadline },
                    });
                }
                ChordDecision::Matched(group) => {
                    self.window = None;
                    self.finished = true;
                    return Ok(Arbitration {
                        at: self.now,
                        choice: ArbitrationChoice::Matched {
                            selection: header.select(&mut evaluate),
                            group,
                            tail: self.pending.drain(..).collect(),
                        },
                    });
                }
                ChordDecision::Replay(events) => {
                    self.window = None;
                    self.cursor += 1;
                    let mut restored = VecDeque::from(events);
                    restored.append(&mut self.pending);
                    self.pending = restored;
                }
            }
        }
    }
    fn yield_work(&self) -> Arbitration {
        Arbitration {
            at: self.now,
            choice: ArbitrationChoice::Yield {
                deadline: self.window.as_ref().map(ChordWindow::deadline),
                buffered: self.buffered(),
            },
        }
    }
    fn restore_window(&mut self) {
        if let Some(window) = self.window.take() {
            let mut restored = VecDeque::from(window.into_buffer());
            restored.append(&mut self.pending);
            self.pending = restored;
        }
    }
    /// Consume the owner on failure/control; returns every still-owned raw input once.
    pub fn into_inputs(mut self) -> Vec<DueEvent<PlannedAction>> {
        self.restore_window();
        self.pending.into_iter().collect()
    }
}
