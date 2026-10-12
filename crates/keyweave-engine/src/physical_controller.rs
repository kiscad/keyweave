//! Pure normalized-input owner for the timed stage pipeline. No capture or native output APIs.
use crate::{
    action_plan::{ActionPlan, ActionSlot, PlanError, PlanOrigin, plan_actions},
    simple_actor::{SimpleActorError, SimpleStageActor, SimpleStep},
    timeline::{DueEvent, Stage, StageTimeline, TimelineError},
    *,
};
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PhysicalControllerError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Simple(#[from] SimpleActorError),
    #[error(transparent)]
    Timeline(#[from] TimelineError),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error("physical source identity exhausted")]
    SourceExhausted,
    #[error("poll due stage work before ordinary physical input")]
    DeadlinePending,
    #[error("stage controller does not accept external timer/cancellation tokens")]
    UnsupportedControl,
    #[error("queued pointer/wheel ordering with future stage actions is not implemented")]
    UnsupportedPointerScheduling,
    #[error("physical controller failed; stop capture and reset required")]
    Failed,
    #[error("physical root invariant failed")]
    Invariant,
}
#[derive(Debug)]
pub struct PhysicalStep {
    pub capture: CaptureDecision,
    pub outputs: Vec<OutputIntent>,
    /// Query only. Backend must prove desired/submitted ownership and choose one repeat policy.
    pub repeat_target: Option<KeyId>,
    pub next_deadline: Option<MonoTime>,
}
pub struct PhysicalStageController {
    simple: SimpleStageActor,
    active: BTreeMap<(DeviceInstanceId, KeyId), ActionPlan>,
    pending: BTreeSet<ActionSlot>,
    history: StageTimeline<()>,
    context: ContextSnapshot,
    next_source: u64,
    now: MonoTime,
    last_seq: Option<u64>,
    failed: bool,
}
impl PhysicalStageController {
    pub fn new(config: Arc<ConfigSnapshot>) -> Result<Self, PhysicalControllerError> {
        Ok(Self {
            simple: SimpleStageActor::new(config)?,
            active: BTreeMap::new(),
            pending: BTreeSet::new(),
            history: StageTimeline::new(),
            context: ContextSnapshot::default(),
            next_source: 1,
            now: MonoTime(0),
            last_seq: None,
            failed: false,
        })
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        self.simple.snapshot()
    }
    pub fn is_failed(&self) -> bool {
        self.failed
    }
    pub fn active_bindings(&self) -> usize {
        self.active.len()
    }
    pub fn pending_bindings(&self) -> usize {
        self.pending.len()
    }
    pub fn next_deadline(&self) -> Option<MonoTime> {
        self.simple.next_deadline()
    }
    pub fn context_revision(&self) -> u64 {
        self.context.revision
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), PhysicalControllerError> {
        if self.failed {
            return Err(PhysicalControllerError::Failed);
        }
        self.simple.activate(config)?;
        Ok(())
    }
    fn check_time(&self, at: MonoTime) -> Result<(), PhysicalControllerError> {
        if at < self.now {
            return Err(EngineError::TimeBackwards.into());
        }
        Ok(())
    }
    fn step(&self, capture: CaptureDecision) -> PhysicalStep {
        PhysicalStep {
            capture,
            outputs: Vec::new(),
            repeat_target: None,
            next_deadline: self.next_deadline(),
        }
    }
    fn collect(&mut self, actual: SimpleStep, step: &mut PhysicalStep) {
        step.outputs.extend(actual.outputs);
        for slot in actual.released_producers {
            self.pending.remove(&slot);
        }
        if !actual.discarded_input_batches.is_empty() {
            let discarded: BTreeSet<_> = actual.discarded_input_batches.into_iter().collect();
            self.active
                .retain(|_, plan| !discarded.contains(&plan.origin.batch));
            self.pending.retain(|slot| !discarded.contains(&slot.batch));
        }
    }
    fn observe(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
        step: &mut PhysicalStep,
    ) -> Result<(), PhysicalControllerError> {
        self.history.check_device(device, removed)?;
        let actual = self.simple.observe_device(device, removed, at)?;
        self.history.observe_device(device, removed)?;
        self.collect(actual, step);
        Ok(())
    }
    /// Trusted lifecycle observation is still caller supplied, not native device authorization.
    pub fn observe_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
    ) -> Result<PhysicalStep, PhysicalControllerError> {
        self.check_time(at)?;
        self.history.check_device(device, removed)?;
        let mut step = self.step(CaptureDecision::Pass);
        let result = self.observe(device, removed, at, &mut step);
        self.now = at;
        if let Err(error) = result {
            self.failed = true;
            return Err(error);
        }
        step.next_deadline = self.next_deadline();
        Ok(step)
    }
    /// Drain one bounded stage step before retrying an ordinary input with the same sequence.
    pub fn poll(&mut self, at: MonoTime) -> Result<Option<PhysicalStep>, PhysicalControllerError> {
        if self.failed {
            return Err(PhysicalControllerError::Failed);
        }
        self.check_time(at)?;
        let result = self.simple.poll_with_context(at, &self.context);
        self.now = at;
        let actual = match result {
            Ok(actual) => actual,
            Err(error) => {
                self.failed = true;
                return Err(error.into());
            }
        };
        let Some(actual) = actual else {
            return Ok(None);
        };
        let mut step = self.step(CaptureDecision::Pass);
        self.collect(actual, &mut step);
        step.next_deadline = self.next_deadline();
        Ok(Some(step))
    }
    pub fn handle(
        &mut self,
        input: InputEnvelope,
    ) -> Result<PhysicalStep, PhysicalControllerError> {
        self.check_time(input.at_ns)?;
        if self.last_seq.is_some_and(|seq| input.seq <= seq) {
            return Err(EngineError::SequenceBackwards.into());
        }
        if self.failed && !matches!(input.event, InputEvent::Reset) {
            return Err(PhysicalControllerError::Failed);
        }
        if input.provenance == Provenance::Physical {
            match &input.event {
                InputEvent::Key { .. }
                | InputEvent::Wheel { .. }
                | InputEvent::PointerMove { .. } => {
                    self.history.check_device(input.device, false)?;
                    if self.next_deadline().is_some_and(|due| due <= input.at_ns) {
                        return Err(PhysicalControllerError::DeadlinePending);
                    }
                    if matches!(
                        input.event,
                        InputEvent::Wheel { .. } | InputEvent::PointerMove { .. }
                    ) && !self.simple.pointer_ordering_ready()
                    {
                        return Err(PhysicalControllerError::UnsupportedPointerScheduling);
                    }
                }
                InputEvent::ContextChanged { context } => {
                    validate_context_update(
                        context,
                        self.context.revision,
                        input.at_ns,
                        |device| self.history.check_device(device, false).is_err(),
                    )?;
                }
                InputEvent::DeviceRemoved => self.history.check_device(input.device, true)?,
                InputEvent::TimerFired { .. } | InputEvent::CancelDelayed { .. } => {
                    return Err(PhysicalControllerError::UnsupportedControl);
                }
                InputEvent::Reset => {}
            }
            if let InputEvent::Key {
                key,
                edge: KeyEdge::Down,
            } = &input.event
                && !self.active.contains_key(&(input.device, *key))
            {
                if self.active.len().saturating_add(self.pending.len()) >= MAX_BINDINGS {
                    return Err(EngineError::ResourceLimit.into());
                }
                self.next_source
                    .checked_add(1)
                    .ok_or(PhysicalControllerError::SourceExhausted)?;
            }
        }
        self.now = input.at_ns;
        self.last_seq = Some(input.seq);
        let mut step = self.step(CaptureDecision::Capture);
        if input.provenance != Provenance::Physical {
            step.capture = CaptureDecision::Pass;
            return Ok(step);
        }
        let result = self.handle_inner(input, &mut step);
        if let Err(error) = result {
            self.failed = true;
            return Err(error);
        }
        step.next_deadline = self.next_deadline();
        Ok(step)
    }
    fn handle_inner(
        &mut self,
        input: InputEnvelope,
        step: &mut PhysicalStep,
    ) -> Result<(), PhysicalControllerError> {
        match input.event {
            InputEvent::Key { key, edge } => {
                self.observe(input.device, false, input.at_ns, step)?;
                let identity = (input.device, key);
                match edge {
                    KeyEdge::Down => {
                        if self.active.contains_key(&identity) {
                            return Ok(());
                        }
                        let id = self.simple.next_batch_id()?;
                        let following = self
                            .next_source
                            .checked_add(1)
                            .ok_or(PhysicalControllerError::SourceExhausted)?;
                        let plan = plan_actions(
                            &[key.into()],
                            PlanOrigin {
                                batch: id,
                                stage: Stage::Simple,
                                start: input.at_ns,
                                device: input.device,
                                source: SourceToken(self.next_source),
                                config: self.snapshot().clone(),
                            },
                            false,
                        )?;
                        let down = plan
                            .events()
                            .first()
                            .ok_or(PhysicalControllerError::Invariant)?
                            .clone();
                        self.simple.register_input(plan.clone())?;
                        self.next_source = following;
                        let actual = self.simple.admit(
                            &DueEvent {
                                event: down,
                                delivered_at: input.at_ns,
                            },
                            &self.context,
                        )?;
                        self.active.insert(identity, plan);
                        self.collect(actual, step);
                    }
                    KeyEdge::Up => {
                        let Some(plan) = self.active.get(&identity) else {
                            return Ok(());
                        };
                        let slot = plan
                            .events()
                            .first()
                            .ok_or(PhysicalControllerError::Invariant)?
                            .payload
                            .slot;
                        let up = plan
                            .input_up_events(input.at_ns)
                            .into_iter()
                            .next()
                            .ok_or(PhysicalControllerError::Invariant)?;
                        let actual = self.simple.admit(
                            &DueEvent {
                                event: up,
                                delivered_at: input.at_ns,
                            },
                            &self.context,
                        )?;
                        self.active.remove(&identity);
                        self.pending.insert(slot);
                        self.collect(actual, step);
                    }
                    KeyEdge::Repeat => {
                        step.repeat_target = self
                            .active
                            .get(&identity)
                            .and_then(|plan| plan.events().first())
                            .and_then(|event| {
                                self.simple.physical_repeat_target(event.payload.slot)
                            });
                    }
                }
            }
            InputEvent::Wheel {
                horizontal,
                vertical,
            } => {
                self.observe(input.device, false, input.at_ns, step)?;
                self.simple
                    .observe_wheel(horizontal, vertical, input.at_ns)?;
                step.outputs
                    .extend(self.simple.synchronize_modifiers(input.at_ns)?);
                step.capture = CaptureDecision::Pass;
            }
            InputEvent::PointerMove { .. } => {
                self.observe(input.device, false, input.at_ns, step)?;
                step.outputs
                    .extend(self.simple.synchronize_modifiers(input.at_ns)?);
                step.capture = CaptureDecision::Pass;
            }
            InputEvent::ContextChanged { context } => {
                self.context = context;
                step.capture = CaptureDecision::Pass;
            }
            InputEvent::DeviceRemoved => self.observe(input.device, true, input.at_ns, step)?,
            InputEvent::Reset => {
                let actual = self.simple.reset(input.at_ns)?;
                self.history.reset(input.at_ns)?;
                self.collect(actual, step);
                self.active.clear();
                self.pending.clear();
                self.failed = false;
            }
            InputEvent::TimerFired { .. } | InputEvent::CancelDelayed { .. } => {
                return Err(PhysicalControllerError::UnsupportedControl);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn exhausted_source_cursor_rejects_before_sequence_device_or_root_commit_and_reset_does_not_reuse()
     {
        let mut actor =
            PhysicalStageController::new(Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))))
                .unwrap();
        actor.next_source = u64::MAX;
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        let event = InputEnvelope {
            seq: 1,
            at_ns: MonoTime(100),
            device,
            provenance: Provenance::Physical,
            event: InputEvent::Key {
                key: "a".parse().unwrap(),
                edge: KeyEdge::Down,
            },
        };
        assert!(matches!(
            actor.handle(event),
            Err(PhysicalControllerError::SourceExhausted)
        ));
        assert_eq!(actor.last_seq, None);
        assert_eq!(actor.simple.pending_inputs(), 0);
        assert_eq!(actor.next_source, u64::MAX);
        assert_eq!(
            actor.history.check_live_device(device),
            Err(TimelineError::RetiredDevice)
        );
        assert!(!actor.failed);
        actor
            .handle(InputEnvelope {
                seq: 1,
                at_ns: MonoTime(100),
                device,
                provenance: Provenance::Physical,
                event: InputEvent::Reset,
            })
            .unwrap();
        assert_eq!(actor.next_source, u64::MAX);
    }
}
