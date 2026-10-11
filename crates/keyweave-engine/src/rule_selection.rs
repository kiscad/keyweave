//! Immutable rule decision for a generating stage; no ownership or IO work.
use crate::{Action, Condition, ConfigSnapshot, Mapping, Truth};
use keyweave_types::{DeviceInstanceId, KeyId, MonoTime, SourceToken};
use std::{collections::BTreeSet, sync::Arc};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingStage {
    Simple,
    Complex,
}
/// Retains the original snapshot, selected rule and once-filtered main actions.
/// Delayed/held/alone branches are filtered at their own generating event.
/// Selection is not activation, source reservation, a binding or a native lease.
pub struct RuleSelection {
    config: Arc<ConfigSnapshot>,
    stage: MappingStage,
    index: usize,
    modifiers: BTreeSet<KeyId>,
    consumed: BTreeSet<KeyId>,
    actions: Vec<usize>,
}
impl RuleSelection {
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        &self.config
    }
    pub fn stage(&self) -> MappingStage {
        self.stage
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn mapping(&self) -> &Mapping {
        match self.stage {
            MappingStage::Simple => &self.config.simple[self.index],
            MappingStage::Complex => &self.config.complex[self.index],
        }
    }
    pub fn modifiers_at_down(&self) -> &BTreeSet<KeyId> {
        &self.modifiers
    }
    pub fn consumed_modifiers(&self) -> &BTreeSet<KeyId> {
        &self.consumed
    }
    /// Generate only main-to work for the next stage, using the saved eligibility
    /// and old snapshot. Caller still must reserve owners/slots, enqueue atomically,
    /// retain input-up bindings and implement the other rule branches.
    pub fn plan_main(
        &self,
        batch: crate::action_plan::BatchId,
        start: MonoTime,
        device: DeviceInstanceId,
        source: SourceToken,
    ) -> Result<crate::action_plan::ActionPlan, crate::action_plan::PlanError> {
        crate::action_plan::plan_action_refs(
            self.actions(),
            crate::action_plan::PlanOrigin {
                batch,
                start,
                device,
                source,
                config: Arc::clone(&self.config),
                stage: match self.stage {
                    MappingStage::Simple => crate::timeline::Stage::Complex,
                    MappingStage::Complex => crate::timeline::Stage::Output,
                },
            },
            false,
        )
    }
    /// Call at the branch's generating event, after any required child cleanup.
    /// The caller decides timer/alone/cancellation eligibility and supplies halt.
    pub fn select_branch(
        &self,
        branch: RuleBranch,
        halted: bool,
        mut evaluate: impl FnMut(&Condition) -> Truth,
    ) -> Option<BranchSelection> {
        let actions = branch_actions(self.mapping(), branch)?;
        let indices = if halted {
            Vec::new()
        } else {
            actions
                .iter()
                .enumerate()
                .filter(|(_, action)| action_matches(action, &mut evaluate))
                .map(|(index, _)| index)
                .collect()
        };
        Some(BranchSelection {
            config: Arc::clone(&self.config),
            stage: self.stage,
            index: self.index,
            branch,
            actions: indices,
            modifiers: self.modifiers.clone(),
            consumed: self.consumed.clone(),
        })
    }
    pub fn actions(&self) -> impl DoubleEndedIterator<Item = &Action> + ExactSizeIterator + Clone {
        self.actions.iter().map(|index| &self.mapping().to[*index])
    }
}
/// Trusted caller supplies its current stage state; evaluate must observe the
/// same condition state for this whole call. Only Truth::True matches, including
/// already-negated unless predicates. No re-evaluation on queued delivery.
pub fn select_rule(
    config: Arc<ConfigSnapshot>,
    stage: MappingStage,
    key: KeyId,
    modifiers: BTreeSet<KeyId>,
    mut evaluate: impl FnMut(&Condition) -> Truth,
) -> Option<RuleSelection> {
    let mappings = match stage {
        MappingStage::Simple => &config.simple,
        MappingStage::Complex => &config.complex,
    };
    for (index, mapping) in mappings.iter().enumerate() {
        if mapping.from != key
            || !mapping
                .extras
                .conditions
                .iter()
                .all(|condition| evaluate(condition) == Truth::True)
        {
            continue;
        }
        let Some(consumed) = mapping
            .extras
            .from_modifiers
            .consumed(&modifiers, mapping.allow_extra_modifiers)
        else {
            continue;
        };
        let actions = mapping
            .to
            .iter()
            .enumerate()
            .filter(|(_, action)| action_matches(action, &mut evaluate))
            .map(|(index, _)| index)
            .collect();
        return Some(RuleSelection {
            config,
            stage,
            index,
            modifiers,
            consumed,
            actions,
        });
    }
    None
}

