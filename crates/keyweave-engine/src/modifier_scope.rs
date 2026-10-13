//! Saved modifier scope for deferred work. No platform IO or native authority.
use crate::{
    rule_selection::{BranchModifierPolicy, BranchSelection, RuleBranch, RuleSelection},
    *,
};

/// Constructed from a frozen rule decision; actual owners are captured at execution.
pub(crate) struct ModifierScope {
    policy: BranchModifierPolicy,
    snapshot: BTreeSet<KeyId>,
    consumed: BTreeSet<KeyId>,
    until_input_up: bool,
    snapshot_events: bool,
}
impl ModifierScope {
    fn main(selection: &RuleSelection) -> Self {
        Self {
            policy: BranchModifierPolicy::CurrentMinusConsumed,
            snapshot: selection.modifiers_at_down().clone(),
            consumed: selection.consumed_modifiers().clone(),
            snapshot_events: false,
            until_input_up: selection
                .actions()
                .next_back()
                .is_some_and(|action| action.key().is_some_and(KeyId::is_modifier)),
        }
    }
    fn branch(selection: &BranchSelection) -> Self {
        Self {
            policy: selection.modifier_policy(),
            snapshot: selection.modifiers_at_down().clone(),
            consumed: selection.consumed_modifiers().clone(),
            snapshot_events: false,
            until_input_up: selection.branch() == RuleBranch::Held
                && selection
                    .actions()
                    .next_back()
                    .is_some_and(|action| action.key().is_some_and(KeyId::is_modifier)),
        }
    }
    pub(crate) fn consumed(&self) -> &BTreeSet<KeyId> {
        &self.consumed
    }
    pub(crate) fn snapshot(&self) -> &BTreeSet<KeyId> {
        &self.snapshot
    }
    pub(crate) fn policy(&self) -> BranchModifierPolicy {
        self.policy
    }
    pub(crate) fn until_input_up(&self) -> bool {
        self.until_input_up
    }
    pub(crate) fn reservation(&self) -> usize {
        if self.policy == BranchModifierPolicy::DownSnapshotMinusConsumed && !self.snapshot_events {
            self.snapshot.len()
        } else {
            0
        }
    }
    pub(crate) fn keys(&self) -> impl Iterator<Item = KeyId> + '_ {
        self.snapshot.iter().chain(&self.consumed).copied()
    }
    /// Exact execution-time checks precede all owner/hidden mutation.
    pub(crate) fn open(self, core: &mut Engine, base: u64) -> Result<ScopeBinding, EngineError> {
        let mut temporary = Vec::new();
        let mut hidden = Vec::new();
        if self.policy == BranchModifierPolicy::DownSnapshotMinusConsumed {
            // Include already-hidden owners: scopes may finish in either order.
            for (key, owners) in &core.owners {
                if key.is_modifier() && !self.snapshot.contains(key) {
                    hidden.extend(owners.iter().map(|source| (*key, *source)));
                }
            }
            // Own the whole snapshot for the asynchronous lifetime: an original
            // or another scope's owner may disappear before this branch ends.
            for (index, key) in self.snapshot.iter().enumerate() {
                let source = SourceToken(
                    base.checked_add(index as u64)
                        .ok_or(EngineError::ResourceLimit)?,
                );
                temporary.push((*key, source));
            }
        }
        for key in &self.consumed {
            hidden.extend(
                core.owners
                    .get(key)
                    .into_iter()
                    .flatten()
                    .map(|source| (*key, *source)),
            );
        }
        hidden.extend(
            temporary
                .iter()
                .filter(|(key, _)| self.consumed.contains(key))
                .copied(),
        );
        if hidden.len()
            > MAX_PERSISTENT_SUPPRESSION_REFERENCES.saturating_sub(core.suppression_references)
        {
            return Err(EngineError::ResourceLimit);
        }
        for (key, source) in &temporary {
            core.owners.entry(*key).or_default().insert(*source);
        }
        for owner in &hidden {
            *core.hidden.entry(*owner).or_default() += 1;
        }
        core.suppression_references += hidden.len();
        Ok(ScopeBinding { temporary, hidden })
    }
}
/// Only actual execution creates this payload. Drop alone does not release owners.
pub(crate) struct ScopeBinding {
    temporary: Vec<(KeyId, SourceToken)>,
    hidden: Vec<(KeyId, SourceToken)>,
}
impl ScopeBinding {
    pub(crate) fn temporary_owners(&self) -> impl Iterator<Item = (KeyId, SourceToken)> + '_ {
        self.temporary.iter().copied()
    }
    pub(crate) fn close(self, core: &mut Engine) {
        core.suppression_references -= self.hidden.len();
        for owner in self.hidden {
            if let Some(count) = core.hidden.get_mut(&owner) {
                *count -= 1;
                if *count == 0 {
                    core.hidden.remove(&owner);
                }
            }
        }
        for (key, source) in self.temporary.into_iter().rev() {
            core.remove_owner(key, source);
        }
    }
}

