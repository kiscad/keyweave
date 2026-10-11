//! Plan an already-filtered action batch before executing any of its work.
//! Actor execution, conditions, owner reservations and halt state are separate.
use crate::{
    Action, ActionTarget, ConfigSnapshot, MAX_OUTPUTS_PER_EVENT,
    timeline::{Stage, StageEvent},
};
use keyweave_types::{DeviceInstanceId, KeyEdge, MonoTime, SourceToken};
use std::sync::Arc;
use thiserror::Error;

pub const MAX_ACTION_DURATION_NS: u64 = 86_400_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BatchId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ActionSlot {
    pub batch: BatchId,
    pub index: u16,
}

/// Batch identities come from the actor; they must never alias live batches.
/// The same slot on down/up names the same saved next-stage binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedAction {
    pub slot: ActionSlot,
    pub target: ActionTarget,
    pub edge: KeyEdge,
    pub lazy: bool,
    pub halt: bool,
}

#[derive(Clone, Debug)]
pub struct PlanOrigin {
    pub batch: BatchId,
    pub stage: Stage,
    pub start: MonoTime,
    pub device: DeviceInstanceId,
    pub source: SourceToken,
    pub config: Arc<ConfigSnapshot>,
}

#[derive(Clone, Debug)]
pub struct ActionPlan {
    pub(crate) events: Vec<StageEvent<PlannedAction>>,
    /// Up commands that must wait for original input up; duration never replaces it.
    pub(crate) release_on_input_up: Vec<PlannedAction>,
    pub(crate) ends_at: MonoTime,
    pub(crate) origin: PlanOrigin,
}
impl ActionPlan {
    pub fn events(&self) -> &[StageEvent<PlannedAction>] {
        &self.events
    }
    pub fn releases(&self) -> &[PlannedAction] {
        &self.release_on_input_up
    }
    pub fn ends_at(&self) -> MonoTime {
        self.ends_at
    }
    pub fn input_up_events(&self, at: MonoTime) -> Vec<StageEvent<PlannedAction>> {
        self.release_on_input_up
            .iter()
            .cloned()
            .map(|payload| self.origin.event(at.max(self.ends_at), payload))
            .collect()
    }
}
impl PlanOrigin {
    fn event(&self, at: MonoTime, payload: PlannedAction) -> StageEvent<PlannedAction> {
        StageEvent {
            stage: self.stage,
            at_ns: at,
            device: self.device,
            source: self.source,
            config: self.config.clone(),
            payload,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum PlanError {
    #[error("action batch expansion exceeds bounded planning capacity")]
    Capacity,
    #[error("action duration exceeds one day")]
    InvalidDuration,
    #[error("action time would overflow monotonic time")]
    TimeOverflow,
    #[error("modifier scope must contain only standard modifiers on a key action")]
    InvalidScope,
}

/// `actions` must already be filtered once against the generating stage's state.
/// Main targets keep their options; modifier scopes enter the next stage too.
/// `tap_all` is true for alone, after-up and either delayed-action branch.
pub fn plan_actions(
    actions: &[Action],
    origin: PlanOrigin,
    tap_all: bool,
) -> Result<ActionPlan, PlanError> {
    plan_action_refs(actions.iter(), origin, tap_all)
}

// Internal stable iterators only: caller is the slice wrapper or immutable
// RuleSelection's private index list. Preflight runs before any expansion.
pub(crate) fn plan_action_refs<'a>(
    actions: impl Clone + ExactSizeIterator<Item = &'a Action>,
    origin: PlanOrigin,
    tap_all: bool,
) -> Result<ActionPlan, PlanError> {
    let action_count = actions.len();
    let mut slots = 0usize;
    let mut end = origin.start.0;
    for (index, action) in actions.clone().enumerate() {
        slots = slots
            .saturating_add(1)
            .saturating_add(action.modifiers.len());
        if slots.saturating_mul(2) > MAX_OUTPUTS_PER_EVENT {
            return Err(PlanError::Capacity);
        }
        if action.hold_down_ns > MAX_ACTION_DURATION_NS {
            return Err(PlanError::InvalidDuration);
        }
        if (!action.modifiers.is_empty() && action.key().is_none())
            || action.modifiers.iter().any(|key| !key.is_modifier())
        {
            return Err(PlanError::InvalidScope);
        }
        if tap_all || !action.repeat || index + 1 < action_count {
            end = end
                .checked_add(action.hold_down_ns)
                .ok_or(PlanError::TimeOverflow)?;
        }
    }
    let mut events = Vec::with_capacity(slots * 2);
    let mut release_on_input_up = Vec::new();
    let mut at = origin.start;
    let mut next_slot = 0u16; // Preflight bounds slots <= 512, including all scopes.
    for (index, action) in actions.enumerate() {
        let last = index + 1 == action_count;
        let main_modifier = action.key().is_some_and(|key| key.is_modifier());
        let mut scopes = Vec::with_capacity(action.modifiers.len());
        for key in &action.modifiers {
            let slot = ActionSlot {
                batch: origin.batch,
                index: next_slot,
            };
            next_slot += 1;
            let target = ActionTarget::Key(*key);
            events.push(origin.event(
                at,
                PlannedAction {
                    slot,
                    target: target.clone(),
                    edge: KeyEdge::Down,
                    lazy: tap_all || action.lazy || !main_modifier,
                    halt: false,
                },
            ));
            scopes.push(PlannedAction {
                slot,
                target,
                edge: KeyEdge::Up,
                lazy: true,
                halt: false,
            });
        }
        let slot = ActionSlot {
            batch: origin.batch,
            index: next_slot,
        };
        next_slot += 1;
        events.push(origin.event(
            at,
            PlannedAction {
                slot,
                target: action.target.clone(),
                edge: KeyEdge::Down,
                lazy: action.lazy,
                halt: action.halt,
            },
        ));
        let up = PlannedAction {
            slot,
            target: action.target.clone(),
            edge: KeyEdge::Up,
            lazy: action.lazy,
            halt: false,
        };
        if !tap_all && last && action.repeat {
            release_on_input_up.push(up);
        } else {
            // Checked in preflight; no visible owner or queue mutation occurred.
            at = MonoTime(at.0 + action.hold_down_ns);
            events.push(origin.event(at, up));
        }
        if !tap_all && last && main_modifier {
            release_on_input_up.extend(scopes.into_iter().rev());
        } else {
            events.extend(scopes.into_iter().map(|payload| origin.event(at, payload)));
        }
    }
    Ok(ActionPlan {
        events,
        release_on_input_up,
        ends_at: MonoTime(end),
        origin,
    })
}
