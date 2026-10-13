#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, simple_actor::*, simultaneous::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn cfg(extras: impl FnOnce(&mut MappingExtras)) -> Arc<ConfigSnapshot> {
    let mut spec = Simultaneous::new(vec![key("a"), key("b")]);
    spec.threshold_ns = 50;
    let mut metadata = MappingExtras {
        simultaneous: Some(spec),
        ..Default::default()
    };
    extras(&mut metadata);
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "simple chord model".into(),
        simple: vec![],
        complex: vec![Mapping {
            id: "group".into(),
            from: key("a"),
            to: vec![key("c").into()],
            allow_extra_modifiers: true,
            extras: metadata,
        }],
    })
}
fn actor(cfg: &Arc<ConfigSnapshot>, names: &[(&str, u64)]) -> (SimpleStageActor, Vec<ActionPlan>) {
    let mut actor = SimpleStageActor::new_with_chord_model(cfg.clone()).unwrap();
    let mut plans = vec![];
    for (index, (name, at)) in names.iter().enumerate() {
        let id = index as u64 + 1;
        let device = DeviceInstanceId { id, generation: 1 };
        actor.observe_device(device, false, MonoTime(0)).unwrap();
        let plan = plan_actions(
            &[key(name).into()],
            PlanOrigin {
                batch: BatchId(id),
                stage: Stage::Simple,
                start: MonoTime(*at),
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
        delivered_at: plan.events()[0].at_ns,
    }
}
fn up(plan: &ActionPlan, at: u64) -> DueEvent<PlannedAction> {
    DueEvent {
        event: StageEvent {
            stage: Stage::Simple,
            at_ns: MonoTime(at),
            device: plan.events()[0].device,
            source: plan.events()[0].source,
            config: plan.events()[0].config.clone(),
            payload: plan.releases()[0].clone(),
        },
        delivered_at: MonoTime(at),
    }
}
fn drain(actor: &mut SimpleStageActor, at: u64) -> (Vec<(KeyId, KeyEdge)>, Vec<ActionSlot>) {
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
    panic!("bounded model did not stabilize")
}
#[test]
fn public_model_executes_complete_simple_complex_group_and_retires_both_original_roots() {
    let cfg = cfg(|_| {});
    assert!(SimpleStageActor::new(cfg.clone()).is_err());
    let (mut actor, plans) = actor(&cfg, &[("a", 100), ("b", 110)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    actor
        .admit(&down(&plans[1]), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 110).0, [(key("c"), KeyEdge::Down)]);
    actor
        .admit(&up(&plans[0], 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 120).0, [(key("c"), KeyEdge::Up)]);
    assert_eq!(actor.pending_inputs(), 2);
    actor
        .admit(&up(&plans[1], 130), &ContextSnapshot::default())
        .unwrap();
    let (_, released) = drain(&mut actor, 130);
    assert_eq!(released.len(), 2);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn late_poll_delivers_known_boundary_input_before_timeout_with_original_stage_history() {
    let cfg = cfg(|_| {});
    let (mut actor, plans) = actor(&cfg, &[("a", 100), ("b", 150)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    actor
        .admit(&down(&plans[1]), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 200).0, [(key("c"), KeyEdge::Down)]);
    actor
        .admit(&up(&plans[0], 210), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 210);
    actor
        .admit(&up(&plans[1], 220), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 220);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn timeout_replays_passthrough_in_same_pipeline_without_deadlock_or_new_owner_on_up() {
    let cfg = cfg(|_| {});
    let (mut actor, plans) = actor(&cfg, &[("a", 100)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    assert_eq!(actor.next_deadline(), Some(MonoTime(151)));
    assert_eq!(drain(&mut actor, 151).0, [(key("a"), KeyEdge::Down)]);
    actor
        .admit(&up(&plans[0], 160), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 160).0, [(key("a"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn actual_surviving_completion_on_device_control_retires_simple_parent_after_original_up() {
    let mut snapshot = (*cfg(|extras| extras.after_key_up = vec![key("f").into()])).clone();
    snapshot.complex[0].to[0].hold_down_ns = 50;
    snapshot.complex[0].to.push(key("d").into());
    let cfg = Arc::new(snapshot);
    let (mut actor, plans) = actor(&cfg, &[("a", 100), ("b", 110)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    actor
        .admit(&down(&plans[1]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 110);
    actor
        .admit(&up(&plans[0], 120), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 120);
    let control = actor
        .observe_device(plans[1].events()[0].device, true, MonoTime(125))
        .unwrap();
    assert_eq!(
        control
            .outputs
            .iter()
            .map(|intent| (intent.key, intent.edge))
            .collect::<Vec<_>>(),
        [(key("c"), KeyEdge::Up)]
    );
    assert_eq!(
        control.released_producers,
        [plans[0].events()[0].payload.slot]
    );
    assert_eq!(control.discarded_input_batches, [BatchId(2)]);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn model_reload_preserves_old_group_snapshot_and_old_up_after_new_configuration() {
    let cfg =
        cfg(|extras| extras.simultaneous.as_mut().unwrap().after_key_up = vec![key("z").into()]);
    let (mut actor, plans) = actor(&cfg, &[("a", 100), ("b", 110)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    actor
        .admit(&down(&plans[1]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 110);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor
        .admit(&up(&plans[0], 120), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 120).0, [(key("c"), KeyEdge::Up)]);
    actor
        .admit(&up(&plans[1], 130), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(
        drain(&mut actor, 130).0,
        [(key("z"), KeyEdge::Down), (key("z"), KeyEdge::Up)]
    );
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn waiting_wheel_interrupt_history_cancels_group_alone_but_zero_wheel_does_not() {
    for wheel in [0, 1] {
        let cfg = cfg(|extras| {
            extras.if_alone = vec![key("e").into()];
            extras.alone_timeout_ns = 100;
        });
        let (mut actor, plans) = actor(&cfg, &[("a", 100), ("b", 110)]);
        actor
            .admit(&down(&plans[0]), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 100);
        actor.observe_wheel(0, wheel, MonoTime(105)).unwrap();
        actor
            .admit(&down(&plans[1]), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 110);
        actor
            .admit(&up(&plans[0], 120), &ContextSnapshot::default())
            .unwrap();
        let mut expected = vec![(key("c"), KeyEdge::Up)];
        if wheel == 0 {
            expected.extend([(key("e"), KeyEdge::Down), (key("e"), KeyEdge::Up)]);
        }
        assert_eq!(drain(&mut actor, 120).0, expected);
        actor
            .admit(&up(&plans[1], 130), &ContextSnapshot::default())
            .unwrap();
        drain(&mut actor, 130);
        assert_eq!(actor.reserved_sources(), 0);
    }
}
#[test]
fn reset_waiting_model_cancels_both_actor_input_and_upstream_generation_without_fake_up() {
    let cfg = cfg(|_| {});
    let (mut actor, plans) = actor(&cfg, &[("a", 100)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    let reset = actor.reset(MonoTime(110)).unwrap();
    assert!(reset.outputs.is_empty());
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
    assert_eq!(actor.next_deadline(), None);
}
#[test]
fn model_accepts_complex_metadata_but_rejects_simple_simultaneous_and_invalid_options() {
    let mut snapshot = (*cfg(|_| {})).clone();
    snapshot.simple = snapshot.complex.clone();
    let cfg = Arc::new(snapshot);
    assert!(SimpleStageActor::new_with_chord_model(cfg.clone()).is_err());
    assert!(keyweave_engine::complex_actor::ComplexStageActor::new_with_chord_model(cfg).is_err());
    let mut snapshot = (*self::cfg(|_| {})).clone();
    snapshot.complex[0]
        .extras
        .simultaneous
        .as_mut()
        .unwrap()
        .keys
        .clear();
    assert!(SimpleStageActor::new_with_chord_model(Arc::new(snapshot)).is_err());
}

#[test]
fn actual_simple_consumed_scope_captures_reserved_member_and_restores_shared_output_after_body() {
    let mut snapshot = (*cfg(|extras| {
        let group = extras.simultaneous.as_mut().unwrap();
        group.keys = vec![key("left_shift"), key("b")];
        group.detect_key_down_uninterruptedly = true;
        group.threshold_ns = 100;
    }))
    .clone();
    snapshot.complex[0].from = key("left_shift");
    let mut metadata = MappingExtras::default();
    metadata.from_modifiers.mandatory.insert(Modifier::Shift);
    snapshot.simple.push(Mapping {
        id: "body".into(),
        from: key("q"),
        to: vec![Action {
            hold_down_ns: 50,
            repeat: false,
            ..key("f").into()
        }],
        allow_extra_modifiers: true,
        extras: metadata,
    });
    let cfg = Arc::new(snapshot);
    let (mut actor, plans) = actor(&cfg, &[("left_shift", 100), ("b", 110), ("q", 105)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 100).0.is_empty());
    actor
        .admit(&down(&plans[2]), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 105).0.is_empty());
    actor
        .admit(&down(&plans[1]), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 110).0.is_empty());
    // The existing Simple FIFO normalizes b behind the 50 ns tap and its Scope close.
    assert_eq!(
        drain(&mut actor, 155).0,
        [
            (key("f"), KeyEdge::Down),
            (key("f"), KeyEdge::Up),
            (key("c"), KeyEdge::Down)
        ]
    );
    actor
        .admit(&up(&plans[0], 160), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 160).0, [(key("c"), KeyEdge::Up)]);
    actor
        .admit(&up(&plans[1], 170), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 170);
    actor
        .admit(&up(&plans[2], 180), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 180);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}

#[test]
fn scope_close_uses_actual_body_up_not_group_retirement_and_restores_all_mode_before_last_member_up()
 {
    let mut snapshot = (*cfg(|extras| {
        let group = extras.simultaneous.as_mut().unwrap();
        group.keys = vec![key("left_shift"), key("b")];
        group.key_up_when = KeyUpWhen::All;
    }))
    .clone();
    snapshot.complex[0].from = key("left_shift");
    let mut metadata = MappingExtras::default();
    metadata.from_modifiers.mandatory.insert(Modifier::Shift);
    snapshot.simple.push(Mapping {
        id: "body-member".into(),
        from: key("q"),
        to: vec![Action {
            hold_down_ns: 50,
            repeat: false,
            ..key("b").into()
        }],
        allow_extra_modifiers: true,
        extras: metadata,
    });
    let cfg = Arc::new(snapshot);
    let (mut actor, plans) = actor(&cfg, &[("left_shift", 100), ("q", 105)]);
    actor
        .admit(&down(&plans[0]), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 100);
    actor
        .admit(&down(&plans[1]), &ContextSnapshot::default())
        .unwrap();
    assert!(drain(&mut actor, 105).0.is_empty());
    assert_eq!(drain(&mut actor, 155).0, [(key("c"), KeyEdge::Down)]);
    actor
        .admit(&up(&plans[0], 160), &ContextSnapshot::default())
        .unwrap();
    assert_eq!(drain(&mut actor, 160).0, [(key("c"), KeyEdge::Up)]);
    actor
        .admit(&up(&plans[1], 170), &ContextSnapshot::default())
        .unwrap();
    drain(&mut actor, 170);
    assert_eq!(actor.pending_inputs(), 0);
    assert_eq!(actor.pending_children(), 0);
    assert_eq!(actor.reserved_sources(), 0);
}