/// Action work and its frozen scope are constructed together and cannot be swapped.
pub struct ScopedActionPlan {
    plan: crate::action_plan::ActionPlan,
    scope: ModifierScope,
}
impl ScopedActionPlan {
    /// Internal pass-through batches have no consumed or snapshot modifiers.
    pub(crate) fn plain(plan: crate::action_plan::ActionPlan) -> Self {
        Self {
            plan,
            scope: ModifierScope {
                policy: BranchModifierPolicy::CurrentMinusConsumed,
                snapshot: BTreeSet::new(),
                consumed: BTreeSet::new(),
                until_input_up: false,
                snapshot_events: false,
            },
        }
    }
    pub fn main(
        selection: &RuleSelection,
        batch: crate::action_plan::BatchId,
        start: MonoTime,
        device: DeviceInstanceId,
        source: SourceToken,
    ) -> Result<Self, crate::action_plan::PlanError> {
        let scope = ModifierScope::main(selection);
        Self::new(selection.plan_main(batch, start, device, source)?, scope)
    }
    pub fn branch(
        selection: &BranchSelection,
        batch: crate::action_plan::BatchId,
        start: MonoTime,
        device: DeviceInstanceId,
        source: SourceToken,
    ) -> Result<Self, crate::action_plan::PlanError> {
        let scope = ModifierScope::branch(selection);
        Self::new(selection.plan(batch, start, device, source)?, scope)
    }
    fn new(
        plan: crate::action_plan::ActionPlan,
        scope: ModifierScope,
    ) -> Result<Self, crate::action_plan::PlanError> {
        if scope.keys().any(|key| !key.is_modifier()) || !scope.consumed.is_subset(&scope.snapshot)
        {
            return Err(crate::action_plan::PlanError::InvalidScope);
        }
        Ok(Self { plan, scope })
    }
    pub fn actions(&self) -> &crate::action_plan::ActionPlan {
        &self.plan
    }
    /// Simple snapshot differences must pass through Complex rather than bypass it.
    /// Existing current modifiers retain their saved bindings; only missing keys are added.
    pub(crate) fn materialize_complex_snapshot(
        mut self,
        visible: &BTreeSet<KeyId>,
    ) -> Result<Self, crate::action_plan::PlanError> {
        use crate::action_plan::{ActionSlot, PlanError, PlannedAction};
        use crate::timeline::{Stage, StageEvent};
        if self.scope.policy != BranchModifierPolicy::DownSnapshotMinusConsumed {
            return Ok(self);
        }
        if self.plan.origin.stage != Stage::Complex || !self.plan.releases().is_empty() {
            return Err(PlanError::InvalidScope);
        }
        if self.plan.events.is_empty() {
            return Ok(self);
        }
        let missing: Vec<_> = self
            .scope
            .snapshot
            .difference(visible)
            .filter(|key| !self.scope.consumed.contains(key))
            .copied()
            .collect();
        let slots = self
            .plan
            .events
            .iter()
            .filter(|event| event.payload.edge == KeyEdge::Down)
            .count();
        if slots.saturating_add(missing.len()).saturating_mul(2) > MAX_OUTPUTS_PER_EVENT {
            return Err(PlanError::Capacity);
        }
        let mut events = Vec::with_capacity(self.plan.events.len() + missing.len() * 2);
        let mut ups = Vec::with_capacity(missing.len());
        for (offset, key) in missing.into_iter().enumerate() {
            let payload = PlannedAction {
                slot: ActionSlot {
                    batch: self.plan.origin.batch,
                    index: (slots + offset) as u16,
                },
                target: ActionTarget::Key(key),
                edge: KeyEdge::Down,
                lazy: true,
                halt: false,
            };
            self.plan.snapshot_modifier_slots.insert(payload.slot);
            let event = StageEvent {
                stage: Stage::Complex,
                at_ns: self.plan.origin.start,
                device: self.plan.origin.device,
                source: self.plan.origin.source,
                config: self.plan.origin.config.clone(),
                payload,
            };
            let mut up = event.clone();
            up.at_ns = self.plan.ends_at;
            up.payload.edge = KeyEdge::Up;
            events.push(event);
            ups.push(up);
        }
        events.append(&mut self.plan.events);
        events.extend(ups.into_iter().rev());
        self.plan.events = events;
        self.scope.snapshot_events = true;
        Ok(self)
    }
    pub(crate) fn scope(&self) -> &ModifierScope {
        &self.scope
    }
    pub(crate) fn into_parts(self) -> (crate::action_plan::ActionPlan, ModifierScope) {
        (self.plan, self.scope)
    }
}