/// Branch admission is separate from action eligibility: this enum does not fire a timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleBranch {
    Held,
    Alone,
    AfterKeyUp,
    DelayedInvoked,
    DelayedCanceled,
}
/// Apply this scope at execution; it does not mint owners or physically press keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchModifierPolicy {
    CurrentMinusConsumed,
    DownSnapshotMinusConsumed,
}
/// Frozen branch actions retain the original rule even after RuleSelection is dropped.
pub struct BranchSelection {
    config: Arc<ConfigSnapshot>,
    stage: MappingStage,
    index: usize,
    branch: RuleBranch,
    actions: Vec<usize>,
    modifiers: BTreeSet<KeyId>,
    consumed: BTreeSet<KeyId>,
}
impl BranchSelection {
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        &self.config
    }
    pub fn branch(&self) -> RuleBranch {
        self.branch
    }
    pub fn modifiers_at_down(&self) -> &BTreeSet<KeyId> {
        &self.modifiers
    }
    pub fn consumed_modifiers(&self) -> &BTreeSet<KeyId> {
        &self.consumed
    }
    pub fn modifier_policy(&self) -> BranchModifierPolicy {
        match self.branch {
            RuleBranch::Held | RuleBranch::AfterKeyUp => BranchModifierPolicy::CurrentMinusConsumed,
            _ => BranchModifierPolicy::DownSnapshotMinusConsumed,
        }
    }
    pub fn actions(&self) -> impl DoubleEndedIterator<Item = &Action> + ExactSizeIterator + Clone {
        let mapping = match self.stage {
            MappingStage::Simple => &self.config.simple[self.index],
            MappingStage::Complex => &self.config.complex[self.index],
        };
        // Construction accepts delayed branches only when their original array exists.
        let actions = branch_actions(mapping, self.branch).unwrap_or(&[]);
        self.actions.iter().map(move |index| &actions[*index])
    }
    /// Plans action work only; the caller must retain and apply modifier_policy.
    pub fn plan(
        &self,
        batch: crate::action_plan::BatchId,
        start: MonoTime,
        device: DeviceInstanceId,
        source: SourceToken,
    ) -> Result<crate::action_plan::ActionPlan, crate::action_plan::PlanError> {
        crate::action_plan::plan_action_refs(
            self.actions(),
            crate::action_plan::PlanOrigin {
                batch,
                start,
                device,
                source,
                config: Arc::clone(&self.config),
                stage: match self.stage {
                    MappingStage::Simple => crate::timeline::Stage::Complex,
                    MappingStage::Complex => crate::timeline::Stage::Output,
                },
            },
            self.branch != RuleBranch::Held,
        )
    }
}
fn branch_actions(mapping: &Mapping, branch: RuleBranch) -> Option<&[Action]> {
    Some(match branch {
        RuleBranch::Held => &mapping.extras.if_held_down,
        RuleBranch::Alone => &mapping.extras.if_alone,
        RuleBranch::AfterKeyUp => &mapping.extras.after_key_up,
        RuleBranch::DelayedInvoked => &mapping.extras.delayed.as_ref()?.if_invoked,
        RuleBranch::DelayedCanceled => &mapping.extras.delayed.as_ref()?.if_canceled,
    })
}
fn action_matches(action: &Action, evaluate: &mut impl FnMut(&Condition) -> Truth) -> bool {
    action
        .conditions
        .iter()
        .all(|condition| evaluate(condition) == Truth::True)
}
/// A generating event filters the whole array before the first action executes.
pub(crate) fn filter_actions(
    actions: &[Action],
    mut evaluate: impl FnMut(&Condition) -> Truth,
) -> Vec<&Action> {
    actions
        .iter()
        .filter(|action| action_matches(action, &mut evaluate))
        .collect()
}
