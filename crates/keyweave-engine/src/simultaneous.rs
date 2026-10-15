//! Bounded Complex-stage lookahead. Matching is separate from output/binding ownership.
mod arbiter;
mod owner;
use crate::{
    Action, ActionTarget,
    action_plan::PlannedAction,
    timeline::{DueEvent, Stage, StageEvent},
};
pub use arbiter::{
    Arbitration, ArbitrationChoice, ChordArbiter, MAX_ARBITER_RULES, MAX_ARBITRATION_WORK,
};
use keyweave_types::{BUTTON_PAGE, KEYBOARD_PAGE, KeyEdge, KeyId, MonoTime};
pub use owner::{
    GroupBindingOwner, GroupBranchAdmission, GroupOwnerError, RejectedGroupOwner,
    UnfinishedGroupOwner,
};
use thiserror::Error;

pub(crate) fn is_chord_key(key: KeyId) -> bool {
    key.page == KEYBOARD_PAGE || key.page == BUTTON_PAGE && (1..=32).contains(&key.usage)
}
pub const MAX_CHORD_KEYS: usize = 32;
pub const MAX_CHORD_EVENTS: usize = 256;
pub const MAX_CHORD_THRESHOLD_NS: u64 = 1_000_000_000;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyOrder {
    #[default]
    Insensitive,
    Strict,
    StrictInverse,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyUpWhen {
    #[default]
    Any,
    All,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Simultaneous {
    pub keys: Vec<KeyId>,
    pub threshold_ns: u64,
    pub detect_key_down_uninterruptedly: bool,
    pub key_down_order: KeyOrder,
    pub key_up_order: KeyOrder,
    pub key_up_when: KeyUpWhen,
    pub after_key_up: Vec<Action>,
}
impl Simultaneous {
    pub fn new(keys: Vec<KeyId>) -> Self {
        Self {
            keys,
            threshold_ns: 50_000_000,
            detect_key_down_uninterruptedly: false,
            key_down_order: KeyOrder::Insensitive,
            key_up_order: KeyOrder::Insensitive,
            key_up_when: KeyUpWhen::Any,
            after_key_up: vec![],
        }
    }
    pub fn validate(&self) -> Result<(), ChordError> {
        if self.keys.is_empty()
            || self.keys.len() > MAX_CHORD_KEYS
            || self.keys.iter().any(|key| !is_chord_key(*key))
            || self
                .keys
                .iter()
                .enumerate()
                .any(|(i, key)| self.keys[..i].contains(key))
            || self.threshold_ns > MAX_CHORD_THRESHOLD_NS
        {
            return Err(ChordError::Definition);
        }
        let slots = self
            .after_key_up
            .iter()
            .map(|action| 1usize.saturating_add(action.modifiers.len()))
            .fold(0usize, usize::saturating_add);
        if slots.saturating_mul(2) > crate::MAX_OUTPUTS_PER_EVENT
            || self
                .after_key_up
                .iter()
                .any(|action| action.hold_down_ns > crate::action_plan::MAX_ACTION_DURATION_NS)
        {
            return Err(ChordError::Definition);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ChordError {
    #[error("invalid bounded keyboard/button simultaneous definition")]
    Definition,
    #[error("invalid Complex input receipt")]
    Input,
    #[error("chord clock moved backwards")]
    Time,
    #[error("chord deadline overflows monotonic time")]
    Overflow,
    #[error("chord input buffer is full")]
    Capacity,
    #[error("resolve the window before the next input outside it")]
    Boundary,
    #[error("duplicate chord source edge")]
    Duplicate,
    #[error("chord source provenance changed")]
    Provenance,
    #[error("receipt belongs to another source")]
    Outside,
    #[error("chord window has already returned its decision")]
    Finished,
}
#[derive(Debug)]
pub struct RejectedChordEvent {
    pub error: ChordError,
    pub event: DueEvent<PlannedAction>,
}
#[derive(Clone, Debug)]
pub struct ChordObservation {
    pub event: StageEvent<PlannedAction>,
    pub delivered_at: MonoTime,
}
impl ChordObservation {
    fn from_receipt(receipt: &DueEvent<PlannedAction>) -> Self {
        Self {
            event: receipt.event.clone(),
            delivered_at: receipt.delivered_at,
        }
    }
}
#[derive(Clone, Debug)]
pub struct ChordMember {
    down: ChordObservation,
    up: Option<ChordObservation>,
}
impl ChordMember {
    pub fn down(&self) -> &ChordObservation {
        &self.down
    }
    pub fn up(&self) -> Option<&ChordObservation> {
        self.up.as_ref()
    }
}
#[derive(Debug)]
pub struct ChordMatch {
    spec: Simultaneous,
    members: Vec<ChordMember>,
    events: Vec<DueEvent<PlannedAction>>,
    at: MonoTime,
}
impl ChordMatch {
    pub fn members(&self) -> &[ChordMember] {
        &self.members
    }
    pub fn events(&self) -> &[DueEvent<PlannedAction>] {
        &self.events
    }
    pub fn matched_at(&self) -> MonoTime {
        self.at
    }
    pub fn spec(&self) -> &Simultaneous {
        &self.spec
    }
    pub fn into_active(self) -> (ActiveChord, Vec<DueEvent<PlannedAction>>) {
        (
            ActiveChord {
                spec: self.spec,
                members: self.members,
                now: self.at,
                output_released: false,
                after_up_reported: false,
            },
            self.events,
        )
    }
}
#[derive(Debug)]
pub enum ChordDecision {
    Pending { deadline: MonoTime, buffered: usize },
    Matched(ChordMatch),
    Replay(Vec<DueEvent<PlannedAction>>),
}
pub struct ChordWindow {
    spec: Simultaneous,
    events: Vec<DueEvent<PlannedAction>>,
    members: Vec<ChordMember>,
    up_order: Vec<KeyId>,
    end: MonoTime,
    now: MonoTime,
    failed: bool,
    finished: bool,
}
fn key(event: &DueEvent<PlannedAction>) -> Option<KeyId> {
    key_event(&event.event)
}
fn key_event(event: &StageEvent<PlannedAction>) -> Option<KeyId> {
    match event.payload.target {
        ActionTarget::Key(key) => Some(key),
        _ => None,
    }
}
fn identity(a: &StageEvent<PlannedAction>, b: &StageEvent<PlannedAction>) -> bool {
    a.device == b.device && a.source == b.source && a.payload.slot == b.payload.slot
}
fn provenance(a: &StageEvent<PlannedAction>, b: &StageEvent<PlannedAction>) -> bool {
    identity(a, b)
        && a.stage == b.stage
        && key_event(a) == key_event(b)
        && std::sync::Arc::ptr_eq(&a.config, &b.config)
}
fn valid(event: &DueEvent<PlannedAction>) -> bool {
    event.event.stage == Stage::Complex
        && event.delivered_at >= event.event.at_ns
        && event.event.device.generation != 0
        && event.event.source.0 != 0
        && event.event.payload.slot.batch.0 != 0
        && usize::from(event.event.payload.slot.index) < 512
        && matches!(event.event.payload.edge, KeyEdge::Down | KeyEdge::Up)
}
fn ordered(actual: impl Iterator<Item = KeyId>, keys: &[KeyId], order: KeyOrder) -> bool {
    match order {
        KeyOrder::Insensitive => true,
        KeyOrder::Strict => actual.zip(keys.iter().copied()).all(|(a, b)| a == b),
        KeyOrder::StrictInverse => actual.zip(keys.iter().rev().copied()).all(|(a, b)| a == b),
    }
}
impl ChordWindow {
    pub(super) fn into_buffer(self) -> Vec<DueEvent<PlannedAction>> {
        self.events
    }
    pub fn new(
        spec: Simultaneous,
        first: DueEvent<PlannedAction>,
    ) -> Result<Self, Box<RejectedChordEvent>> {
        let checked = spec.validate().and_then(|()| {
            if !valid(&first)
                || first.event.payload.edge != KeyEdge::Down
                || !key(&first).is_some_and(|key| spec.keys.contains(&key))
            {
                return Err(ChordError::Input);
            }
            first
                .event
                .at_ns
                .checked_add(spec.threshold_ns)
                .ok_or(ChordError::Overflow)
        });
        let end = match checked {
            Ok(end) => end,
            Err(error) => {
                return Err(Box::new(RejectedChordEvent {
                    error,
                    event: first,
                }));
            }
        };
        let failed = !ordered(key(&first).into_iter(), &spec.keys, spec.key_down_order);
        Ok(Self {
            spec,
            end,
            now: first.delivered_at,
            members: vec![ChordMember {
                down: ChordObservation::from_receipt(&first),
                up: None,
            }],
            events: vec![first],
            up_order: vec![],
            failed,
            finished: false,
        })
    }
    pub fn buffered(&self) -> usize {
        self.events.len()
    }
    pub fn deadline(&self) -> MonoTime {
        self.end
    }
    pub fn observe(
        &mut self,
        event: DueEvent<PlannedAction>,
    ) -> Result<ChordDecision, Box<RejectedChordEvent>> {
        let checked = self.check(&event);
        if let Err(error) = checked {
            return Err(Box::new(RejectedChordEvent { error, event }));
        }
        let Some(target) = key(&event) else {
            self.now = event.delivered_at;
            self.events.push(event);
            return Ok(self.decide(false));
        };
        self.now = event.delivered_at;
        match event.event.payload.edge {
            KeyEdge::Down => {
                if self.spec.keys.contains(&target)
                    && !self
                        .members
                        .iter()
                        .any(|m| key_event(&m.down.event) == Some(target))
                {
                    self.members.push(ChordMember {
                        down: ChordObservation::from_receipt(&event),
                        up: None,
                    });
                    self.failed |= !ordered(
                        self.members.iter().filter_map(|m| key_event(&m.down.event)),
                        &self.spec.keys,
                        self.spec.key_down_order,
                    );
                } else if !self.spec.keys.contains(&target)
                    && !self.spec.detect_key_down_uninterruptedly
                    && self.members.len() < self.spec.keys.len()
                {
                    self.failed = true;
                }
            }
            KeyEdge::Up => {
                if let Some(member) = self
                    .members
                    .iter_mut()
                    .find(|m| identity(&m.down.event, &event.event))
                {
                    member.up = Some(ChordObservation::from_receipt(&event));
                    self.up_order.push(target);
                    self.failed |= self.members.len() < self.spec.keys.len()
                        || !ordered(
                            self.up_order.iter().copied(),
                            &self.spec.keys,
                            self.spec.key_up_order,
                        );
                }
            }
            KeyEdge::Repeat => unreachable!("receipt preflight excludes repeat"),
        }
        self.events.push(event);
        Ok(self.decide(false))
    }
    fn check(&self, event: &DueEvent<PlannedAction>) -> Result<(), ChordError> {
        if self.finished {
            return Err(ChordError::Finished);
        }
        if !valid(event) {
            return Err(ChordError::Input);
        }
        if event.delivered_at < self.now
            || self
                .events
                .last()
                .is_some_and(|last| event.event.at_ns < last.event.at_ns)
        {
            return Err(ChordError::Time);
        }
        if event.event.at_ns > self.end {
            return Err(ChordError::Boundary);
        }
        if self.events.len() >= MAX_CHORD_EVENTS {
            return Err(ChordError::Capacity);
        }
        if event.event.payload.edge == KeyEdge::Down
            && self.events.iter().any(|old| {
                old.event.payload.edge == KeyEdge::Down && identity(&old.event, &event.event)
            })
        {
            return Err(ChordError::Duplicate);
        }
        if let Some(member) = self
            .members
            .iter()
            .find(|m| identity(&m.down.event, &event.event))
        {
            if !provenance(&member.down.event, &event.event) {
                return Err(ChordError::Provenance);
            }
            if event.event.payload.edge == KeyEdge::Up && member.up.is_some() {
                return Err(ChordError::Duplicate);
            }
        }
        Ok(())
    }
    /// `frontier` bounds every still-unobserved input earliest time; None is explicit EOF.
    /// Clock alone cannot exclude queued input inside the inclusive window.
    pub fn advance(
        &mut self,
        now: MonoTime,
        frontier: Option<MonoTime>,
    ) -> Result<ChordDecision, ChordError> {
        if self.finished {
            return Err(ChordError::Finished);
        }
        if now < self.now {
            return Err(ChordError::Time);
        }
        self.now = now;
        Ok(self.decide(now >= self.end && frontier.is_none_or(|at| at > self.end)))
    }
    fn decide(&mut self, mature: bool) -> ChordDecision {
        let complete = self.members.len() == self.spec.keys.len();
        let ready = complete
            && (self.spec.key_up_order == KeyOrder::Insensitive
                || self.up_order.len() >= self.spec.keys.len().saturating_sub(1)
                || mature);
        if self.failed || (mature && !complete) {
            self.finished = true;
            return ChordDecision::Replay(std::mem::take(&mut self.events));
        }
        if ready {
            self.finished = true;
            return ChordDecision::Matched(ChordMatch {
                spec: std::mem::replace(&mut self.spec, Simultaneous::new(vec![])),
                members: std::mem::take(&mut self.members),
                events: std::mem::take(&mut self.events),
                at: self.now,
            });
        }
        ChordDecision::Pending {
            deadline: self.end,
            buffered: self.events.len(),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChordUpFacts {
    pub captured: bool,
    pub release_output: bool,
    pub finished: bool,
    pub after_group_up: bool,
}
pub struct ActiveChord {
    spec: Simultaneous,
    members: Vec<ChordMember>,
    now: MonoTime,
    output_released: bool,
    after_up_reported: bool,
}
impl ActiveChord {
    pub fn is_member(&self, event: &DueEvent<PlannedAction>) -> bool {
        self.members
            .iter()
            .any(|m| provenance(&m.down.event, &event.event))
    }
    pub fn initial_facts(&mut self) -> ChordUpFacts {
        self.facts(true)
    }
    /// Observe without consuming the caller's original receipt, needed for binding Up admission.
    pub fn up(&mut self, event: &DueEvent<PlannedAction>) -> Result<ChordUpFacts, ChordError> {
        if !valid(event) || event.event.payload.edge != KeyEdge::Up {
            return Err(ChordError::Input);
        }
        if event.delivered_at < self.now {
            return Err(ChordError::Time);
        }
        let member = self
            .members
            .iter_mut()
            .find(|m| identity(&m.down.event, &event.event))
            .ok_or(ChordError::Outside)?;
        if event.event.at_ns < member.down.event.at_ns {
            return Err(ChordError::Time);
        }
        if !provenance(&member.down.event, &event.event) {
            return Err(ChordError::Provenance);
        }
        if member.up.is_some() {
            return Err(ChordError::Duplicate);
        }
        self.now = event.delivered_at;
        member.up = Some(ChordObservation::from_receipt(event));
        Ok(self.facts(true))
    }
    fn facts(&mut self, captured: bool) -> ChordUpFacts {
        let finished = self.members.iter().all(|m| m.up.is_some());
        let should_release = match self.spec.key_up_when {
            KeyUpWhen::Any => self.members.iter().any(|m| m.up.is_some()),
            KeyUpWhen::All => finished,
        };
        let release_output = should_release && !self.output_released;
        let after_group_up = finished && !self.after_up_reported;
        self.output_released |= should_release;
        self.after_up_reported |= finished;
        ChordUpFacts {
            captured,
            release_output,
            finished,
            after_group_up,
        }
    }
}
