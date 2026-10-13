use super::*;
use crate::{
    output_stage::{OutputStageActor, OutputStageError},
    producer_decision::{ProducerDecision, ProducerUpError},
    stage_bindings::{BindingArena, BindingError, SharedRetention},
    terminal_actor::ActorStep,
};
#[derive(Debug, thiserror::Error)]
pub enum GroupOwnerError {
    #[error(transparent)]
    Chord(#[from] ChordError),
    #[error(transparent)]
    Binding(#[from] BindingError),
    #[error(transparent)]
    Up(#[from] ProducerUpError),
    #[error(transparent)]
    Output(#[from] OutputStageError),
    #[error("group sources/output do not match prepared ownership")]
    Ownership,
    #[error("group still has member/output/branch work")]
    Pending,
    #[error("group owner failed; outer actor must reset/control cleanup")]
    Failed,
}
pub struct RejectedGroupOwner {
    pub error: GroupOwnerError,
    pub group: ChordMatch,
    pub handle: SharedRetention,
}
impl std::fmt::Debug for RejectedGroupOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RejectedGroupOwner")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
pub struct UnfinishedGroupOwner {
    pub error: GroupOwnerError,
    pub owner: GroupBindingOwner,
}
impl std::fmt::Debug for UnfinishedGroupOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnfinishedGroupOwner")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
/// Owns group Up/completion facts and retention. The outer actor still owns branch scheduling.
#[must_use = "retain owner until group completion or explicit outer control cleanup"]
pub struct GroupBindingOwner {
    active: ActiveChord,
    handle: Option<SharedRetention>,
    members: Vec<crate::action_plan::ActionSlot>,
    main: Option<crate::action_plan::BatchId>,
    output_done: bool,
    all_up: bool,
    failed: bool,
}
impl GroupBindingOwner {
    pub fn new(
        group: ChordMatch,
        handle: SharedRetention,
        main: Option<crate::action_plan::BatchId>,
        arena: &BindingArena<ProducerDecision>,
        output: &OutputStageActor,
    ) -> Result<(Self, Vec<DueEvent<PlannedAction>>), Box<RejectedGroupOwner>> {
        let members: Vec<_> = group
            .members()
            .iter()
            .map(|member| member.down().event.payload.slot)
            .collect();
        let checked = (|| {
            if arena.retained_members(&handle)? != members.as_slice() {
                return Err(GroupOwnerError::Ownership);
            }
            let first = group.members().first().ok_or(GroupOwnerError::Ownership)?;
            let selection = arena
                .live(first.down().event.payload.slot)
                .and_then(ProducerDecision::selection)
                .ok_or(GroupOwnerError::Ownership)?;
            if selection.stage() != crate::rule_selection::MappingStage::Complex
                || !std::sync::Arc::ptr_eq(selection.snapshot(), &first.down().event.config)
                || selection.mapping().extras.simultaneous.as_ref() != Some(group.spec())
            {
                return Err(GroupOwnerError::Ownership);
            }
            for member in group.members() {
                let decision = arena
                    .live(member.down().event.payload.slot)
                    .ok_or(GroupOwnerError::Ownership)?;
                if !provenance(decision.down(), &member.down().event) {
                    return Err(GroupOwnerError::Ownership);
                }
                if member.up().is_some() != decision.up_delivered_at().is_some() {
                    return Err(GroupOwnerError::Ownership);
                }
            }
            if let Some(main) = main
                && output.parent_binding(main) != members.first().copied()
            {
                return Err(GroupOwnerError::Ownership);
            }
            Ok(())
        })();
        if let Err(error) = checked {
            return Err(Box::new(RejectedGroupOwner {
                error,
                group,
                handle,
            }));
        }
        let (active, events) = group.into_active();
        Ok((
            Self {
                active,
                handle: Some(handle),
                members,
                main,
                output_done: main.is_none(),
                all_up: false,
                failed: false,
            },
            events,
        ))
    }
    /// Process initial lookahead facts only after corresponding actual producer Ups were admitted.
    pub fn initial(
        &mut self,
        at: MonoTime,
        output: &mut OutputStageActor,
    ) -> Result<ChordUpFacts, GroupOwnerError> {
        if self.failed {
            return Err(GroupOwnerError::Failed);
        }
        let facts = self.active.initial_facts();
        if let Err(error) = self.apply_facts(facts, at, output) {
            self.failed = true;
            return Err(error);
        }
        Ok(facts)
    }
    pub fn up(
        &mut self,
        receipt: &DueEvent<PlannedAction>,
        arena: &mut BindingArena<ProducerDecision>,
        output: &mut OutputStageActor,
    ) -> Result<ChordUpFacts, GroupOwnerError> {
        if self.failed {
            return Err(GroupOwnerError::Failed);
        }
        if !self.active.is_member(receipt) {
            return Err(ChordError::Outside.into());
        }
        // Original Up guard is cold; only commit after group and saved output Up admission.
        let marker = arena.prepare_producer_up(receipt)?;
        let facts = self.active.up(receipt)?;
        if let Err(error) = self.apply_facts(facts, receipt.delivered_at, output) {
            self.failed = true;
            return Err(error);
        }
        marker.commit();
        Ok(facts)
    }
    fn apply_facts(
        &mut self,
        facts: ChordUpFacts,
        at: MonoTime,
        output: &mut OutputStageActor,
    ) -> Result<(), GroupOwnerError> {
        if facts.release_output
            && let Some(main) = self.main
        {
            output.input_up(main, at)?;
        }
        self.all_up = facts.finished;
        Ok(())
    }
    pub fn observe_output_step(
        &mut self,
        step: &ActorStep,
        output: &OutputStageActor,
    ) -> Result<bool, GroupOwnerError> {
        if self.failed {
            return Err(GroupOwnerError::Failed);
        }
        if self.output_done || step.completed_batch != self.main || self.main.is_none() {
            return Ok(false);
        }
        let id = self.main.ok_or(GroupOwnerError::Ownership)?;
        if output.parent_binding(id).is_some() {
            return Err(GroupOwnerError::Ownership);
        }
        self.output_done = true;
        Ok(true)
    }
    pub fn main_done(&self) -> bool {
        self.output_done
    }
    pub fn members_up(&self) -> bool {
        self.all_up
    }
    /// Transfer metadata ownership to outer Reset/device control, including after failure.
    /// This performs no output release, producer finish, retention release or native recovery.
    pub fn into_control_ownership(
        self,
    ) -> (
        SharedRetention,
        Vec<crate::action_plan::ActionSlot>,
        Option<crate::action_plan::BatchId>,
    ) {
        (
            self.handle
                .unwrap_or_else(|| unreachable!("live group always owns retention")),
            self.members,
            self.main,
        )
    }
    /// `outer_work_done` covers future branches/scopes not represented by this main owner.
    /// Return actual payloads for outer input-table accounting; no batch retirement or native proof.
    pub fn finish(
        mut self,
        arena: &mut BindingArena<ProducerDecision>,
        outer_work_done: bool,
    ) -> Result<Vec<(crate::action_plan::ActionSlot, ProducerDecision)>, Box<UnfinishedGroupOwner>>
    {
        let checked = (|| {
            if self.failed {
                return Err(GroupOwnerError::Failed);
            }
            if !self.all_up || !self.output_done || !outer_work_done {
                return Err(GroupOwnerError::Pending);
            }
            let handle = self.handle.as_ref().ok_or(GroupOwnerError::Ownership)?;
            if arena.retained_members(handle)? != self.members.as_slice() {
                return Err(GroupOwnerError::Ownership);
            }
            for &slot in &self.members {
                if arena
                    .live(slot)
                    .and_then(ProducerDecision::up_delivered_at)
                    .is_none()
                    || arena.has_child_work(slot)?
                    || arena.blocking_retentions(slot) != [handle.key()]
                {
                    return Err(GroupOwnerError::Pending);
                }
            }
            Ok(())
        })();
        if let Err(error) = checked {
            return Err(Box::new(UnfinishedGroupOwner { error, owner: self }));
        }
        let handle = self
            .handle
            .take()
            .unwrap_or_else(|| unreachable!("preflighted owned handle"));
        if let Err(rejected) = arena.release_shared_retention(handle) {
            self.handle = Some(rejected.handle);
            return Err(Box::new(UnfinishedGroupOwner {
                error: rejected.error.into(),
                owner: self,
            }));
        }
        let finished = self
            .members
            .iter()
            .map(|&slot| {
                (
                    slot,
                    arena.finish_producer(slot).unwrap_or_else(|_| {
                        unreachable!("preflighted members without children/retentions")
                    }),
                )
            })
            .collect();
        Ok(finished)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::{action_plan::*, modifier_scope::ScopedActionPlan};
    use keyweave_types::*;
    use std::{collections::BTreeSet, sync::Arc};
    fn key(name: &str) -> KeyId {
        name.parse().unwrap()
    }
    fn origin(cfg: &Arc<crate::ConfigSnapshot>, id: u64, stage: Stage, at: u64) -> PlanOrigin {
        PlanOrigin {
            batch: BatchId(id),
            stage,
            start: MonoTime(at),
            device: DeviceInstanceId {
                id: if id == 10 { 1 } else { id },
                generation: 1,
            },
            source: SourceToken(if id == 10 { 1 } else { id }),
            config: cfg.clone(),
        }
    }
    fn raw(
        cfg: &Arc<crate::ConfigSnapshot>,
        id: u64,
        name: &str,
        edge: KeyEdge,
        at: u64,
    ) -> DueEvent<PlannedAction> {
        DueEvent {
            event: crate::timeline::StageEvent {
                stage: Stage::Complex,
                at_ns: MonoTime(at),
                device: DeviceInstanceId { id, generation: 1 },
                source: SourceToken(id),
                config: cfg.clone(),
                payload: PlannedAction {
                    slot: ActionSlot {
                        batch: BatchId(id),
                        index: 0,
                    },
                    target: crate::ActionTarget::Key(key(name)),
                    edge,
                    lazy: false,
                    halt: false,
                },
            },
            delivered_at: MonoTime(at),
        }
    }
    fn setup(
        when: KeyUpWhen,
        timed: bool,
    ) -> (
        GroupBindingOwner,
        BindingArena<ProducerDecision>,
        OutputStageActor,
        Arc<crate::ConfigSnapshot>,
    ) {
        let mut spec = Simultaneous::new(vec![key("a"), key("b")]);
        spec.threshold_ns = 50;
        spec.key_up_when = when;
        let cfg = Arc::new(crate::ConfigSnapshot {
            generation: ConfigGeneration(1),
            name: "owned group".into(),
            simple: vec![],
            complex: vec![crate::Mapping {
                id: "group".into(),
                from: key("a"),
                to: if timed {
                    vec![
                        crate::Action {
                            hold_down_ns: 50,
                            ..key("c").into()
                        },
                        key("d").into(),
                    ]
                } else {
                    vec![key("c").into()]
                },
                allow_extra_modifiers: true,
                extras: crate::MappingExtras {
                    simultaneous: Some(spec),
                    ..Default::default()
                },
            }],
        });
        let mut arbiter = ChordArbiter::new(raw(&cfg, 1, "a", KeyEdge::Down, 100)).unwrap();
        arbiter
            .observe(raw(&cfg, 2, "b", KeyEdge::Down, 110))
            .unwrap();
        let ArbitrationChoice::Matched {
            selection,
            group,
            tail: _,
        } = arbiter
            .advance(MonoTime(110), None, &BTreeSet::new(), |_| {
                crate::Truth::True
            })
            .unwrap()
            .choice
        else {
            panic!("group")
        };
        let plan = ScopedActionPlan::main(
            &selection,
            BatchId(10),
            MonoTime(110),
            DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            SourceToken(1),
        )
        .unwrap();
        let slots = plan
            .actions()
            .events()
            .iter()
            .filter(|event| event.payload.edge == KeyEdge::Down)
            .count();
        let mut arena = BindingArena::default();
        let mut inputs = vec![];
        let mut selection = Some(selection);
        for (id, name) in [(1, "a"), (2, "b")] {
            arena
                .prepare(origin(&cfg, id, Stage::Complex, 100), 1, None)
                .unwrap()
                .commit();
            let down = raw(
                &cfg,
                id,
                name,
                KeyEdge::Down,
                if id == 1 { 100 } else { 110 },
            );
            let up = raw(&cfg, id, name, KeyEdge::Up, 120);
            let decision = ProducerDecision::new(
                down.event.clone(),
                MonoTime(110),
                Stage::Output,
                if id == 1 { selection.take() } else { None },
            )
            .with_original_up(&up.event.payload)
            .unwrap();
            inputs.push((down, decision));
        }
        let mut output = OutputStageActor::new(cfg.clone()).unwrap();
        for id in [1, 2] {
            output
                .observe_device(DeviceInstanceId { id, generation: 1 }, false, MonoTime(0))
                .unwrap();
        }
        let registration = arena
            .prepare_shared_child_with_down(origin(&cfg, 10, Stage::Output, 110), slots, inputs)
            .unwrap();
        let handle = output
            .prepare_shared_child_scoped(plan, registration)
            .unwrap()
            .commit();
        let (mut owner, events) =
            GroupBindingOwner::new(group, handle, Some(BatchId(10)), &arena, &output).unwrap();
        assert_eq!(events.len(), 2);
        assert!(
            !owner
                .initial(MonoTime(110), &mut output)
                .unwrap()
                .release_output
        );
        (owner, arena, output, cfg)
    }
    fn drain(
        owner: &mut GroupBindingOwner,
        output: &mut OutputStageActor,
        at: u64,
    ) -> Vec<(KeyId, KeyEdge)> {
        let mut result = vec![];
        for _ in 0..10000 {
            let Some(step) = output.poll(MonoTime(at)).unwrap() else {
                return result;
            };
            owner.observe_output_step(&step, output).unwrap();
            result.extend(step.outputs.into_iter().map(|out| (out.key, out.edge)));
        }
        panic!("bounded")
    }
    #[test]
    fn owned_any_all_updates_actual_original_up_and_saved_output_once() {
        for when in [KeyUpWhen::Any, KeyUpWhen::All] {
            let (mut owner, mut arena, mut output, cfg) = setup(when, false);
            assert_eq!(
                drain(&mut owner, &mut output, 110),
                [(key("c"), KeyEdge::Down)]
            );
            let facts = owner
                .up(
                    &raw(&cfg, 1, "a", KeyEdge::Up, 120),
                    &mut arena,
                    &mut output,
                )
                .unwrap();
            assert_eq!(facts.release_output, when == KeyUpWhen::Any);
            assert_eq!(
                arena
                    .live(ActionSlot {
                        batch: BatchId(1),
                        index: 0
                    })
                    .unwrap()
                    .up_delivered_at(),
                Some(MonoTime(120))
            );
            let first = drain(&mut owner, &mut output, 120);
            if when == KeyUpWhen::Any {
                assert_eq!(first, [(key("c"), KeyEdge::Up)]);
            } else {
                assert!(first.is_empty());
            }
            let rejected = owner.finish(&mut arena, true).err().unwrap();
            assert!(matches!(rejected.error, GroupOwnerError::Pending));
            owner = rejected.owner;
            let last = owner
                .up(
                    &raw(&cfg, 2, "b", KeyEdge::Up, 130),
                    &mut arena,
                    &mut output,
                )
                .unwrap();
            assert!(last.finished);
            drain(&mut owner, &mut output, 130);
            assert!(owner.main_done() && owner.members_up());
            arena.cancel(BatchId(10)).unwrap();
            let finished = owner.finish(&mut arena, true).unwrap();
            assert_eq!(finished.len(), 2);
            assert_eq!(arena.live_slots(), 0);
            assert_eq!(arena.shared_retention_references(), 0);
        }
    }
    #[test]
    fn all_physical_up_does_not_finish_timed_output_or_outer_branch_work() {
        let (mut owner, mut arena, mut output, cfg) = setup(KeyUpWhen::Any, true);
        drain(&mut owner, &mut output, 110);
        owner
            .up(
                &raw(&cfg, 1, "a", KeyEdge::Up, 120),
                &mut arena,
                &mut output,
            )
            .unwrap();
        owner
            .up(
                &raw(&cfg, 2, "b", KeyEdge::Up, 130),
                &mut arena,
                &mut output,
            )
            .unwrap();
        let rejected = owner.finish(&mut arena, true).err().unwrap();
        owner = rejected.owner;
        assert_eq!(arena.shared_retention_references(), 2);
        assert_eq!(
            drain(&mut owner, &mut output, 160),
            [
                (key("c"), KeyEdge::Up),
                (key("d"), KeyEdge::Down),
                (key("d"), KeyEdge::Up)
            ]
        );
        arena.cancel(BatchId(10)).unwrap();
        let rejected = owner.finish(&mut arena, false).err().unwrap();
        assert!(matches!(rejected.error, GroupOwnerError::Pending));
        assert_eq!(rejected.owner.finish(&mut arena, true).unwrap().len(), 2);
    }
    #[test]
    fn bad_original_up_is_cold_and_cannot_mark_producer_or_release_output() {
        let (mut owner, mut arena, mut output, cfg) = setup(KeyUpWhen::Any, false);
        drain(&mut owner, &mut output, 110);
        let mut up = raw(&cfg, 1, "a", KeyEdge::Up, 120);
        up.event.payload.halt = true;
        assert!(owner.up(&up, &mut arena, &mut output).is_err());
        assert!(
            arena
                .live(ActionSlot {
                    batch: BatchId(1),
                    index: 0
                })
                .unwrap()
                .up_delivered_at()
                .is_none()
        );
        assert!(drain(&mut owner, &mut output, 120).is_empty());
        assert!(
            owner
                .up(
                    &raw(&cfg, 1, "a", KeyEdge::Up, 121),
                    &mut arena,
                    &mut output
                )
                .unwrap()
                .release_output
        );
    }
    #[test]
    fn forged_completion_before_actual_recipe_finishes_is_refused() {
        let (mut owner, _arena, output, _cfg) = setup(KeyUpWhen::Any, false);
        let step = ActorStep {
            outputs: vec![],
            halt_requested: false,
            completed_batch: Some(BatchId(10)),
        };
        assert!(matches!(
            owner.observe_output_step(&step, &output),
            Err(GroupOwnerError::Ownership)
        ));
        assert!(!owner.main_done());
    }
    #[test]
    fn failed_owner_transfers_control_metadata_without_releasing_actual_work() {
        let (mut owner, mut arena, output, _cfg) = setup(KeyUpWhen::Any, false);
        owner.failed = true;
        let rejected = owner.finish(&mut arena, true).err().unwrap();
        assert!(matches!(rejected.error, GroupOwnerError::Failed));
        let (handle, members, main) = rejected.owner.into_control_ownership();
        assert_eq!(arena.retained_members(&handle).unwrap(), members);
        assert_eq!(arena.shared_retention_references(), 2);
        assert_eq!(arena.live_slots(), 2);
        assert_eq!(main, Some(BatchId(10)));
        assert_eq!(output.parent_binding(BatchId(10)), Some(members[0]));
        assert!(arena.cancel(BatchId(1)).is_err());
        // Outer control explicitly releases only this metadata hold; output remains owned.
        arena.release_shared_retention(handle).unwrap();
        assert_eq!(arena.live_slots(), 2);
        assert_eq!(output.parent_binding(BatchId(10)), Some(members[0]));
    }
}
