//! Frozen producer Down decision, before actual binding/source/child admission.
use crate::{
    action_plan::{BatchId, PlanError, PlanOrigin, PlannedAction, plan_actions},
    modifier_scope::ScopedActionPlan,
    rule_selection::RuleSelection,
    timeline::{Stage, StageEvent},
    *,
};

/// Once-selected main work. No producer activation or native authority is implied.
/// Retain this through downstream admission/retry and the original producer Up.
pub struct ProducerDecision {
    down: StageEvent<PlannedAction>,
    delivered_at: MonoTime,
    destination: Stage,
    selection: Option<RuleSelection>,
}
impl ProducerDecision {
    pub(crate) fn new(
        down: StageEvent<PlannedAction>,
        delivered_at: MonoTime,
        destination: Stage,
        selection: Option<RuleSelection>,
    ) -> Self {
        Self {
            down,
            delivered_at,
            destination,
            selection,
        }
    }
    pub fn down(&self) -> &StageEvent<PlannedAction> {
        &self.down
    }
    pub fn delivered_at(&self) -> MonoTime {
        self.delivered_at
    }
    pub fn destination(&self) -> Stage {
        self.destination
    }
    /// None is pass-through. Some with zero eligible actions still consumes the match.
    pub fn selection(&self) -> Option<&RuleSelection> {
        self.selection.as_ref()
    }
    /// Belongs to the incoming action's original rule, not the newly selected mapping.
    pub fn originating_halt(&self) -> bool {
        self.down.payload.halt
    }
    /// Pure re-planning preserves the frozen main indices and original input metadata.
    pub fn plan_main(&self, batch: BatchId) -> Result<ScopedActionPlan, PlanError> {
        if let Some(selection) = &self.selection {
            ScopedActionPlan::main(
                selection,
                batch,
                self.delivered_at,
                self.down.device,
                self.down.source,
            )
        } else {
            let action = Action {
                target: self.down.payload.target.clone(),
                repeat: true,
                lazy: self.down.payload.lazy,
                halt: false,
                hold_down_ns: 0,
                modifiers: Vec::new(),
                conditions: Vec::new(),
            };
            let plan = plan_actions(
                &[action],
                PlanOrigin {
                    batch,
                    stage: self.destination,
                    start: self.delivered_at,
                    device: self.down.device,
                    source: self.down.source,
                    config: Arc::clone(&self.down.config),
                },
                false,
            )?;
            Ok(ScopedActionPlan::plain(plan))
        }
    }
}
