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
    #[error("physical controller failed; stop capture and reset required")]
    Failed,
    #[error("physical root invariant failed")]
    Invariant,
}
#[derive(Debug)]
pub struct PhysicalStep {
    pub capture: CaptureDecision,
    pub outputs: Vec<OutputIntent>,
    /// Submit keyboard outputs first, then this step's captured pointer receipt.
    pub forwarded_pointers: Vec<DueEvent<crate::pointer_work::PointerMotion>>,
    /// Query only. Backend must prove desired/submitted ownership and choose one repeat policy.
    pub repeat_target: Option<KeyId>,
    pub next_deadline: Option<MonoTime>,
}
struct SourceLease {
    device: DeviceInstanceId,
    units: usize,
}
pub struct PhysicalStageController {
    simple: SimpleStageActor,
    active: BTreeMap<(DeviceInstanceId, KeyId), ActionPlan>,
    source_bounds: BTreeMap<KeyId, usize>,
    source_leases: BTreeMap<ActionSlot, SourceLease>,
    reserved_sources: usize,
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
            source_bounds: config.timed_stage_source_bounds(),
            source_leases: BTreeMap::new(),
            reserved_sources: 0,
            simple: SimpleStageActor::new_with_source_limit(
                config,
                crate::source_identity::ORIGIN_DOMAIN_START,
            )?,
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
    /// Model diagnostics, not native API acceptance. Queued stage work is counted
    /// once; the Output generation mirror is not another deadline task.
    pub fn state(&self) -> EngineState {
        let (output_owners, pressed_outputs, pending_deadlines) = self.simple.output_model();
        EngineState {
            bindings: self.active.len(),
            output_owners,
            pressed_outputs,
            binding_generations: self
                .active
                .values()
                .map(|plan| plan.origin.config.generation)
                .collect(),
            pending_deadlines,
            pending_bindings: self.pending.len(),
        }
    }
    pub fn snapshot(&self) -> &Arc<ConfigSnapshot> {
        self.simple.snapshot()
    }
    pub fn is_failed(&self) -> bool {
        self.failed
    }
    /// Conservative whole-root leases, including possible future branches; not actual emitted owners.
    pub fn reserved_sources(&self) -> usize {
        self.reserved_sources
    }
    fn prepare_origin(&self) -> Result<(SourceToken, u64), PhysicalControllerError> {
        crate::source_identity::prepare_origin(self.next_source)
            .ok_or(PhysicalControllerError::SourceExhausted)
    }
    fn source_cost(&self, key: KeyId) -> usize {
        self.source_bounds.get(&key).copied().unwrap_or(3)
    }
    fn release_source_lease(&mut self, slot: ActionSlot) -> Result<(), PhysicalControllerError> {
        if let Some(lease) = self.source_leases.remove(&slot) {
            self.reserved_sources = self
                .reserved_sources
                .checked_sub(lease.units)
                .ok_or(PhysicalControllerError::Invariant)?;
        }
        Ok(())
    }
    /// Query the retained actual leaf without selection or repeat generation.
    pub fn physical_repeat_target(&self, device: DeviceInstanceId, key: KeyId) -> Option<KeyId> {
        if self.failed {
            return None;
        }
        self.active
            .get(&(device, key))
            .and_then(|plan| plan.events().first())
            .and_then(|event| self.simple.physical_repeat_target(event.payload.slot))
    }
    /// Lifecycle reset does not consume input sequence or reuse source identities.
    /// Native callers must stop capture first and recover their accepted ledger separately.
    pub fn reset(&mut self, at: MonoTime) -> Result<PhysicalStep, PhysicalControllerError> {
        self.check_time(at)?;
        self.now = at;
        let mut step = self.step(CaptureDecision::Capture);
        let result = self.reset_inner(at, &mut step);
        self.failed = result.is_err();
        result?;
        step.next_deadline = self.next_deadline();
        Ok(step)
    }
    fn reset_inner(
        &mut self,
        at: MonoTime,
        step: &mut PhysicalStep,
    ) -> Result<(), PhysicalControllerError> {
        let actual = self.simple.reset(at)?;
        self.history.reset(at)?;
        self.collect(actual, step)?;
        self.active.clear();
        self.pending.clear();
        if !self.source_leases.is_empty() || self.reserved_sources != 0 {
            return Err(PhysicalControllerError::Invariant);
        }
        self.failed = false;
        Ok(())
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
        let bounds = config.timed_stage_source_bounds();
        self.simple.activate(config)?;
        self.source_bounds = bounds;
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
            forwarded_pointers: Vec::new(),
            repeat_target: None,
            next_deadline: self.next_deadline(),
        }
    }
    fn collect(
        &mut self,
        actual: SimpleStep,
        step: &mut PhysicalStep,
    ) -> Result<(), PhysicalControllerError> {
        step.outputs.extend(actual.outputs);
        step.forwarded_pointers.extend(actual.forwarded_pointers);
        for slot in actual.released_producers {
            self.pending.remove(&slot);
            self.release_source_lease(slot)?;
        }
        if !actual.discarded_input_batches.is_empty() {
            let discarded: BTreeSet<_> = actual.discarded_input_batches.into_iter().collect();
            self.active
                .retain(|_, plan| !discarded.contains(&plan.origin.batch));
            self.pending.retain(|slot| !discarded.contains(&slot.batch));
            let slots: Vec<_> = self
                .source_leases
                .keys()
                .copied()
                .filter(|slot| discarded.contains(&slot.batch))
                .collect();
            for slot in slots {
                self.release_source_lease(slot)?;
            }
        }
        Ok(())
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
        self.collect(actual, step)?;
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
        if let Err(error) = self.collect(actual, &mut step) {
            self.failed = true;
            return Err(error);
        }
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
            if matches!(
                input.event,
                InputEvent::Wheel { .. } | InputEvent::PointerMove { .. }
            ) {
                self.prepare_origin()?;
            }
            match &input.event {
                InputEvent::Key { .. }
                | InputEvent::Wheel { .. }
                | InputEvent::PointerMove { .. } => {
                    self.history.check_device(input.device, false)?;
                    if self.next_deadline().is_some_and(|due| due <= input.at_ns) {
                        return Err(PhysicalControllerError::DeadlinePending);
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
                self.prepare_origin()?;
                let reclaimed = self
                    .source_leases
                    .values()
                    .filter(|lease| {
                        lease.device.id == input.device.id
                            && lease.device.generation < input.device.generation
                    })
                    .map(|lease| lease.units)
                    .sum::<usize>();
                let retained = self
                    .reserved_sources
                    .checked_sub(reclaimed)
                    .ok_or(PhysicalControllerError::Invariant)?;
                if self.source_cost(*key) > MAX_OUTPUT_OWNERS.saturating_sub(retained) {
                    return Err(EngineError::ResourceLimit.into());
                }
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
    fn queue_pointer(
        &mut self,
        device: DeviceInstanceId,
        at: MonoTime,
        payload: crate::pointer_work::PointerMotion,
        step: &mut PhysicalStep,
    ) -> Result<(), PhysicalControllerError> {
        let (source, following) = self.prepare_origin()?;
        self.observe(device, false, at, step)?;
        self.simple.enqueue_pointer(crate::timeline::StageEvent {
            stage: Stage::Complex,
            at_ns: at,
            device,
            source,
            config: self.snapshot().clone(),
            payload,
        })?;
        self.next_source = following;
        step.capture = CaptureDecision::Capture;
        Ok(())
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
                        let (source, following) = self.prepare_origin()?;
                        let plan = plan_actions(
                            &[key.into()],
                            PlanOrigin {
                                batch: id,
                                stage: Stage::Simple,
                                start: input.at_ns,
                                device: input.device,
                                source,
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
                        let cost = self.source_cost(key);
                        self.reserved_sources = self
                            .reserved_sources
                            .checked_add(cost)
                            .filter(|total| *total <= MAX_OUTPUT_OWNERS)
                            .ok_or(PhysicalControllerError::Invariant)?;
                        self.source_leases.insert(
                            down.payload.slot,
                            SourceLease {
                                device: input.device,
                                units: cost,
                            },
                        );
                        self.next_source = following;
                        let actual = self.simple.admit(
                            &DueEvent {
                                event: down,
                                delivered_at: input.at_ns,
                            },
                            &self.context,
                        )?;
                        self.active.insert(identity, plan);
                        self.collect(actual, step)?;
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
                        self.collect(actual, step)?;
                    }
                    KeyEdge::Repeat => {
                        step.repeat_target = self.physical_repeat_target(input.device, key);
                    }
                }
            }
            InputEvent::Wheel {
                horizontal,
                vertical,
            } => {
                self.queue_pointer(
                    input.device,
                    input.at_ns,
                    crate::pointer_work::PointerMotion::Wheel {
                        horizontal,
                        vertical,
                    },
                    step,
                )?;
            }
            InputEvent::PointerMove { x, y } => {
                self.queue_pointer(
                    input.device,
                    input.at_ns,
                    crate::pointer_work::PointerMotion::Move { x, y },
                    step,
                )?;
            }
            InputEvent::ContextChanged { context } => {
                self.context = context;
                step.capture = CaptureDecision::Pass;
            }
            InputEvent::DeviceRemoved => self.observe(input.device, true, input.at_ns, step)?,
            InputEvent::Reset => self.reset_inner(input.at_ns, step)?,
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

#[cfg(test)]
mod origin_domain_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn final_origin_identity_is_used_once_and_reset_does_not_reopen_domain_or_consume_rejected_sequence()
     {
        let boundary = crate::source_identity::ORIGIN_DOMAIN_START;
        let mut actor =
            PhysicalStageController::new(Arc::new(ConfigSnapshot::empty(ConfigGeneration(1))))
                .unwrap();
        actor.next_source = boundary - 1;
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        let key = "a".parse::<KeyId>().unwrap();
        let event = |seq, event| InputEnvelope {
            seq,
            at_ns: MonoTime(100),
            device,
            provenance: Provenance::Physical,
            event,
        };
        actor
            .handle(event(
                1,
                InputEvent::Key {
                    key,
                    edge: KeyEdge::Down,
                },
            ))
            .unwrap();
        assert_eq!(
            actor.active[&(device, key)].origin.source,
            SourceToken(u64::MAX)
        );
        assert_eq!(actor.next_source, boundary);
        actor.handle(event(2, InputEvent::Reset)).unwrap();
        assert_eq!(actor.next_source, boundary);
        assert_eq!(actor.reserved_sources(), 0);
        assert!(matches!(
            actor.handle(event(
                3,
                InputEvent::Key {
                    key,
                    edge: KeyEdge::Down
                }
            )),
            Err(PhysicalControllerError::SourceExhausted)
        ));
        assert!(matches!(
            actor.handle(event(3, InputEvent::PointerMove { x: 1, y: 2 })),
            Err(PhysicalControllerError::SourceExhausted)
        ));
        assert_eq!(actor.last_seq, Some(2));
        assert!(!actor.is_failed());
    }
}
