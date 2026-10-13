use super::*;
use crate::simultaneous::{ArbitrationChoice, ChordArbiter, MAX_CHORD_EVENTS};
use std::collections::VecDeque;

pub(super) struct PendingArbitration {
    arbiter: ChordArbiter,
    view: ProducerDecision,
    deadline: Option<MonoTime>,
    dirty: bool,
    devices: Vec<DeviceInstanceId>,
}
pub(super) enum ReplaySelection {
    Decide,
    Ordinary,
    Selected(Option<rule_selection::RuleSelection>),
}
pub(super) struct ReplayInput {
    due: DueEvent<PlannedAction>,
    selection: ReplaySelection,
}
impl ComplexStageActor {
    /// Lower output/timers remain independent. Suppress timeout while a known input is inside
    /// the inclusive window; an idle queue does not prove end-of-stream at the endpoint itself.
    pub fn next_arbitrated_deadline(&self, frontier: Option<MonoTime>) -> Option<MonoTime> {
        let raw_frontier = self
            .replay
            .front()
            .map(|input| input.due.event.at_ns)
            .into_iter()
            .chain(frontier)
            .min();
        let waiting = self.arbitration.as_ref().and_then(|waiting| {
            if waiting.dirty {
                Some(self.now)
            } else {
                waiting
                    .deadline
                    .filter(|end| raw_frontier.is_none_or(|next| next > *end))
                    .and_then(|end| end.checked_add(1))
            }
        });
        self.next_deadline()
            .into_iter()
            .chain(waiting)
            .chain((!self.replay.is_empty()).then_some(self.now))
            .min()
    }
    pub fn buffered_arbitration_inputs(&self) -> usize {
        self.replay.len()
            + self
                .arbitration
                .as_ref()
                .map_or(0, |waiting| waiting.arbiter.buffered())
    }
    /// Accept one registered input into the owned arbiter. Caller retains its receipt until Ok.
    /// Drain indicated lower/replay/yield work first; frontier is the next real upstream history.
    pub fn admit_arbitrated(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
        frontier: Option<MonoTime>,
    ) -> Result<ComplexStep, ComplexActorError> {
        self.running()?;
        self.check_time(due.delivered_at)?;
        if self
            .next_deadline()
            .is_some_and(|deadline| deadline <= due.delivered_at)
            || !self.replay.is_empty()
            || self
                .arbitration
                .as_ref()
                .is_some_and(|waiting| waiting.dirty)
        {
            return Err(ComplexActorError::DeadlinePending);
        }
        let result = self.accept_arbitrated(due, context, frontier);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn check_arbitration_receipt(
        &self,
        due: &DueEvent<PlannedAction>,
    ) -> Result<(), ComplexActorError> {
        let event = &due.event;
        let slot = event.payload.slot;
        let input = self
            .inputs
            .get(&slot.batch)
            .ok_or(ComplexActorError::Input)?;
        if event.stage != Stage::Complex
            || event.device != input.plan.origin.device
            || event.source != input.plan.origin.source
            || !Arc::ptr_eq(&event.config, &input.plan.origin.config)
            || due.delivered_at < event.at_ns
        {
            return Err(ComplexActorError::Input);
        }
        if event.payload.edge == KeyEdge::Down {
            self.registered_group_down(event)?;
        } else if event.payload.edge == KeyEdge::Up {
            let expected = input
                .plan
                .events()
                .iter()
                .find(|other| other.payload.slot == slot && other.payload.edge == KeyEdge::Up);
            let (payload, earliest) = match expected {
                Some(other) => (&other.payload, other.at_ns),
                None => (
                    input
                        .plan
                        .releases()
                        .iter()
                        .find(|other| other.slot == slot)
                        .ok_or(ComplexActorError::Input)?,
                    input.plan.ends_at(),
                ),
            };
            if &event.payload != payload || event.at_ns < earliest {
                return Err(ComplexActorError::Input);
            }
        } else {
            return Err(ComplexActorError::Input);
        }
        Ok(())
    }
    fn interrupt_arbitrated_down(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
        self.held = None;
        if matches!(due.event.payload.target, ActionTarget::Key(key) if key.usage != 0) {
            self.alone_candidate = None;
        }
        if let Some((_, previous)) = self.delayed.take() {
            self.branch(
                previous,
                rule_selection::RuleBranch::DelayedCanceled,
                context,
                due.delivered_at,
            )?;
            self.finish(previous, step)?;
        }
        Ok(())
    }
    fn accept_arbitrated(
        &mut self,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
        frontier: Option<MonoTime>,
    ) -> Result<ComplexStep, ComplexActorError> {
        self.check_arbitration_receipt(due)?;
        if self.buffered_arbitration_inputs() >= MAX_CHORD_EVENTS {
            return Err(
                simultaneous::GroupOwnerError::Chord(simultaneous::ChordError::Capacity).into(),
            );
        }
        let mut step = ComplexStep::default();
        let owned = DueEvent {
            event: due.event.clone(),
            delivered_at: due.delivered_at,
        };
        if let Some(waiting) = self.arbitration.as_mut() {
            waiting
                .arbiter
                .observe(owned)
                .map_err(|rejected| simultaneous::GroupOwnerError::Chord(rejected.error))?;
            waiting.dirty = true;
            if !waiting.devices.contains(&due.event.device) {
                waiting.devices.push(due.event.device);
            }
        } else if due.event.payload.edge == KeyEdge::Down
            && matches!(due.event.payload.target, ActionTarget::Key(key) if key.page == keyweave_types::KEYBOARD_PAGE)
        {
            self.interrupt_arbitrated_down(due, context, &mut step)?;
            let (_, identity) = self.registered_group_down(&due.event)?;
            let view =
                ProducerDecision::new(due.event.clone(), due.delivered_at, Stage::Output, None)
                    .with_device_identity(identity);
            let arbiter = ChordArbiter::new(owned)
                .map_err(|rejected| simultaneous::GroupOwnerError::Chord(rejected.error))?;
            self.arbitration = Some(PendingArbitration {
                arbiter,
                view,
                deadline: None,
                dirty: true,
                devices: vec![due.event.device],
            });
        } else {
            if due.event.payload.edge == KeyEdge::Down {
                self.interrupt_arbitrated_down(due, context, &mut step)?;
            }
            self.replay.push_front(ReplayInput {
                due: owned,
                selection: if due.event.payload.edge == KeyEdge::Down {
                    ReplaySelection::Ordinary
                } else {
                    ReplaySelection::Selected(None)
                },
            });
        }
        self.now = due.delivered_at;
        if !self
            .next_deadline()
            .is_some_and(|deadline| deadline <= self.now)
            && self.arbitration.is_some()
        {
            let (actual, _) = self.advance_arbitration(self.now, context, frontier)?;
            Self::append_arbitration_step(&mut step, actual);
        }
        Ok(step)
    }
    fn append_arbitration_step(step: &mut ComplexStep, actual: ComplexStep) {
        step.outputs.extend(actual.outputs);
        step.forwarded_pointers.extend(actual.forwarded_pointers);
        step.released_producers.extend(actual.released_producers);
        step.discarded_input_batches
            .extend(actual.discarded_input_batches);
        if actual.originating_halt.is_some() {
            step.originating_halt = actual.originating_halt;
        }
    }
    fn prepend_replay(
        &mut self,
        inputs: Vec<DueEvent<PlannedAction>>,
        selected: Option<ReplaySelection>,
    ) -> Result<(), ComplexActorError> {
        if inputs.len() + self.buffered_arbitration_inputs() > MAX_CHORD_EVENTS {
            return Err(
                simultaneous::GroupOwnerError::Chord(simultaneous::ChordError::Capacity).into(),
            );
        }
        let mut selection = selected;
        let mut prefix = VecDeque::new();
        for due in inputs {
            prefix.push_back(ReplayInput {
                due,
                selection: selection.take().unwrap_or(ReplaySelection::Decide),
            });
        }
        prefix.append(&mut self.replay);
        self.replay = prefix;
        Ok(())
    }
    fn advance_arbitration(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
        frontier: Option<MonoTime>,
    ) -> Result<(ComplexStep, bool), ComplexActorError> {
        let waiting = self
            .arbitration
            .as_mut()
            .ok_or(ComplexActorError::Invariant)?;
        let frontier = self
            .replay
            .front()
            .map(|input| input.due.event.at_ns)
            .into_iter()
            .chain(frontier)
            .min();
        // None means idle, not physical EOF: equality must still accept a boundary input.
        let guarded = match waiting.deadline {
            Some(end) if at > end => frontier,
            _ => Some(frontier.map_or(at, |next| next.min(at))),
        };
        let modifiers = self.output.generated_modifiers();
        let result = waiting
            .arbiter
            .advance(at, guarded, &modifiers, |condition| {
                self.output
                    .condition_truth_for_producer(condition, context, &waiting.view, at)
            })
            .map_err(simultaneous::GroupOwnerError::Chord)?;
        match result.choice {
            ArbitrationChoice::Pending { deadline } => {
                deadline
                    .checked_add(1)
                    .ok_or(ComplexActorError::TimeOverflow)?;
                let waiting = self
                    .arbitration
                    .as_mut()
                    .ok_or(ComplexActorError::Invariant)?;
                waiting.deadline = Some(deadline);
                waiting.dirty = false;
                Ok((ComplexStep::default(), false))
            }
            ArbitrationChoice::Yield { deadline, .. } => {
                let waiting = self
                    .arbitration
                    .as_mut()
                    .ok_or(ComplexActorError::Invariant)?;
                waiting.deadline = deadline;
                waiting.dirty = true;
                Ok((ComplexStep::default(), true))
            }
            ArbitrationChoice::Ordinary { selection, inputs } => {
                self.arbitration = None;
                self.prepend_replay(inputs, Some(ReplaySelection::Selected(Some(selection))))?;
                Ok((ComplexStep::default(), true))
            }
            ArbitrationChoice::Pass { inputs } => {
                self.arbitration = None;
                self.prepend_replay(inputs, Some(ReplaySelection::Selected(None)))?;
                Ok((ComplexStep::default(), true))
            }
            ArbitrationChoice::Matched {
                selection,
                group,
                tail,
            } => {
                self.arbitration = None;
                let chosen = self.admit_matched_group(group, selection, at)?;
                let mut inputs = chosen.replay;
                inputs.extend(tail);
                self.prepend_replay(inputs, None)?;
                Ok((chosen.step, true))
            }
        }
    }
    /// One bounded lower/arbiter/replay step. A stable Pending returns None so callers can
    /// deliver their known frontier input rather than spinning on a stale timeout.
    pub fn poll_arbitrated(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
        frontier: Option<MonoTime>,
    ) -> Result<Option<ComplexStep>, ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        let result = self.poll_arbitrated_inner(at, context, frontier);
        if result.is_err() {
            self.failed = true;
        }
        self.now = at;
        result
    }
    fn poll_arbitrated_inner(
        &mut self,
        at: MonoTime,
        context: &ContextSnapshot,
        frontier: Option<MonoTime>,
    ) -> Result<Option<ComplexStep>, ComplexActorError> {
        if self.next_deadline().is_some_and(|deadline| deadline <= at) {
            return self.poll_with_context(at, context);
        }
        if self
            .arbitration
            .as_ref()
            .is_some_and(|waiting| waiting.dirty)
        {
            let (step, _) = self.advance_arbitration(at, context, frontier)?;
            return Ok(Some(step));
        }
        if let Some(mut input) = self.replay.pop_front() {
            input.due.delivered_at = at;
            let step = match input.selection {
                ReplaySelection::Selected(selection) => {
                    self.admit_inner(&input.due, context, |_| true, Some(selection), true)?
                }
                ReplaySelection::Decide => self.accept_arbitrated(&input.due, context, frontier)?,
                ReplaySelection::Ordinary => {
                    self.admit_inner(&input.due, context, |_| true, None, true)?
                }
            };
            return Ok(Some(step));
        }
        if self.arbitration.is_some() {
            let (step, changed) = self.advance_arbitration(at, context, frontier)?;
            return Ok(changed.then_some(step));
        }
        Ok(None)
    }
    pub(super) fn cancel_arbitration_device(&mut self, device: DeviceInstanceId, removed: bool) {
        let invalidated = |due: &DueEvent<PlannedAction>| {
            due.event.device.id == device.id
                && (due.event.device.generation < device.generation
                    || (removed && due.event.device.generation == device.generation))
        };
        let affected = self.arbitration.as_ref().is_some_and(|waiting| {
            waiting.devices.iter().any(|source| {
                source.id == device.id
                    && (source.generation < device.generation
                        || (removed && source.generation == device.generation))
            })
        });
        if affected && let Some(waiting) = self.arbitration.take() {
            let inputs = waiting.arbiter.into_inputs();
            let mut prefix = VecDeque::new();
            for due in inputs {
                if !invalidated(&due) {
                    prefix.push_back(ReplayInput {
                        due,
                        selection: ReplaySelection::Decide,
                    });
                }
            }
            prefix.append(&mut self.replay);
            self.replay = prefix;
        }
        self.replay.retain(|input| !invalidated(&input.due));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{action_plan::*, simultaneous::*, timeline::*};
    fn key(name: &str) -> KeyId {
        name.parse().unwrap()
    }
    fn rule(from: &str, to: &str) -> Mapping {
        Mapping {
            id: from.into(),
            from: key(from),
            to: vec![key(to).into()],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }
    }
    fn cfg(normal: bool, extras: impl FnOnce(&mut MappingExtras)) -> Arc<ConfigSnapshot> {
        let mut group = rule("a", "c");
        let mut spec = Simultaneous::new(vec![key("a"), key("b")]);
        spec.threshold_ns = 50;
        group.extras.simultaneous = Some(spec);
        extras(&mut group.extras);
        let mut complex = vec![group];
        if normal {
            complex.push(rule("a", "d"));
        }
        Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "arbitrated actor".into(),
            simple: vec![],
            complex,
        })
    }
    fn setup(cfg: &Arc<ConfigSnapshot>, names: &[&str]) -> (ComplexStageActor, Vec<ActionPlan>) {
        let mut actor = ComplexStageActor::from_output(OutputStageActor::new(cfg.clone()).unwrap());
        let mut plans = vec![];
        for (index, name) in names.iter().enumerate() {
            let id = index as u64 + 1;
            let at = 100 + index as u64 * 10;
            let device = DeviceInstanceId { id, generation: 1 };
            actor.observe_device(device, false, MonoTime(0)).unwrap();
            let plan = plan_actions(
                &[key(name).into()],
                PlanOrigin {
                    batch: BatchId(id),
                    stage: Stage::Complex,
                    start: MonoTime(at),
                    device,
                    source: SourceToken(id),
                    config: cfg.clone(),
                },
                false,
            )
            .unwrap();
            actor.register_input(plan.clone()).unwrap();
            plans.push(plan);
        }
        (actor, plans)
    }
    fn down(plan: &ActionPlan) -> DueEvent<PlannedAction> {
        DueEvent {
            event: plan.events()[0].clone(),
            delivered_at: plan.origin.start,
        }
    }
    fn up(plan: &ActionPlan, at: u64) -> DueEvent<PlannedAction> {
        DueEvent {
            event: StageEvent {
                stage: Stage::Complex,
                at_ns: MonoTime(at),
                device: plan.origin.device,
                source: plan.origin.source,
                config: plan.origin.config.clone(),
                payload: plan.releases()[0].clone(),
            },
            delivered_at: MonoTime(at),
        }
    }
    fn drain(
        actor: &mut ComplexStageActor,
        at: u64,
        frontier: Option<u64>,
    ) -> Vec<(KeyId, KeyEdge)> {
        let mut edges = vec![];
        for _ in 0..1000 {
            let Some(step) = actor
                .poll_arbitrated(
                    MonoTime(at),
                    &ContextSnapshot::default(),
                    frontier.map(MonoTime),
                )
                .unwrap()
            else {
                return edges;
            };
            edges.extend(
                step.outputs
                    .into_iter()
                    .map(|intent| (intent.key, intent.edge)),
            );
        }
        panic!("bounded scheduler did not stabilize")
    }
    fn context(vendor: u32) -> ContextSnapshot {
        ContextSnapshot {
            revision: 1,
            observed_at_ns: MonoTime(0),
            valid_until_ns: MonoTime(1000),
            devices: Some(vec![DeviceContext {
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                properties: DeviceProperties {
                    vendor_id: Some(vendor),
                    ..Default::default()
                },
            }]),
            ..Default::default()
        }
    }
    fn device_condition(vendor: u32) -> Condition {
        Condition::Device {
            identifiers: vec![DeviceProperties {
                vendor_id: Some(vendor),
                ..Default::default()
            }],
            unless: false,
            exists: false,
        }
    }
    #[test]
    fn automatic_actor_holds_first_then_matches_group_and_replays_original_member_ups() {
        let cfg = cfg(false, |_| {});
        assert!(ComplexStageActor::new(cfg.clone()).is_err());
        let (mut actor, plans) = setup(&cfg, &["a", "b"]);
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(110)),
            )
            .unwrap();
        assert!(actor.producer(plans[0].events()[0].payload.slot).is_none());
        assert_eq!(actor.buffered_arbitration_inputs(), 1);
        actor
            .admit_arbitrated(&down(&plans[1]), &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 110, None), [(key("c"), KeyEdge::Down)]);
        actor
            .admit_arbitrated(&up(&plans[0], 120), &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 120, None), [(key("c"), KeyEdge::Up)]);
        actor
            .admit_arbitrated(&up(&plans[1], 130), &ContextSnapshot::default(), None)
            .unwrap();
        drain(&mut actor, 130, None);
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn known_frontier_blocks_late_timeout_and_boundary_history_matches_at_actual_clock() {
        let cfg = cfg(true, |_| {});
        let (mut actor, plans) = setup(&cfg, &["a", "b"]);
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(150)),
            )
            .unwrap();
        assert_eq!(actor.next_arbitrated_deadline(Some(MonoTime(150))), None);
        assert!(
            actor
                .poll_arbitrated(
                    MonoTime(200),
                    &ContextSnapshot::default(),
                    Some(MonoTime(150))
                )
                .unwrap()
                .is_none()
        );
        let mut late = down(&plans[1]);
        late.event.at_ns = MonoTime(150);
        late.delivered_at = MonoTime(200);
        actor
            .admit_arbitrated(&late, &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 200, None), [(key("c"), KeyEdge::Down)]);
        let saved = actor.producer(plans[0].events()[0].payload.slot).unwrap();
        assert_eq!(saved.down().at_ns, MonoTime(100));
        assert_eq!(saved.delivered_at(), MonoTime(200));
        actor.reset(MonoTime(210)).unwrap();
    }
    #[test]
    fn idle_endpoint_does_not_expire_even_when_first_input_is_delivered_at_endpoint() {
        let cfg = cfg(true, |_| {});
        let (mut actor, plans) = setup(&cfg, &["a", "b"]);
        let mut first = down(&plans[0]);
        first.delivered_at = MonoTime(150);
        actor
            .admit_arbitrated(&first, &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(actor.next_arbitrated_deadline(None), Some(MonoTime(151)));
        assert!(
            actor
                .poll_arbitrated(MonoTime(150), &ContextSnapshot::default(), None)
                .unwrap()
                .is_none()
        );
        let mut peer = down(&plans[1]);
        peer.event.at_ns = MonoTime(150);
        peer.delivered_at = MonoTime(150);
        actor
            .admit_arbitrated(&peer, &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 150, None), [(key("c"), KeyEdge::Down)]);
    }
    #[test]
    fn timed_out_ordinary_selection_is_frozen_and_never_rematched_during_replay() {
        let mut snapshot = (*cfg(true, |_| {})).clone();
        snapshot.complex[1].extras.conditions = vec![device_condition(100)];
        snapshot.complex.push(rule("a", "e"));
        let cfg = Arc::new(snapshot);
        let (mut actor, plans) = setup(&cfg, &["a"]);
        actor
            .admit_arbitrated(&down(&plans[0]), &context(100), None)
            .unwrap();
        assert!(
            actor
                .poll_arbitrated(MonoTime(151), &context(100), None)
                .unwrap()
                .is_some()
        );
        let mut edges = vec![];
        while let Some(step) = actor
            .poll_arbitrated(MonoTime(152), &context(200), None)
            .unwrap()
        {
            edges.extend(
                step.outputs
                    .into_iter()
                    .map(|intent| (intent.key, intent.edge)),
            );
        }
        assert_eq!(edges, [(key("d"), KeyEdge::Down)]);
        actor
            .admit_arbitrated(&up(&plans[0], 160), &context(200), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 160, None), [(key("d"), KeyEdge::Up)]);
    }
    #[test]
    fn pending_rule_conditions_are_rechecked_before_timeout_and_fallback_uses_fresh_view() {
        let cfg = cfg(true, |extras| {
            extras.conditions = vec![device_condition(100)]
        });
        let (mut actor, plans) = setup(&cfg, &["a"]);
        actor
            .admit_arbitrated(&down(&plans[0]), &context(100), None)
            .unwrap();
        assert!(
            actor
                .poll_arbitrated(MonoTime(110), &context(200), None)
                .unwrap()
                .is_some()
        );
        assert_eq!(drain(&mut actor, 110, None), [(key("d"), KeyEdge::Down)]);
    }
    #[test]
    fn interruption_replay_keeps_original_order_and_each_following_down_enters_arbitration() {
        let cfg = cfg(true, |_| {});
        let (mut actor, plans) = setup(&cfg, &["a", "x"]);
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(110)),
            )
            .unwrap();
        actor
            .admit_arbitrated(&down(&plans[1]), &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(
            drain(&mut actor, 110, None),
            [(key("d"), KeyEdge::Down), (key("x"), KeyEdge::Down)]
        );
    }
    #[test]
    fn replay_alone_uses_original_event_span_instead_of_collapsed_delivery_times() {
        for timeout in [5, 15] {
            let mut snapshot = (*cfg(true, |_| {})).clone();
            snapshot.complex[1].extras.if_alone = vec![key("e").into()];
            snapshot.complex[1].extras.alone_timeout_ns = timeout;
            let cfg = Arc::new(snapshot);
            let (mut actor, plans) = setup(&cfg, &["a"]);
            actor
                .admit_arbitrated(&down(&plans[0]), &ContextSnapshot::default(), None)
                .unwrap();
            actor
                .admit_arbitrated(&up(&plans[0], 110), &ContextSnapshot::default(), None)
                .unwrap();
            let mut expected = vec![(key("d"), KeyEdge::Down), (key("d"), KeyEdge::Up)];
            if timeout == 15 {
                expected.extend([(key("e"), KeyEdge::Down), (key("e"), KeyEdge::Up)]);
            }
            assert_eq!(drain(&mut actor, 110, None), expected);
            assert_eq!(actor.reserved_sources(), 0);
        }
    }
    #[test]
    fn long_rule_catalog_yields_and_resumes_without_rejecting_or_reactivating_first_input() {
        let mut snapshot = (*cfg(false, |_| {})).clone();
        let mut catalog = Vec::new();
        for index in 0..130 {
            let mut mapping = rule("z", "x");
            mapping.id = format!("skip-{index}");
            catalog.push(mapping);
        }
        catalog.append(&mut snapshot.complex);
        snapshot.complex = catalog;
        let cfg = Arc::new(snapshot);
        let (mut actor, plans) = setup(&cfg, &["a", "b"]);
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(110)),
            )
            .unwrap();
        assert_eq!(
            actor.next_arbitrated_deadline(Some(MonoTime(110))),
            Some(MonoTime(100))
        );
        assert!(matches!(
            actor.admit_arbitrated(&down(&plans[1]), &ContextSnapshot::default(), None),
            Err(ComplexActorError::DeadlinePending)
        ));
        assert!(drain(&mut actor, 100, Some(110)).is_empty());
        assert!(!actor.is_failed());
        actor
            .admit_arbitrated(&down(&plans[1]), &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 110, None), [(key("c"), KeyEdge::Down)]);
    }
    #[test]
    fn reset_waiting_cancels_owned_inputs_without_manufacturing_output() {
        let cfg = cfg(false, |_| {});
        let (mut actor, plans) = setup(&cfg, &["a"]);
        actor
            .admit_arbitrated(&down(&plans[0]), &ContextSnapshot::default(), None)
            .unwrap();
        let reset = actor.reset(MonoTime(110)).unwrap();
        assert!(reset.outputs.is_empty());
        assert_eq!(actor.buffered_arbitration_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn device_control_preserves_unrelated_window_and_requeues_surviving_original_history() {
        let cfg = cfg(true, |extras| {
            extras.simultaneous.as_mut().unwrap().key_up_order = KeyOrder::Strict
        });
        let (mut actor, plans) = setup(&cfg, &["a", "b"]);
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(110)),
            )
            .unwrap();
        actor
            .admit_arbitrated(&down(&plans[1]), &ContextSnapshot::default(), None)
            .unwrap();
        let unrelated = DeviceInstanceId {
            id: 3,
            generation: 1,
        };
        actor
            .observe_device(unrelated, false, MonoTime(111))
            .unwrap();
        actor
            .observe_device(unrelated, true, MonoTime(112))
            .unwrap();
        assert_eq!(actor.buffered_arbitration_inputs(), 2);
        let removed = actor
            .observe_device(plans[1].origin.device, true, MonoTime(115))
            .unwrap();
        assert!(removed.outputs.is_empty());
        assert_eq!(actor.buffered_arbitration_inputs(), 1);
        assert_eq!(drain(&mut actor, 151, None), [(key("d"), KeyEdge::Down)]);
        let saved = actor.producer(plans[0].events()[0].payload.slot).unwrap();
        assert_eq!(saved.down().at_ns, MonoTime(100));
        assert_eq!(saved.delivered_at(), MonoTime(151));
    }
    #[test]
    fn consumer_down_interrupts_keyboard_window_and_ordinary_consumer_mapping_is_preserved() {
        let mut snapshot = (*cfg(true, |_| {})).clone();
        snapshot
            .complex
            .push(rule("consumer:volume_increment", "f"));
        let cfg = Arc::new(snapshot);
        let (mut actor, plans) = setup(&cfg, &["a", "consumer:volume_increment"]);
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(110)),
            )
            .unwrap();
        actor
            .admit_arbitrated(&down(&plans[1]), &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(
            drain(&mut actor, 110, None),
            [(key("d"), KeyEdge::Down), (key("f"), KeyEdge::Down)]
        );
    }
    fn variable(value: i64) -> Action {
        Action {
            target: ActionTarget::SetVariable(VariableAction {
                name: "flag".into(),
                operation: VariableOperation::Set {
                    value: Some(VariableValue::Integer(value)),
                    key_up_value: None,
                },
            }),
            ..key("vk_none").into()
        }
    }
    #[test]
    fn variable_receipt_is_neutral_to_window_and_replays_before_later_original_inputs() {
        let cfg = cfg(false, |extras| extras.after_key_up = vec![variable(9)]);
        let (mut actor, plans) = setup(&cfg, &["a", "b"]);
        let plan = plan_actions(
            &[variable(1)],
            PlanOrigin {
                batch: BatchId(3),
                stage: Stage::Complex,
                start: MonoTime(105),
                device: plans[0].origin.device,
                source: SourceToken(3),
                config: cfg.clone(),
            },
            false,
        )
        .unwrap();
        actor.register_input(plan.clone()).unwrap();
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(105)),
            )
            .unwrap();
        actor
            .admit_arbitrated(
                &down(&plan),
                &ContextSnapshot::default(),
                Some(MonoTime(110)),
            )
            .unwrap();
        assert!(actor.producer(plans[0].events()[0].payload.slot).is_none());
        actor
            .admit_arbitrated(&down(&plans[1]), &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 110, None), [(key("c"), KeyEdge::Down)]);
        assert_eq!(
            actor.generated_variables()["flag"],
            VariableValue::Integer(1)
        );
        actor
            .admit_arbitrated(&up(&plan, 115), &ContextSnapshot::default(), None)
            .unwrap();
        drain(&mut actor, 115, None);
        assert_eq!(
            actor.generated_variables()["flag"],
            VariableValue::Integer(1)
        );
        actor.reset(MonoTime(125)).unwrap();
    }
    #[test]
    fn previous_delayed_cancellation_changes_generated_view_before_new_group_selection() {
        let mut snapshot = (*cfg(false, |extras| {
            extras.conditions = vec![Condition::Variable {
                name: "flag".into(),
                value: VariableValue::Integer(1),
                unless: false,
            }]
        }))
        .clone();
        let mut previous = rule("x", "x");
        previous.extras.delayed = Some(DelayedAction {
            delay_ns: 1000,
            if_invoked: vec![],
            if_canceled: vec![variable(1)],
        });
        snapshot.complex.push(previous);
        let cfg = Arc::new(snapshot);
        let (mut actor, plans) = setup(&cfg, &["x", "a", "b"]);
        actor
            .admit_arbitrated(
                &down(&plans[0]),
                &ContextSnapshot::default(),
                Some(MonoTime(110)),
            )
            .unwrap();
        assert_eq!(
            drain(&mut actor, 100, Some(110)),
            [(key("x"), KeyEdge::Down)]
        );
        actor
            .admit_arbitrated(
                &down(&plans[1]),
                &ContextSnapshot::default(),
                Some(MonoTime(120)),
            )
            .unwrap();
        assert!(drain(&mut actor, 110, Some(120)).is_empty());
        assert_eq!(
            actor.generated_variables()["flag"],
            VariableValue::Integer(1)
        );
        actor
            .admit_arbitrated(&down(&plans[2]), &ContextSnapshot::default(), None)
            .unwrap();
        assert_eq!(drain(&mut actor, 120, None), [(key("c"), KeyEdge::Down)]);
        actor.reset(MonoTime(130)).unwrap();
    }
    #[test]
    fn owned_window_buffer_is_bounded_and_rejection_keeps_all_accepted_inputs_until_reset() {
        let cfg = cfg(false, |extras| {
            extras
                .simultaneous
                .as_mut()
                .unwrap()
                .detect_key_down_uninterruptedly = true
        });
        let (mut actor, first) = setup(&cfg, &["a"]);
        actor
            .admit_arbitrated(&down(&first[0]), &ContextSnapshot::default(), None)
            .unwrap();
        for id in 2..=257 {
            let plan = plan_actions(
                &[key("x").into()],
                PlanOrigin {
                    batch: BatchId(id),
                    stage: Stage::Complex,
                    start: MonoTime(101),
                    device: first[0].origin.device,
                    source: SourceToken(id),
                    config: cfg.clone(),
                },
                false,
            )
            .unwrap();
            actor.register_input(plan.clone()).unwrap();
            let result = actor.admit_arbitrated(&down(&plan), &ContextSnapshot::default(), None);
            if id <= 256 {
                result.unwrap();
            } else {
                assert!(result.is_err());
            }
        }
        assert!(actor.is_failed());
        assert_eq!(actor.buffered_arbitration_inputs(), 256);
        assert_eq!(actor.producers.live_slots(), 0);
        let reset = actor.reset(MonoTime(101)).unwrap();
        assert!(reset.outputs.is_empty());
        assert_eq!(actor.buffered_arbitration_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn inclusive_window_wakeup_overflow_fails_without_wrapping_or_emitting_keys() {
        let cfg = cfg(false, |_| {});
        let mut actor = ComplexStageActor::from_output(OutputStageActor::new(cfg.clone()).unwrap());
        let device = DeviceInstanceId {
            id: 1,
            generation: 1,
        };
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let at = MonoTime(u64::MAX - 50);
        let plan = plan_actions(
            &[key("a").into()],
            PlanOrigin {
                batch: BatchId(1),
                stage: Stage::Complex,
                start: at,
                device,
                source: SourceToken(1),
                config: cfg.clone(),
            },
            false,
        )
        .unwrap();
        actor.register_input(plan.clone()).unwrap();
        assert!(matches!(
            actor.admit_arbitrated(&down(&plan), &ContextSnapshot::default(), None),
            Err(ComplexActorError::TimeOverflow)
        ));
        assert!(actor.is_failed());
        assert_eq!(actor.producers.live_slots(), 0);
        let reset = actor.reset(at).unwrap();
        assert!(reset.outputs.is_empty());
        assert_eq!(actor.reserved_sources(), 0);
    }
}
