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
}
impl ModifierScope {
    fn main(selection: &RuleSelection) -> Self {
        Self {
            policy: BranchModifierPolicy::CurrentMinusConsumed,
            snapshot: selection.modifiers_at_down().clone(),
            consumed: selection.consumed_modifiers().clone(),
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
        if self.policy == BranchModifierPolicy::DownSnapshotMinusConsumed {
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
    pub(crate) fn scope(&self) -> &ModifierScope {
        &self.scope
    }
    pub(crate) fn into_parts(self) -> (crate::action_plan::ActionPlan, ModifierScope) {
        (self.plan, self.scope)
    }
}
