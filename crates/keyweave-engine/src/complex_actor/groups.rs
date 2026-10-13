use super::*;
use crate::simultaneous::{ChordMatch, GroupBindingOwner, GroupOwnerError, UnfinishedGroupOwner};

/// A chosen group has executed its original member admission; only outside inputs are replayable.
pub struct MatchedGroupStep {
    pub step: ComplexStep,
    pub replay: Vec<DueEvent<PlannedAction>>,
}
impl ComplexStageActor {
    /// Handoff from ordered arbitration. Default construction still gates simultaneous input.
    /// The caller must coordinate input frontier and interrupt previous candidates before selecting.
    pub fn admit_matched_group(
        &mut self,
        group: ChordMatch,
        selection: rule_selection::RuleSelection,
        at: MonoTime,
    ) -> Result<MatchedGroupStep, ComplexActorError> {
        self.running()?;
        self.check_time(at)?;
        if self.next_deadline().is_some_and(|deadline| deadline <= at) {
            return Err(ComplexActorError::DeadlinePending);
        }
        let result = self.admit_matched_group_inner(group, selection, at);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn registered_group_down(
        &self,
        down: &timeline::StageEvent<PlannedAction>,
    ) -> Result<(PlannedAction, bool), ComplexActorError> {
        let slot = down.payload.slot;
        let input = self
            .inputs
            .get(&slot.batch)
            .ok_or(ComplexActorError::Input)?;
        let origin = &input.plan.origin;
        if down.stage != Stage::Complex
            || down.device != origin.device
            || down.source != origin.source
            || !Arc::ptr_eq(&down.config, &origin.config)
        {
            return Err(ComplexActorError::Input);
        }
        let expected = input
            .plan
            .events()
            .iter()
            .find(|event| event.payload.slot == slot && event.payload.edge == KeyEdge::Down)
            .ok_or(ComplexActorError::Input)?;
        if down.payload != expected.payload || down.at_ns < expected.at_ns {
            return Err(ComplexActorError::Input);
        }
        self.producers.check_down(slot)?;
        self.output.check_live_device(down.device)?;
        let up = input
            .plan
            .events()
            .iter()
            .find(|event| event.payload.slot == slot && event.payload.edge == KeyEdge::Up)
            .map(|event| &event.payload)
            .or_else(|| {
                input
                    .plan
                    .releases()
                    .iter()
                    .find(|action| action.slot == slot)
            })
            .cloned()
            .ok_or(ComplexActorError::Input)?;
        Ok((up, !input.plan.snapshot_modifier_slots.contains(&slot)))
    }
    fn admit_matched_group_inner(
        &mut self,
        group: ChordMatch,
        selection: rule_selection::RuleSelection,
        at: MonoTime,
    ) -> Result<MatchedGroupStep, ComplexActorError> {
        let first = group.members().first().ok_or(ComplexActorError::Input)?;
        if selection.stage() != rule_selection::MappingStage::Complex
            || !Arc::ptr_eq(selection.snapshot(), &first.down().event.config)
            || selection.mapping().extras.simultaneous.as_ref() != Some(group.spec())
            || at < group.matched_at()
            || group.events().iter().any(|event| at < event.delivered_at)
        {
            return Err(ComplexActorError::Input);
        }
        let primary = first.down().event.payload.slot;
        let has_initial_up = group.members().iter().any(|member| member.up().is_some());
        let mut credentials = Vec::with_capacity(group.members().len());
        for member in group.members() {
            let (up, identity) = self.registered_group_down(&member.down().event)?;
            if let Some(observed) = member.up() {
                let input = &self.inputs[&up.slot.batch];
                let earliest = input
                    .plan
                    .events()
                    .iter()
                    .find(|event| {
                        event.payload.slot == up.slot && event.payload.edge == KeyEdge::Up
                    })
                    .map_or(input.plan.ends_at(), |event| event.at_ns);
                if observed.event.payload != up || observed.event.at_ns < earliest {
                    return Err(ComplexActorError::Input);
                }
            }
            credentials.push((up, identity));
        }
        let rule = selection.mapping();
        let candidate = !rule.extras.if_alone.is_empty();
        let held = if rule.extras.if_held_down.is_empty() {
            None
        } else {
            Some((
                MonoTime(
                    at.0.checked_add(rule.extras.held_threshold_ns)
                        .ok_or(ComplexActorError::TimeOverflow)?,
                ),
                primary,
            ))
        };
        let delayed = match rule
            .extras
            .delayed
            .as_ref()
            .filter(|branch| !branch.if_invoked.is_empty() || !branch.if_canceled.is_empty())
        {
            Some(branch) => Some((
                MonoTime(
                    at.0.checked_add(branch.delay_ns)
                        .ok_or(ComplexActorError::TimeOverflow)?,
                ),
                primary,
            )),
            None => None,
        };
        let id = self.next_batch_id()?;
        let plan = modifier_scope::ScopedActionPlan::main(
            &selection,
            id,
            at,
            first.down().event.device,
            first.down().event.source,
        )?;
        let slots = plan
            .actions()
            .events()
            .iter()
            .filter(|event| event.payload.edge == KeyEdge::Down)
            .count();
        let generated_halt = plan
            .actions()
            .events()
            .iter()
            .any(|event| event.payload.halt);
        let member_slots: Vec<_> = group
            .members()
            .iter()
            .map(|member| member.down().event.payload.slot)
            .collect();
        let member_downs: Vec<_> = group
            .members()
            .iter()
            .map(|member| member.down().event.clone())
            .collect();
        let projection_references = self.prepared_group_projection_references(&member_slots)?;
        let primary_device = first.down().event.device;
        let mut selected = Some(selection);
        let mut downs = Vec::with_capacity(member_slots.len());
        for (index, (member, (up, identity))) in group.members().iter().zip(credentials).enumerate()
        {
            let down = DueEvent {
                event: member.down().event.clone(),
                delivered_at: at,
            };
            let decision = ProducerDecision::new(
                down.event.clone(),
                at,
                Stage::Output,
                if index == 0 { selected.take() } else { None },
            )
            .with_device_identity(identity)
            .with_original_up(&up)?;
            downs.push((down, decision));
        }
        // Prior interruption belongs to the outer arbiter's Down frame. Never fire old delayed
        // cancellation after its selection has been frozen using a different generated view.
        if self.delayed.is_some() {
            return Err(ComplexActorError::Input);
        }
        let (handle, projection) = if slots == 0 {
            let activation = self
                .producers
                .prepare_shared_consumed_down(plan.actions().origin.clone(), downs)
                .map_err(|rejected| rejected.error)?;
            let handle = self
                .output
                .prepare_shared_empty_child_scoped(plan, activation)?
                .commit();
            (handle, None)
        } else {
            let registration = self
                .producers
                .prepare_shared_child_with_down(plan.actions().origin.clone(), slots, downs)
                .map_err(|rejected| rejected.error)?;
            if projection_references != 0 {
                let (handle, projection) = self
                    .output
                    .prepare_projected_shared_child_scoped(plan, registration)?
                    .commit();
                (handle, Some(projection))
            } else {
                (
                    self.output
                        .prepare_shared_child_scoped(plan, registration)?
                        .commit(),
                    None,
                )
            }
        };
        self.last_batch = id.0;
        let main = (slots != 0).then_some(id);
        if let Some(main) = main {
            self.children.insert(main, primary);
        }
        self.quarantined_group_holds.push(handle);
        self.merge_group_projections(
            primary,
            &member_slots,
            primary_device,
            projection_references,
            projection,
        );
        // The real original Up markers must exist before the owner applies initial lookahead facts.
        for member in group.members() {
            if let Some(up) = member.up() {
                let due = DueEvent {
                    event: up.event.clone(),
                    delivered_at: at,
                };
                let marker = self.producers.prepare_producer_up(&due)?;
                marker.commit();
                let input = self
                    .inputs
                    .get_mut(&due.event.payload.slot.batch)
                    .ok_or(ComplexActorError::Invariant)?;
                input.unreleased = input
                    .unreleased
                    .checked_sub(1)
                    .ok_or(ComplexActorError::Invariant)?;
            }
        }
        let handle = self
            .quarantined_group_holds
            .pop()
            .ok_or(ComplexActorError::Invariant)?;
        let (owner, events) =
            match GroupBindingOwner::new(group, handle, main, &self.producers, &self.output) {
                Ok(owned) => owned,
                Err(rejected) => {
                    self.quarantined_group_holds.push(rejected.handle);
                    return Err(rejected.error.into());
                }
            };
        self.groups.insert(primary, owner);
        for &slot in &member_slots {
            self.group_members.insert(slot, primary);
            self.bindings.insert(slot, main);
        }
        self.held = held;
        self.delayed = delayed;
        self.alone_candidate = candidate.then_some(primary);
        if generated_halt {
            self.halted.insert(primary);
        }
        let mut step = ComplexStep::default();
        if self
            .producers
            .live(primary)
            .is_some_and(ProducerDecision::originating_halt)
        {
            step.originating_halt = Some(primary);
        }
        self.now = at;
        if has_initial_up {
            self.pending_group_initial.insert(primary);
        }
        let replay = events
            .into_iter()
            .filter(|event| {
                !member_downs.iter().any(|down| {
                    down.stage == event.event.stage
                        && down.device == event.event.device
                        && down.source == event.event.source
                        && Arc::ptr_eq(&down.config, &event.event.config)
                        && down.payload.slot == event.event.payload.slot
                        && down.payload.target == event.event.payload.target
                })
            })
            .collect();
        Ok(MatchedGroupStep { step, replay })
    }
    pub(super) fn initialize_group_up(
        &mut self,
        primary: ActionSlot,
        at: MonoTime,
        context: &ContextSnapshot,
    ) -> Result<ComplexStep, ComplexActorError> {
        self.pending_group_initial.remove(&primary);
        let facts = self
            .groups
            .get_mut(&primary)
            .ok_or(ComplexActorError::Invariant)?
            .initial(at, &mut self.output)?;
        let mut step = ComplexStep::default();
        self.apply_group_up_facts(primary, facts, at, context, &mut step)?;
        Ok(step)
    }
    pub(super) fn admit_group_up(
        &mut self,
        primary: ActionSlot,
        due: &DueEvent<PlannedAction>,
        context: &ContextSnapshot,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
        let facts = self
            .groups
            .get_mut(&primary)
            .ok_or(ComplexActorError::Invariant)?
            .up(due, &mut self.producers, &mut self.output)?;
        let input = self
            .inputs
            .get_mut(&due.event.payload.slot.batch)
            .ok_or(ComplexActorError::Invariant)?;
        input.unreleased = input
            .unreleased
            .checked_sub(1)
            .ok_or(ComplexActorError::Invariant)?;
        self.apply_group_up_facts(primary, facts, due.delivered_at, context, step)?;
        self.now = due.delivered_at;
        Ok(())
    }
    fn apply_group_up_facts(
        &mut self,
        primary: ActionSlot,
        facts: simultaneous::ChordUpFacts,
        at: MonoTime,
        context: &ContextSnapshot,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
        if self.held.is_some_and(|(_, slot)| slot == primary)
            && self.groups.get(&primary).is_some_and(|group| {
                group.members().iter().any(|slot| {
                    self.producers
                        .live(*slot)
                        .is_some_and(|decision| decision.up_delivered_at().is_some())
                })
            })
        {
            self.held = None;
        }
        if facts.release_output {
            let trigger = self
                .groups
                .get(&primary)
                .and_then(GroupBindingOwner::last_up_member)
                .ok_or(ComplexActorError::Invariant)?;
            if self.alone_candidate == Some(primary) {
                self.alone_candidate = None;
                let decision = self
                    .producers
                    .live(primary)
                    .ok_or(ComplexActorError::Invariant)?;
                let release_time = self
                    .groups
                    .get(&primary)
                    .and_then(GroupBindingOwner::last_up_event_time)
                    .ok_or(ComplexActorError::Invariant)?;
                let duration = release_time
                    .0
                    .checked_sub(decision.down().at_ns.0)
                    .ok_or(ComplexActorError::Invariant)?;
                if decision
                    .selection()
                    .is_some_and(|rule| duration < rule.mapping().extras.alone_timeout_ns)
                {
                    self.branch_for_trigger(
                        primary,
                        rule_selection::RuleBranch::Alone,
                        context,
                        at,
                        trigger,
                    )?;
                }
            }
            self.branch_for_trigger(
                primary,
                rule_selection::RuleBranch::AfterKeyUp,
                context,
                at,
                trigger,
            )?;
        }
        if facts.after_group_up {
            let id = self.next_batch_id()?;
            let owner = self
                .groups
                .get_mut(&primary)
                .ok_or(ComplexActorError::Invariant)?;
            let admitted = if self.projection_groups.contains_key(&primary) {
                owner.dispatch_projected_group_after_up(
                    id,
                    at,
                    context,
                    self.halted.contains(&primary),
                    &mut self.producers,
                    &mut self.output,
                )?
            } else {
                owner.dispatch_group_after_up(
                    id,
                    at,
                    context,
                    self.halted.contains(&primary),
                    &mut self.producers,
                    &mut self.output,
                )?
            };
            if let Some(projection) = admitted.projection {
                self.projection_groups
                    .get_mut(&primary)
                    .ok_or(ComplexActorError::Invariant)?
                    .outputs
                    .push(projection);
            }
            if let Some(batch) = admitted.batch {
                self.children.insert(batch, primary);
                self.last_batch = batch.0;
            }
            if admitted.generated_halt {
                self.halted.insert(primary);
            }
        }
        self.finish_group(primary, step)
    }
    pub(super) fn finish_group(
        &mut self,
        primary: ActionSlot,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
        let owner = self
            .groups
            .remove(&primary)
            .ok_or(ComplexActorError::Invariant)?;
        let outer_done = !self.delayed.is_some_and(|(_, slot)| slot == primary)
            && !self.held.is_some_and(|(_, slot)| slot == primary);
        let finished = match owner.finish(&mut self.producers, outer_done) {
            Ok(finished) => finished,
            Err(rejected) => {
                let UnfinishedGroupOwner { error, owner } = *rejected;
                self.groups.insert(primary, owner);
                return if matches!(error, GroupOwnerError::Pending) {
                    Ok(())
                } else {
                    Err(error.into())
                };
            }
        };
        for (slot, _) in finished {
            self.group_members.remove(&slot);
            self.finish_input_metadata(slot, step)?;
        }
        Ok(())
    }
    pub(super) fn cancel_groups_for_device(
        &mut self,
        device: DeviceInstanceId,
        removed: bool,
        at: MonoTime,
        step: &mut ComplexStep,
    ) -> Result<(), ComplexActorError> {
        let invalidated = |saved: DeviceInstanceId| {
            saved.id == device.id
                && (saved.generation < device.generation
                    || (removed && saved.generation == device.generation))
        };
        let mut affected = Vec::new();
        for (&primary, group) in &self.groups {
            for &member in group.members() {
                let saved = self
                    .producers
                    .live(member)
                    .ok_or(ComplexActorError::Invariant)?;
                if invalidated(saved.down().device) {
                    affected.push(primary);
                    break;
                }
            }
        }
        for primary in affected {
            let children: Vec<_> = self
                .children
                .iter()
                .filter(|(_, parent)| **parent == primary)
                .map(|(&id, _)| id)
                .collect();
            for id in children {
                step.outputs.extend(self.output.cancel_batch(id, at)?);
                let canceled = self.producers.cancel_subtree(id)?;
                if canceled.len() != 1 || !canceled[0].payloads.is_empty() {
                    return Err(ComplexActorError::Invariant);
                }
                self.children.remove(&id);
            }
            if self.held.is_some_and(|(_, slot)| slot == primary) {
                self.held = None;
            }
            if self.delayed.is_some_and(|(_, slot)| slot == primary) {
                self.delayed = None;
            }
            if self.alone_candidate == Some(primary) {
                self.alone_candidate = None;
            }
            self.pending_group_initial.remove(&primary);
            let group = self
                .groups
                .remove(&primary)
                .ok_or(ComplexActorError::Invariant)?;
            let (handle, members, _) = group.into_control_ownership();
            if let Err(rejected) = self.producers.release_shared_retention(handle) {
                self.quarantined_group_holds.push(rejected.handle);
                return Err(rejected.error.into());
            }
            for member in members {
                self.group_members.remove(&member);
                self.bindings.insert(member, None);
                self.halted.remove(&member);
                self.canceled_group_members.insert(member);
                let saved = self
                    .producers
                    .live(member)
                    .ok_or(ComplexActorError::Invariant)?;
                // Invalid devices are discarded by the normal device-input control below.
                // Only surviving members with a real original Up may retire here.
                if !invalidated(saved.down().device) {
                    self.finish(member, step)?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn release_group_holds_for_reset(&mut self) -> Result<(), ComplexActorError> {
        self.pending_group_initial.clear();
        for (_, owner) in std::mem::take(&mut self.groups) {
            let (handle, _, _) = owner.into_control_ownership();
            self.quarantined_group_holds.push(handle);
        }
        self.group_members.clear();
        while let Some(handle) = self.quarantined_group_holds.pop() {
            if let Err(rejected) = self.producers.release_shared_retention(handle) {
                self.quarantined_group_holds.push(rejected.handle);
                return Err(rejected.error.into());
            }
        }
        Ok(())
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
    fn slot(id: u64) -> ActionSlot {
        ActionSlot {
            batch: BatchId(id),
            index: 0,
        }
    }
    fn snapshot(
        when: KeyUpWhen,
        timed: bool,
        empty: bool,
        extras: impl FnOnce(&mut MappingExtras),
    ) -> Arc<ConfigSnapshot> {
        let mut spec = Simultaneous::new(vec![key("a"), key("b")]);
        spec.threshold_ns = 50;
        spec.key_up_when = when;
        let mut metadata = MappingExtras {
            simultaneous: Some(spec),
            ..Default::default()
        };
        extras(&mut metadata);
        Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "actor group".into(),
            simple: vec![],
            complex: vec![Mapping {
                id: "group".into(),
                from: key("a"),
                to: if empty {
                    vec![]
                } else if timed {
                    vec![
                        Action {
                            hold_down_ns: 50,
                            ..key("c").into()
                        },
                        key("d").into(),
                    ]
                } else {
                    vec![key("c").into()]
                },
                allow_extra_modifiers: true,
                extras: metadata,
            }],
        })
    }
    fn setup(cfg: &Arc<ConfigSnapshot>) -> (ComplexStageActor, Vec<ActionPlan>) {
        // Same production actor fields and execution; strict public constructor stays gated.
        let mut actor = ComplexStageActor::from_output(OutputStageActor::new(cfg.clone()).unwrap());
        let mut plans = vec![];
        for (id, name, at) in [(1, "a", 100), (2, "b", 110)] {
            actor
                .observe_device(DeviceInstanceId { id, generation: 1 }, false, MonoTime(0))
                .unwrap();
            let plan = plan_actions(
                &[key(name).into()],
                PlanOrigin {
                    batch: BatchId(id),
                    stage: Stage::Complex,
                    start: MonoTime(at),
                    device: DeviceInstanceId { id, generation: 1 },
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
    fn matched(
        plans: &[ActionPlan],
        initial_up: bool,
    ) -> (ChordMatch, rule_selection::RuleSelection, MonoTime) {
        let mut arbiter = ChordArbiter::new(down(&plans[0])).unwrap();
        arbiter.observe(down(&plans[1])).unwrap();
        let at = if initial_up {
            MonoTime(120)
        } else {
            plans[1].origin.start
        };
        if initial_up {
            arbiter.observe(up(&plans[0], 120)).unwrap();
        }
        let ArbitrationChoice::Matched {
            group,
            selection,
            tail,
        } = arbiter
            .advance(at, None, &BTreeSet::new(), |_| Truth::True)
            .unwrap()
            .choice
        else {
            panic!("group");
        };
        assert!(tail.is_empty());
        (group, selection, at)
    }
    fn admit(actor: &mut ComplexStageActor, plans: &[ActionPlan], initial_up: bool) -> ComplexStep {
        let (group, selection, at) = matched(plans, initial_up);
        let actual = actor.admit_matched_group(group, selection, at).unwrap();
        assert!(actual.replay.is_empty());
        actual.step
    }
    fn drain(actor: &mut ComplexStageActor, at: u64) -> (Vec<(KeyId, KeyEdge)>, Vec<ActionSlot>) {
        let mut edges = vec![];
        let mut released = vec![];
        for _ in 0..1000 {
            let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
                return (edges, released);
            };
            edges.extend(
                step.outputs
                    .into_iter()
                    .map(|intent| (intent.key, intent.edge)),
            );
            released.extend(step.released_producers);
        }
        panic!("bounded actor did not drain")
    }
    #[test]
    fn actual_actor_any_all_members_share_main_repeat_up_and_final_retirement() {
        for when in [KeyUpWhen::Any, KeyUpWhen::All] {
            let cfg = snapshot(when, false, false, |_| {});
            assert!(ComplexStageActor::new(cfg.clone()).is_err());
            let (mut actor, plans) = setup(&cfg);
            admit(&mut actor, &plans, false);
            assert_eq!(drain(&mut actor, 110).0, [(key("c"), KeyEdge::Down)]);
            assert_eq!(actor.physical_repeat_target(slot(1)), Some(key("c")));
            assert_eq!(actor.physical_repeat_target(slot(2)), Some(key("c")));
            let first = actor
                .admit(&up(&plans[0], 120), &ContextSnapshot::default())
                .unwrap();
            assert!(first.released_producers.is_empty());
            let edges = drain(&mut actor, 120).0;
            if when == KeyUpWhen::Any {
                assert_eq!(edges, [(key("c"), KeyEdge::Up)]);
            } else {
                assert!(edges.is_empty());
            }
            assert_eq!(actor.pending_inputs(), 2);
            assert!(actor.physical_repeat_target(slot(1)).is_none());
            assert_eq!(
                actor.physical_repeat_target(slot(2)),
                (when == KeyUpWhen::All).then_some(key("c"))
            );
            let last = actor
                .admit(&up(&plans[1], 130), &ContextSnapshot::default())
                .unwrap();
            let (edges, released) = drain(&mut actor, 130);
            if when == KeyUpWhen::All {
                assert_eq!(edges, [(key("c"), KeyEdge::Up)]);
            } else {
                assert!(edges.is_empty());
            }
            let mut all = last.released_producers;
            all.extend(released);
            assert_eq!(all, [slot(1), slot(2)]);
            assert_eq!(actor.pending_inputs(), 0);
            assert_eq!(actor.reserved_sources(), 0);
            assert!(actor.groups.is_empty() && actor.group_members.is_empty());
        }
    }
    #[test]
    fn actor_early_all_up_retains_timed_main_until_every_actual_output_completes() {
        let cfg = snapshot(KeyUpWhen::Any, true, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        assert_eq!(drain(&mut actor, 110).0, [(key("c"), KeyEdge::Down)]);
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.pending_inputs(), 2);
        assert!(drain(&mut actor, 159).0.is_empty());
        let (edges, released) = drain(&mut actor, 160);
        assert_eq!(
            edges,
            [
                (key("c"), KeyEdge::Up),
                (key("d"), KeyEdge::Down),
                (key("d"), KeyEdge::Up)
            ]
        );
        assert_eq!(released, [slot(1), slot(2)]);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn actor_empty_main_runs_normal_after_up_at_any_all_then_group_after_up_at_all() {
        for when in [KeyUpWhen::Any, KeyUpWhen::All] {
            let cfg = snapshot(when, false, true, |extras| {
                extras.after_key_up = vec![key("e").into()];
                extras.simultaneous.as_mut().unwrap().after_key_up = vec![key("z").into()];
            });
            let (mut actor, plans) = setup(&cfg);
            admit(&mut actor, &plans, false);
            assert!(drain(&mut actor, 110).0.is_empty());
            actor
                .admit(&up(&plans[0], 120), &ContextSnapshot::default())
                .unwrap();
            let first = drain(&mut actor, 120).0;
            if when == KeyUpWhen::Any {
                assert_eq!(first, [(key("e"), KeyEdge::Down), (key("e"), KeyEdge::Up)]);
            } else {
                assert!(first.is_empty());
            }
            actor
                .admit(&up(&plans[1], 130), &ContextSnapshot::default())
                .unwrap();
            let (last, released) = drain(&mut actor, 130);
            let mut expected = vec![];
            if when == KeyUpWhen::All {
                expected.extend([(key("e"), KeyEdge::Down), (key("e"), KeyEdge::Up)]);
            }
            expected.extend([(key("z"), KeyEdge::Down), (key("z"), KeyEdge::Up)]);
            assert_eq!(last, expected);
            assert_eq!(released, [slot(1), slot(2)]);
            assert_eq!(actor.pending_inputs(), 0);
        }
    }
    #[test]
    fn actor_held_replacement_updates_shared_saved_up_and_waits_old_timed_child() {
        for timed in [false, true] {
            let cfg = snapshot(KeyUpWhen::All, timed, false, |extras| {
                extras.held_threshold_ns = 20;
                extras.if_held_down = vec![key("f").into()];
            });
            let (mut actor, plans) = setup(&cfg);
            admit(&mut actor, &plans, false);
            assert_eq!(drain(&mut actor, 110).0, [(key("c"), KeyEdge::Down)]);
            let at_held = drain(&mut actor, 130).0;
            if timed {
                assert!(at_held.is_empty());
            } else {
                assert_eq!(
                    at_held,
                    [(key("c"), KeyEdge::Up), (key("f"), KeyEdge::Down)]
                );
                assert_eq!(actor.physical_repeat_target(slot(2)), Some(key("f")));
            }
            actor
                .admit(&up(&plans[0], 140), &ContextSnapshot::default())
                .unwrap();
            actor
                .admit(&up(&plans[1], 150), &ContextSnapshot::default())
                .unwrap();
            let (last, released) = drain(&mut actor, if timed { 160 } else { 150 });
            if timed {
                assert_eq!(
                    last,
                    [
                        (key("c"), KeyEdge::Up),
                        (key("d"), KeyEdge::Down),
                        (key("d"), KeyEdge::Up),
                        (key("f"), KeyEdge::Down),
                        (key("f"), KeyEdge::Up)
                    ]
                );
            } else {
                assert_eq!(last, [(key("f"), KeyEdge::Up)]);
            }
            assert_eq!(released, [slot(1), slot(2)]);
            assert_eq!(actor.pending_inputs(), 0);
        }
    }
    #[test]
    fn actor_delayed_invoked_keeps_group_roots_after_all_original_up() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |extras| {
            extras.delayed = Some(DelayedAction {
                delay_ns: 25,
                if_invoked: vec![key("v").into()],
                if_canceled: vec![],
            });
        });
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 120);
        actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 130);
        assert_eq!(actor.pending_inputs(), 2);
        assert_eq!(actor.next_deadline(), Some(MonoTime(135)));
        let (edges, released) = drain(&mut actor, 135);
        assert_eq!(edges, [(key("v"), KeyEdge::Down), (key("v"), KeyEdge::Up)]);
        assert_eq!(released, [slot(1), slot(2)]);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn actor_reset_releases_real_group_holds_after_output_reset_without_reusing_ids() {
        let cfg = snapshot(KeyUpWhen::All, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        let reset = actor.reset(MonoTime(115)).unwrap();
        assert_eq!(
            reset
                .outputs
                .iter()
                .map(|out| (out.key, out.edge))
                .collect::<Vec<_>>(),
            [(key("c"), KeyEdge::Up)]
        );
        assert_eq!(reset.discarded_input_batches, [BatchId(1), BatchId(2)]);
        assert_eq!(actor.reserved_sources(), 0);
        assert_eq!(actor.pending_inputs(), 0);
        assert!(
            actor.groups.is_empty()
                && actor.group_members.is_empty()
                && actor.quarantined_group_holds.is_empty()
        );
        assert_eq!(actor.next_batch_id().unwrap(), BatchId(4));
    }
    #[test]
    fn actor_initial_lookahead_up_commits_real_marker_and_releases_saved_main_at_current_time() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |extras| {
            extras.simultaneous.as_mut().unwrap().key_up_order = KeyOrder::Strict;
        });
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, true);
        assert_eq!(
            actor.producer(slot(1)).unwrap().up_delivered_at(),
            Some(MonoTime(120))
        );
        assert_eq!(actor.producer(slot(1)).unwrap().down().at_ns, MonoTime(100));
        assert_eq!(
            actor.producer(slot(1)).unwrap().delivered_at(),
            MonoTime(120)
        );
        assert_eq!(
            drain(&mut actor, 120).0,
            [(key("c"), KeyEdge::Down), (key("c"), KeyEdge::Up)]
        );
        let step = actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(step.released_producers, [slot(1), slot(2)]);
        assert_eq!(actor.pending_inputs(), 0);
    }
    #[test]
    fn actor_bad_registered_peer_is_cold_and_reset_retains_batch_identity() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        let mut arbiter = ChordArbiter::new(down(&plans[0])).unwrap();
        let mut peer = down(&plans[1]);
        peer.event.source = SourceToken(99);
        arbiter.observe(peer).unwrap();
        let ArbitrationChoice::Matched {
            group, selection, ..
        } = arbiter
            .advance(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap()
            .choice
        else {
            panic!("group");
        };
        assert!(
            actor
                .admit_matched_group(group, selection, MonoTime(110))
                .is_err()
        );
        assert!(actor.is_failed());
        assert_eq!(actor.producers.live_slots(), 0);
        assert_eq!(actor.output.pending_batches(), 0);
        let reset = actor.reset(MonoTime(110)).unwrap();
        assert!(reset.outputs.is_empty());
        assert_eq!(actor.reserved_sources(), 0);
        assert_eq!(actor.next_batch_id().unwrap(), BatchId(3));
    }
    #[test]
    fn actor_returns_exact_outside_receipt_and_replay_keeps_same_key_owner_independent() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |extras| {
            extras
                .simultaneous
                .as_mut()
                .unwrap()
                .detect_key_down_uninterruptedly = true;
        });
        let (mut actor, plans) = setup(&cfg);
        let outside = plan_actions(
            &[key("c").into()],
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
        actor.register_input(outside.clone()).unwrap();
        let mut arbiter = ChordArbiter::new(down(&plans[0])).unwrap();
        arbiter.observe(down(&outside)).unwrap();
        arbiter.observe(down(&plans[1])).unwrap();
        let ArbitrationChoice::Matched {
            group,
            selection,
            tail,
        } = arbiter
            .advance(MonoTime(110), None, &BTreeSet::new(), |_| Truth::True)
            .unwrap()
            .choice
        else {
            panic!("group");
        };
        assert!(tail.is_empty());
        let chosen = actor
            .admit_matched_group(group, selection, MonoTime(110))
            .unwrap();
        assert_eq!(chosen.replay.len(), 1);
        let mut replay = chosen.replay.into_iter().next().unwrap();
        assert_eq!(replay.event.payload.slot, slot(3));
        assert_eq!(replay.event.at_ns, MonoTime(105));
        assert_eq!(replay.delivered_at, MonoTime(105));
        assert_eq!(drain(&mut actor, 110).0, [(key("c"), KeyEdge::Down)]);
        replay.delivered_at = MonoTime(111);
        actor.admit(&replay, &ContextSnapshot::default()).unwrap();
        assert!(drain(&mut actor, 111).0.is_empty());
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        assert!(drain(&mut actor, 120).0.is_empty());
        actor
            .admit(&up(&outside, 125), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 125).0, [(key("c"), KeyEdge::Up)]);
        let final_up = actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(final_up.released_producers, [slot(1), slot(2)]);
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn actor_normal_group_after_up_uses_actual_releasing_member_device_and_original_alone_span() {
        for when in [KeyUpWhen::Any, KeyUpWhen::All] {
            let cfg = snapshot(when, false, false, |extras| {
                extras.alone_timeout_ns = 15;
                extras.if_alone = vec![key("e").into()];
                extras.after_key_up = vec![Action {
                    conditions: vec![Condition::Device {
                        identifiers: vec![DeviceProperties {
                            vendor_id: Some(20),
                            ..Default::default()
                        }],
                        unless: false,
                        exists: false,
                    }],
                    ..key("f").into()
                }];
            });
            let (mut actor, plans) = setup(&cfg);
            admit(&mut actor, &plans, false);
            drain(&mut actor, 110);
            let context = ContextSnapshot {
                revision: 1,
                observed_at_ns: MonoTime(119),
                valid_until_ns: MonoTime(200),
                devices: Some(vec![
                    DeviceContext {
                        device: plans[0].origin.device,
                        properties: DeviceProperties {
                            vendor_id: Some(10),
                            ..Default::default()
                        },
                    },
                    DeviceContext {
                        device: plans[1].origin.device,
                        properties: DeviceProperties {
                            vendor_id: Some(20),
                            ..Default::default()
                        },
                    },
                ]),
                ..Default::default()
            };
            actor.admit(&up(&plans[1], 120), &context).unwrap();
            let first = drain(&mut actor, 120).0;
            if when == KeyUpWhen::Any {
                assert_eq!(
                    first,
                    [
                        (key("c"), KeyEdge::Up),
                        (key("f"), KeyEdge::Down),
                        (key("f"), KeyEdge::Up)
                    ]
                );
            } else {
                assert!(first.is_empty());
            }
            actor.admit(&up(&plans[0], 130), &context).unwrap();
            let last = drain(&mut actor, 130).0;
            if when == KeyUpWhen::All {
                assert_eq!(last, [(key("c"), KeyEdge::Up)]);
            } else {
                assert!(last.is_empty());
            }
            assert_eq!(actor.pending_inputs(), 0);
        }
    }
    #[test]
    fn actor_first_physical_up_cancels_group_held_even_when_all_main_remains_pressed() {
        let cfg = snapshot(KeyUpWhen::All, false, false, |extras| {
            extras.held_threshold_ns = 20;
            extras.if_held_down = vec![key("f").into()];
        });
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        actor
            .admit(&up(&plans[1], 120), &ContextSnapshot::default())
            .unwrap();
        assert!(actor.held.is_none());
        assert!(drain(&mut actor, 130).0.is_empty());
        assert!(actor.output.pressed_outputs().contains(&key("c")));
        actor
            .admit(&up(&plans[0], 140), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 140).0, [(key("c"), KeyEdge::Up)]);
        assert_eq!(actor.pending_inputs(), 0);
    }
    #[test]
    fn actor_reset_cancels_pending_initial_up_without_fabricating_down_up_or_branch() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |extras| {
            extras.simultaneous.as_mut().unwrap().key_up_order = KeyOrder::Strict;
            extras.after_key_up = vec![key("z").into()];
        });
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, true);
        assert!(!actor.pending_group_initial.is_empty());
        let reset = actor.reset(MonoTime(120)).unwrap();
        assert!(reset.outputs.is_empty());
        assert!(actor.pending_group_initial.is_empty());
        assert_eq!(actor.next_deadline(), None);
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn actor_late_initial_up_qualifies_alone_from_original_events_and_generates_at_current_clock() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |extras| {
            extras.simultaneous.as_mut().unwrap().key_up_order = KeyOrder::Strict;
            extras.alone_timeout_ns = 25;
            extras.if_alone = vec![key("e").into()];
        });
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, true);
        assert_eq!(
            drain(&mut actor, 200).0,
            [
                (key("c"), KeyEdge::Down),
                (key("c"), KeyEdge::Up),
                (key("e"), KeyEdge::Down),
                (key("e"), KeyEdge::Up)
            ]
        );
        assert_eq!(
            actor.producer(slot(1)).unwrap().up_delivered_at(),
            Some(MonoTime(120))
        );
        let last = actor
            .admit(&up(&plans[1], 210), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(last.released_producers, [slot(1), slot(2)]);
        assert_eq!(actor.pending_inputs(), 0);
    }
    #[test]
    fn any_member_removal_cancels_whole_group_and_survivor_up_never_fires_branches() {
        for when in [KeyUpWhen::Any, KeyUpWhen::All] {
            for removed_id in [1, 2] {
                let cfg = snapshot(when, false, false, |extras| {
                    extras.if_alone = vec![key("e").into()];
                    extras.after_key_up = vec![key("f").into()];
                    extras.if_held_down = vec![key("h").into()];
                    extras.held_threshold_ns = 20;
                    extras.delayed = Some(DelayedAction {
                        delay_ns: 30,
                        if_invoked: vec![key("v").into()],
                        if_canceled: vec![key("w").into()],
                    });
                    extras.simultaneous.as_mut().unwrap().after_key_up = vec![key("z").into()];
                });
                let (mut actor, plans) = setup(&cfg);
                admit(&mut actor, &plans, false);
                drain(&mut actor, 110);
                let control = actor
                    .observe_device(
                        DeviceInstanceId {
                            id: removed_id,
                            generation: 1,
                        },
                        true,
                        MonoTime(115),
                    )
                    .unwrap();
                assert_eq!(
                    control
                        .outputs
                        .iter()
                        .map(|out| (out.key, out.edge))
                        .collect::<Vec<_>>(),
                    [(key("c"), KeyEdge::Up)]
                );
                assert_eq!(control.discarded_input_batches, [BatchId(removed_id)]);
                assert!(control.released_producers.is_empty());
                assert!(actor.groups.is_empty() && actor.group_members.is_empty());
                assert!(
                    actor.held.is_none()
                        && actor.delayed.is_none()
                        && actor.alone_candidate.is_none()
                );
                let survivor = if removed_id == 1 { 2 } else { 1 };
                assert!(actor.canceled_group_members.contains(&slot(survivor)));
                assert!(
                    actor
                        .producer(slot(survivor))
                        .unwrap()
                        .up_delivered_at()
                        .is_none()
                );
                assert_eq!(actor.pending_inputs(), 1);
                assert_eq!(actor.next_deadline(), None);
                let release = actor
                    .admit(
                        &up(&plans[(survivor - 1) as usize], 150),
                        &ContextSnapshot::default(),
                    )
                    .unwrap();
                assert_eq!(release.released_producers, [slot(survivor)]);
                assert!(drain(&mut actor, 150).0.is_empty());
                assert!(actor.canceled_group_members.is_empty());
                assert_eq!(actor.pending_inputs(), 0);
                assert_eq!(actor.reserved_sources(), 0);
            }
        }
    }
    #[test]
    fn queued_group_control_precedes_down_and_never_manufactures_up_or_after_branch() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |extras| {
            extras.after_key_up = vec![key("z").into()];
        });
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        let control = actor
            .observe_device(plans[1].origin.device, true, MonoTime(110))
            .unwrap();
        assert!(control.outputs.is_empty());
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        assert!(drain(&mut actor, 200).0.is_empty());
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn removal_cancels_timed_main_and_queued_after_child_and_reports_seen_up_survivor() {
        let cfg = snapshot(KeyUpWhen::Any, true, false, |extras| {
            extras.after_key_up = vec![key("f").into()];
        });
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        let control = actor
            .observe_device(plans[1].origin.device, true, MonoTime(125))
            .unwrap();
        assert_eq!(
            control
                .outputs
                .iter()
                .map(|out| (out.key, out.edge))
                .collect::<Vec<_>>(),
            [(key("c"), KeyEdge::Up)]
        );
        assert_eq!(control.released_producers, [slot(1)]);
        assert_eq!(control.discarded_input_batches, [BatchId(2)]);
        assert!(drain(&mut actor, 200).0.is_empty());
        assert_eq!(actor.next_deadline(), None);
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn stale_generation_control_is_cold_for_group_and_reset_still_owns_all_holds() {
        let cfg = snapshot(KeyUpWhen::All, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        assert!(
            actor
                .observe_device(
                    DeviceInstanceId {
                        id: 2,
                        generation: 0
                    },
                    true,
                    MonoTime(115)
                )
                .is_err()
        );
        assert!(actor.is_failed());
        assert_eq!(actor.groups.len(), 1);
        assert_eq!(actor.producers.shared_retention_references(), 2);
        assert_eq!(actor.pending_inputs(), 2);
        assert!(actor.output.pressed_outputs().contains(&key("c")));
        let reset = actor.reset(MonoTime(115)).unwrap();
        assert_eq!(
            reset
                .outputs
                .iter()
                .map(|out| (out.key, out.edge))
                .collect::<Vec<_>>(),
            [(key("c"), KeyEdge::Up)]
        );
        assert_eq!(actor.producers.shared_retention_references(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn replacement_generation_cancels_group_but_stale_removal_cannot_release_new_owner() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        let new_device = DeviceInstanceId {
            id: 2,
            generation: 2,
        };
        let changed = actor
            .observe_device(new_device, false, MonoTime(115))
            .unwrap();
        assert_eq!(changed.discarded_input_batches, [BatchId(2)]);
        assert_eq!(
            changed
                .outputs
                .iter()
                .map(|out| (out.key, out.edge))
                .collect::<Vec<_>>(),
            [(key("c"), KeyEdge::Up)]
        );
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        let fresh = plan_actions(
            &[key("b").into()],
            PlanOrigin {
                batch: BatchId(4),
                stage: Stage::Complex,
                start: MonoTime(130),
                device: new_device,
                source: SourceToken(4),
                config: cfg.clone(),
            },
            false,
        )
        .unwrap();
        actor.register_input(fresh.clone()).unwrap();
        actor
            .admit(&down(&fresh), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 130).0, [(key("b"), KeyEdge::Down)]);
        assert!(
            actor
                .observe_device(plans[1].origin.device, true, MonoTime(135))
                .is_err()
        );
        assert!(actor.output.pressed_outputs().contains(&key("b")));
        let reset = actor.reset(MonoTime(135)).unwrap();
        assert_eq!(
            reset
                .outputs
                .iter()
                .map(|out| (out.key, out.edge))
                .collect::<Vec<_>>(),
            [(key("b"), KeyEdge::Up)]
        );
    }
    #[test]
    fn unrelated_device_removal_preserves_group_binding_and_output() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        let unrelated = DeviceInstanceId {
            id: 3,
            generation: 1,
        };
        actor
            .observe_device(unrelated, false, MonoTime(111))
            .unwrap();
        let removed = actor
            .observe_device(unrelated, true, MonoTime(115))
            .unwrap();
        assert!(removed.outputs.is_empty() && removed.discarded_input_batches.is_empty());
        assert_eq!(actor.groups.len(), 1);
        assert_eq!(actor.producers.shared_retention_references(), 2);
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 120).0, [(key("c"), KeyEdge::Up)]);
        actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.pending_inputs(), 0);
    }
    #[test]
    fn group_cancellation_preserves_surviving_batch_future_slot_until_its_actual_down_up() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |extras| {
            extras.after_key_up = vec![key("z").into()];
        });
        let mut actor = ComplexStageActor::from_output(OutputStageActor::new(cfg.clone()).unwrap());
        let first = plan_actions(
            &[
                Action {
                    hold_down_ns: 50,
                    ..key("a").into()
                },
                key("x").into(),
            ],
            PlanOrigin {
                batch: BatchId(1),
                stage: Stage::Complex,
                start: MonoTime(100),
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                source: SourceToken(1),
                config: cfg.clone(),
            },
            false,
        )
        .unwrap();
        let second = plan_actions(
            &[key("b").into()],
            PlanOrigin {
                batch: BatchId(2),
                stage: Stage::Complex,
                start: MonoTime(110),
                device: DeviceInstanceId {
                    id: 2,
                    generation: 1,
                },
                source: SourceToken(2),
                config: cfg.clone(),
            },
            false,
        )
        .unwrap();
        let plans = vec![first, second];
        for plan in &plans {
            actor
                .observe_device(plan.origin.device, false, MonoTime(0))
                .unwrap();
            actor.register_input(plan.clone()).unwrap();
        }
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        actor
            .observe_device(plans[1].origin.device, true, MonoTime(115))
            .unwrap();
        let a_up = plans[0]
            .events()
            .iter()
            .find(|event| event.payload.slot == slot(1) && event.payload.edge == KeyEdge::Up)
            .unwrap()
            .clone();
        actor
            .admit(
                &DueEvent {
                    event: a_up,
                    delivered_at: MonoTime(150),
                },
                &ContextSnapshot::default(),
            )
            .unwrap();
        assert_eq!(actor.pending_inputs(), 1);
        assert_eq!(actor.inputs[&BatchId(1)].remaining, 1);
        let x_down = plans[0]
            .events()
            .iter()
            .find(|event| {
                event.payload.target == ActionTarget::Key(key("x"))
                    && event.payload.edge == KeyEdge::Down
            })
            .unwrap()
            .clone();
        actor
            .admit(
                &DueEvent {
                    event: x_down,
                    delivered_at: MonoTime(150),
                },
                &ContextSnapshot::default(),
            )
            .unwrap();
        assert_eq!(drain(&mut actor, 150).0, [(key("x"), KeyEdge::Down)]);
        actor
            .admit(&up(&plans[0], 160), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 160).0, [(key("x"), KeyEdge::Up)]);
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
        assert!(actor.canceled_group_members.is_empty());
    }
    #[test]
    fn canceling_one_group_never_releases_another_groups_same_key_owner() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, first) = setup(&cfg);
        admit(&mut actor, &first, false);
        drain(&mut actor, 110);
        let mut second = vec![];
        for (id, dev, name, at) in [(4, 3, "a", 120), (5, 4, "b", 130)] {
            actor
                .observe_device(
                    DeviceInstanceId {
                        id: dev,
                        generation: 1,
                    },
                    false,
                    MonoTime(110),
                )
                .unwrap();
            let plan = plan_actions(
                &[key(name).into()],
                PlanOrigin {
                    batch: BatchId(id),
                    stage: Stage::Complex,
                    start: MonoTime(at),
                    device: DeviceInstanceId {
                        id: dev,
                        generation: 1,
                    },
                    source: SourceToken(id),
                    config: cfg.clone(),
                },
                false,
            )
            .unwrap();
            actor.register_input(plan.clone()).unwrap();
            second.push(plan);
        }
        admit(&mut actor, &second, false);
        assert!(drain(&mut actor, 130).0.is_empty());
        let control = actor
            .observe_device(first[1].origin.device, true, MonoTime(140))
            .unwrap();
        assert!(control.outputs.is_empty());
        assert_eq!(actor.groups.len(), 1);
        assert_eq!(actor.physical_repeat_target(slot(4)), Some(key("c")));
        actor
            .admit(&up(&first[0], 145), &ContextSnapshot::default())
            .unwrap();
        assert!(drain(&mut actor, 145).0.is_empty());
        actor
            .admit(&up(&second[0], 150), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 150).0, [(key("c"), KeyEdge::Up)]);
        actor
            .admit(&up(&second[1], 160), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.pending_inputs(), 0);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn secondary_member_projection_hides_and_restores_the_shared_primary_output() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        let reference = actor
            .begin_descendant_projection(slot(2), MonoTime(115))
            .unwrap();
        assert_eq!(
            actor.descendant_projection_device(reference),
            Some(plans[1].origin.device)
        );
        assert_eq!(actor.projection_handles[&reference], slot(1));
        assert_eq!(drain(&mut actor, 115).0, [(key("c"), KeyEdge::Up)]);
        actor
            .close_descendant_projection(reference, MonoTime(125))
            .unwrap();
        assert_eq!(drain(&mut actor, 125).0, [(key("c"), KeyEdge::Down)]);
        actor
            .admit(&up(&plans[0], 130), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(drain(&mut actor, 130).0, [(key("c"), KeyEdge::Up)]);
        actor
            .admit(&up(&plans[1], 140), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.reserved_sources(), 0);
        assert!(actor.projection_devices.is_empty());
    }
    #[test]
    fn both_members_share_one_projection_template_and_only_last_reference_restores() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        let first = actor
            .begin_descendant_projection(slot(1), MonoTime(115))
            .unwrap();
        assert_eq!(drain(&mut actor, 115).0, [(key("c"), KeyEdge::Up)]);
        let second = actor
            .begin_descendant_projection(slot(2), MonoTime(120))
            .unwrap();
        assert_eq!(actor.projection_groups.len(), 1);
        assert_eq!(actor.projection_groups[&slot(1)].references, 2);
        actor
            .close_descendant_projection(first, MonoTime(125))
            .unwrap();
        assert!(drain(&mut actor, 125).0.is_empty());
        actor
            .close_descendant_projection(second, MonoTime(130))
            .unwrap();
        assert_eq!(drain(&mut actor, 130).0, [(key("c"), KeyEdge::Down)]);
        actor
            .admit(&up(&plans[0], 140), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 140);
        actor
            .admit(&up(&plans[1], 150), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.pending_inputs(), 0);
    }
    #[test]
    fn pre_match_member_templates_merge_and_shared_main_is_born_projected_without_down_exposure() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        let first = actor
            .begin_descendant_projection(slot(1), MonoTime(0))
            .unwrap();
        let second = actor
            .begin_descendant_projection(slot(2), MonoTime(0))
            .unwrap();
        assert_eq!(actor.projection_groups.len(), 2);
        admit(&mut actor, &plans, false);
        assert_eq!(actor.projection_groups.len(), 1);
        assert_eq!(actor.projection_groups[&slot(1)].references, 2);
        assert_eq!(actor.projection_handles[&first], slot(1));
        assert_eq!(actor.projection_handles[&second], slot(1));
        assert_eq!(
            actor.descendant_projection_device(first),
            Some(plans[0].origin.device)
        );
        assert_eq!(
            actor.descendant_projection_device(second),
            Some(plans[1].origin.device)
        );
        assert!(drain(&mut actor, 110).0.is_empty());
        actor
            .close_descendant_projection(first, MonoTime(120))
            .unwrap();
        assert!(drain(&mut actor, 120).0.is_empty());
        actor
            .close_descendant_projection(second, MonoTime(130))
            .unwrap();
        assert_eq!(drain(&mut actor, 130).0, [(key("c"), KeyEdge::Down)]);
        actor
            .admit(&up(&plans[0], 140), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 140);
        actor
            .admit(&up(&plans[1], 150), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn born_projected_shared_variable_never_exposes_down_value_before_scope_close() {
        let mut snapshot = (*snapshot(KeyUpWhen::Any, false, false, |_| {})).clone();
        snapshot.complex[0].to = vec![Action {
            target: ActionTarget::SetVariable(VariableAction {
                name: "flag".into(),
                operation: VariableOperation::Set {
                    value: Some(VariableValue::Integer(1)),
                    key_up_value: Some(VariableValue::Integer(2)),
                },
            }),
            ..key("c").into()
        }];
        let cfg = Arc::new(snapshot);
        let (mut actor, plans) = setup(&cfg);
        let reference = actor
            .begin_descendant_projection(slot(2), MonoTime(0))
            .unwrap();
        admit(&mut actor, &plans, false);
        assert_eq!(
            actor.generated_variables()["flag"],
            VariableValue::Integer(2)
        );
        assert!(drain(&mut actor, 110).0.is_empty());
        assert_eq!(
            actor.output.executed_variables()["flag"],
            VariableValue::Integer(2)
        );
        actor
            .close_descendant_projection(reference, MonoTime(120))
            .unwrap();
        drain(&mut actor, 120);
        assert_eq!(
            actor.generated_variables()["flag"],
            VariableValue::Integer(1)
        );
        assert_eq!(
            actor.output.executed_variables()["flag"],
            VariableValue::Integer(1)
        );
        actor
            .admit(&up(&plans[0], 130), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 130);
        assert_eq!(
            actor.output.executed_variables()["flag"],
            VariableValue::Integer(2)
        );
        actor
            .admit(&up(&plans[1], 140), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn empty_shared_main_projects_normal_and_group_after_up_children_and_restores_future_work() {
        let cfg = snapshot(KeyUpWhen::Any, false, true, |extras| {
            extras.after_key_up = vec![Action {
                hold_down_ns: 30,
                ..key("f").into()
            }];
            extras.simultaneous.as_mut().unwrap().after_key_up = vec![Action {
                hold_down_ns: 50,
                ..key("z").into()
            }];
        });
        let (mut actor, plans) = setup(&cfg);
        let reference = actor
            .begin_descendant_projection(slot(2), MonoTime(0))
            .unwrap();
        admit(&mut actor, &plans, false);
        assert!(drain(&mut actor, 110).0.is_empty());
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        assert!(drain(&mut actor, 120).0.is_empty());
        actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        assert!(drain(&mut actor, 130).0.is_empty());
        assert_eq!(actor.projection_groups[&slot(1)].outputs.len(), 2);
        let restored = actor
            .restore_descendant_projection_control(reference, MonoTime(135))
            .unwrap();
        assert_eq!(
            restored
                .outputs
                .iter()
                .map(|out| (out.key, out.edge))
                .collect::<Vec<_>>(),
            [(key("f"), KeyEdge::Down)]
        );
        assert!(drain(&mut actor, 135).0.is_empty());
        assert_eq!(
            drain(&mut actor, 150).0,
            [(key("f"), KeyEdge::Up), (key("z"), KeyEdge::Down)]
        );
        let (last, released) = drain(&mut actor, 200);
        assert_eq!(last, [(key("z"), KeyEdge::Up)]);
        assert_eq!(released, [slot(1), slot(2)]);
        assert_eq!(actor.reserved_sources(), 0);
    }
    #[test]
    fn removing_either_projected_member_keeps_other_device_reference_and_never_revives_canceled_keys()
     {
        for removed_id in [1, 2] {
            let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
            let (mut actor, plans) = setup(&cfg);
            admit(&mut actor, &plans, false);
            drain(&mut actor, 110);
            let first = actor
                .begin_descendant_projection(slot(1), MonoTime(115))
                .unwrap();
            drain(&mut actor, 115);
            let second = actor
                .begin_descendant_projection(slot(2), MonoTime(116))
                .unwrap();
            let control = actor
                .observe_device(
                    DeviceInstanceId {
                        id: removed_id,
                        generation: 1,
                    },
                    true,
                    MonoTime(120),
                )
                .unwrap();
            assert!(control.outputs.is_empty());
            let (removed, surviving) = if removed_id == 1 {
                (first, second)
            } else {
                (second, first)
            };
            assert_eq!(actor.descendant_projection_device(removed), None);
            let survivor = if removed_id == 1 { 2 } else { 1 };
            assert_eq!(
                actor.descendant_projection_device(surviving),
                Some(plans[(survivor - 1) as usize].origin.device)
            );
            assert_eq!(actor.projection_groups[&slot(1)].references, 1);
            assert!(actor.projection_groups[&slot(1)].outputs.is_empty());
            actor
                .close_descendant_projection(surviving, MonoTime(125))
                .unwrap();
            assert!(drain(&mut actor, 125).0.is_empty());
            actor
                .admit(
                    &up(&plans[(survivor - 1) as usize], 130),
                    &ContextSnapshot::default(),
                )
                .unwrap();
            assert_eq!(actor.pending_inputs(), 0);
            assert!(
                actor.projection_groups.is_empty()
                    && actor.projection_handles.is_empty()
                    && actor.projection_devices.is_empty()
            );
        }
    }
    #[test]
    fn last_removed_reference_after_group_completion_releases_output_template_without_restoring_up_sources()
     {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        admit(&mut actor, &plans, false);
        drain(&mut actor, 110);
        actor
            .begin_descendant_projection(slot(2), MonoTime(115))
            .unwrap();
        drain(&mut actor, 115);
        let projection = actor.projection_groups[&slot(1)].outputs[0];
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 120);
        actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        assert_eq!(actor.pending_inputs(), 0);
        assert!(actor.output.has_key_projection(projection));
        let control = actor
            .observe_device(plans[1].origin.device, true, MonoTime(135))
            .unwrap();
        assert!(control.outputs.is_empty());
        assert!(!actor.output.has_key_projection(projection));
        assert!(
            actor.projection_groups.is_empty()
                && actor.projection_handles.is_empty()
                && actor.projection_devices.is_empty()
        );
    }
    #[test]
    fn projected_group_reset_clears_alias_devices_and_restores_no_never_emitted_keys() {
        let cfg = snapshot(KeyUpWhen::Any, false, false, |_| {});
        let (mut actor, plans) = setup(&cfg);
        actor
            .begin_descendant_projection(slot(2), MonoTime(0))
            .unwrap();
        admit(&mut actor, &plans, false);
        assert!(drain(&mut actor, 110).0.is_empty());
        let reset = actor.reset(MonoTime(115)).unwrap();
        assert!(reset.outputs.is_empty());
        assert!(
            actor.projection_groups.is_empty()
                && actor.projection_handles.is_empty()
                && actor.projection_devices.is_empty()
        );
        assert_eq!(actor.reserved_sources(), 0);
    }
}
