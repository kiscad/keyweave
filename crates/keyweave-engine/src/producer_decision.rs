//! Frozen producer Down decision, before actual binding/source/child admission.
use crate::{
    action_plan::{BatchId, PlanError, PlanOrigin, PlannedAction, plan_actions},
    modifier_scope::ScopedActionPlan,
    rule_selection::RuleSelection,
    timeline::{DueEvent, Stage, StageEvent},
    *,
};

/// Once-selected main work. No producer activation or native authority is implied.
/// Retain this through downstream admission/retry and the original producer Up.
pub struct ProducerDecision {
    down: StageEvent<PlannedAction>,
    delivered_at: MonoTime,
    destination: Stage,
    selection: Option<RuleSelection>,
    up_delivered_at: Option<MonoTime>,
    up_lazy: bool,
    device_identity: bool,
    original_event_timing: bool,
}
#[derive(Debug, PartialEq, Eq, Error)]
pub enum ProducerUpError {
    #[error("invalid producer Up lifecycle")]
    Lifecycle,
    #[error("producer Up does not match saved Down provenance")]
    Provenance,
    #[error("producer Up causal time moved backwards")]
    TimeReversed,
    #[error(transparent)]
    Binding(#[from] stage_bindings::BindingError),
}
/// Commit only after saved downstream Up admission succeeds. Abort permits original receipt retry.
#[must_use = "commit or discard the prepared producer Up"]
pub struct PreparedProducerUp<'a> {
    decision: &'a mut ProducerDecision,
    delivered_at: MonoTime,
}
impl PreparedProducerUp<'_> {
    pub fn abort(self) {
        drop(self);
    }
    pub fn commit(self) {
        self.decision.up_delivered_at = Some(self.delivered_at);
    }
}
impl ProducerDecision {
    pub(crate) fn new(
        down: StageEvent<PlannedAction>,
        delivered_at: MonoTime,
        destination: Stage,
        selection: Option<RuleSelection>,
    ) -> Self {
        let up_lazy = down.payload.lazy;
        Self {
            down,
            delivered_at,
            destination,
            selection,
            up_delivered_at: None,
            up_lazy,
            device_identity: true,
            original_event_timing: false,
        }
    }
    /// Preserve a registered input plan's original Up flags, including asymmetric modifier lazy Up.
    /// This is supplied before activation; it does not select conditions again.
    pub fn with_original_up(mut self, up: &PlannedAction) -> Result<Self, ProducerUpError> {
        if self.up_delivered_at.is_some() || up.edge != KeyEdge::Up {
            return Err(ProducerUpError::Lifecycle);
        }
        if up.slot != self.down.payload.slot || up.target != self.down.payload.target || up.halt {
            return Err(ProducerUpError::Provenance);
        }
        self.up_lazy = up.lazy;
        Ok(self)
    }
    pub fn up_delivered_at(&self) -> Option<MonoTime> {
        self.up_delivered_at
    }
    pub(crate) fn with_original_event_timing(mut self, enabled: bool) -> Self {
        self.original_event_timing = enabled;
        self
    }
    pub(crate) fn uses_original_event_timing(&self) -> bool {
        self.original_event_timing
    }
    pub(crate) fn with_device_identity(mut self, known: bool) -> Self {
        self.device_identity = known;
        self
    }
    pub(crate) fn has_device_identity(&self) -> bool {
        self.device_identity
    }
    pub fn prepare_up(
        &mut self,
        up: &DueEvent<PlannedAction>,
    ) -> Result<PreparedProducerUp<'_>, ProducerUpError> {
        if self.up_delivered_at.is_some() || up.event.payload.edge != KeyEdge::Up {
            return Err(ProducerUpError::Lifecycle);
        }
        let event = &up.event;
        if event.payload.slot != self.down.payload.slot
            || event.stage != self.down.stage
            || event.device != self.down.device
            || event.source != self.down.source
            || !Arc::ptr_eq(&event.config, &self.down.config)
            || event.payload.target != self.down.payload.target
            || event.payload.lazy != self.up_lazy
            || event.payload.halt
        {
            return Err(ProducerUpError::Provenance);
        }
        if event.at_ns < self.down.at_ns
            || up.delivered_at < event.at_ns
            || up.delivered_at < self.delivered_at
        {
            return Err(ProducerUpError::TimeReversed);
        }
        Ok(PreparedProducerUp {
            decision: self,
            delivered_at: up.delivered_at,
        })
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
            let mut plan = plan_actions(
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
            for up in &mut plan.release_on_input_up {
                up.lazy = self.up_lazy;
            }
            Ok(ScopedActionPlan::plain(plan))
        }
    }
}
