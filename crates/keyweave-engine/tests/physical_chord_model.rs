#![allow(clippy::unwrap_used)]
use keyweave_engine::{physical_controller::*, simultaneous::*, *};
use keyweave_types::*;
use std::sync::Arc;
fn key(name: &str) -> KeyId {
    name.parse().unwrap()
}
fn rule(from: &str, to: Vec<Action>) -> Mapping {
    Mapping {
        id: from.into(),
        from: key(from),
        to,
        allow_extra_modifiers: true,
        extras: Default::default(),
    }
}
fn cfg(extras: impl FnOnce(&mut MappingExtras)) -> Arc<ConfigSnapshot> {
    let mut mapping = rule("a", vec![key("c").into()]);
    let mut spec = Simultaneous::new(vec![key("a"), key("b")]);
    spec.threshold_ns = 50;
    mapping.extras.simultaneous = Some(spec);
    extras(&mut mapping.extras);
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "physical chord".into(),
        simple: vec![],
        complex: vec![mapping],
    })
}
fn device(id: u64) -> DeviceInstanceId {
    DeviceInstanceId { id, generation: 1 }
}
fn input(seq: u64, at: u64, dev: u64, name: &str, edge: KeyEdge) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(at),
        device: device(dev),
        provenance: Provenance::Physical,
        event: InputEvent::Key {
            key: key(name),
            edge,
        },
    }
}
fn edges(outputs: Vec<OutputIntent>) -> Vec<(KeyId, KeyEdge)> {
    outputs.into_iter().map(|out| (out.key, out.edge)).collect()
}
fn drain(actor: &mut PhysicalStageController, at: u64) -> Vec<(KeyId, KeyEdge)> {
    let mut result = vec![];
    for _ in 0..1000 {
        let Some(step) = actor.poll(MonoTime(at)).unwrap() else {
            return result;
        };
        result.extend(edges(step.outputs));
    }
    panic!("bounded controller did not stabilize")
}
#[test]
fn every_member_bound_includes_all_branches_and_group_after_up_modifiers() {
    let cfg = cfg(|extras| {
        extras.if_held_down = vec![key("h").into()];
        extras.if_alone = vec![key("x").into()];
        extras.after_key_up = vec![key("y").into()];
        extras.delayed = Some(DelayedAction {
            delay_ns: 10,
            if_invoked: vec![key("i").into()],
            if_canceled: vec![key("j").into()],
        });
        extras.simultaneous.as_mut().unwrap().after_key_up = vec![Action {
            modifiers: vec![key("left_shift")],
            ..key("z").into()
        }];
    });
    let bounds = cfg.timed_stage_source_bounds();
    assert_eq!(bounds[&key("a")], 34);
    assert_eq!(bounds[&key("b")], 34);
}
#[test]
fn simple_targets_and_extra_modifiers_expand_through_any_chord_member() {
    let mut snapshot = (*cfg(|extras| {
        extras.simultaneous.as_mut().unwrap().after_key_up = vec![key("z").into(); 3]
    }))
    .clone();
    snapshot.simple = vec![
        rule("q", vec![key("b").into()]),
        rule(
            "w",
            vec![Action {
                modifiers: vec![key("b")],
                ..key("x").into()
            }],
        ),
    ];
    let bounds = snapshot.timed_stage_source_bounds();
    assert_eq!(bounds[&key("b")], 6);
    assert_eq!(bounds[&key("q")], 6);
    assert_eq!(bounds[&key("w")], 8);
}
#[test]
fn either_first_member_gets_full_lease_and_lease_survives_early_original_up() {
    for first in ["a", "b"] {
        let cfg = cfg(|_| {});
        assert!(PhysicalStageController::new(cfg.clone()).is_err());
        let mut actor = PhysicalStageController::new_with_chord_model(cfg.clone()).unwrap();
        let second = if first == "a" { "b" } else { "a" };
        actor
            .handle(input(1, 100, 1, first, KeyEdge::Down))
            .unwrap();
        assert!(drain(&mut actor, 100).is_empty());
        assert_eq!(actor.reserved_sources(), 3);
        actor
            .handle(input(2, 110, 2, second, KeyEdge::Down))
            .unwrap();
        assert_eq!(drain(&mut actor, 110), [(key("c"), KeyEdge::Down)]);
        assert_eq!(actor.reserved_sources(), 6);
        actor.handle(input(3, 120, 1, first, KeyEdge::Up)).unwrap();
        assert_eq!(drain(&mut actor, 120), [(key("c"), KeyEdge::Up)]);
        assert_eq!(actor.reserved_sources(), 6);
        assert_eq!(actor.pending_bindings(), 1);
        actor.handle(input(4, 130, 2, second, KeyEdge::Up)).unwrap();
        drain(&mut actor, 130);
        assert_eq!(actor.reserved_sources(), 0);
        assert_eq!(actor.pending_bindings(), 0);
        assert_eq!(actor.active_bindings(), 0);
    }
}
#[test]
fn all_mode_repeat_query_tracks_actual_shared_leaf_without_allocating_new_roots() {
    let cfg = cfg(|extras| extras.simultaneous.as_mut().unwrap().key_up_when = KeyUpWhen::All);
    let mut actor = PhysicalStageController::new_with_chord_model(cfg).unwrap();
    actor.handle(input(1, 100, 1, "a", KeyEdge::Down)).unwrap();
    drain(&mut actor, 100);
    assert_eq!(actor.physical_repeat_target(device(1), key("a")), None);
    actor.handle(input(2, 110, 2, "b", KeyEdge::Down)).unwrap();
    drain(&mut actor, 110);
    let lease = actor.reserved_sources();
    let repeated = actor
        .handle(input(3, 115, 2, "b", KeyEdge::Repeat))
        .unwrap();
    assert_eq!(repeated.repeat_target, Some(key("c")));
    assert!(repeated.outputs.is_empty());
    assert_eq!(actor.reserved_sources(), lease);
    actor.handle(input(4, 120, 1, "a", KeyEdge::Up)).unwrap();
    drain(&mut actor, 120);
    assert_eq!(
        actor.physical_repeat_target(device(2), key("b")),
        Some(key("c"))
    );
    actor.handle(input(5, 130, 2, "b", KeyEdge::Up)).unwrap();
    assert_eq!(drain(&mut actor, 130), [(key("c"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn member_device_removal_releases_only_its_lease_and_survivor_waits_real_up() {
    let cfg = cfg(|extras| extras.after_key_up = vec![key("z").into()]);
    let mut actor = PhysicalStageController::new_with_chord_model(cfg).unwrap();
    actor.handle(input(1, 100, 1, "a", KeyEdge::Down)).unwrap();
    drain(&mut actor, 100);
    actor.handle(input(2, 110, 2, "b", KeyEdge::Down)).unwrap();
    drain(&mut actor, 110);
    let removed = actor
        .observe_device(device(2), true, MonoTime(115))
        .unwrap();
    assert_eq!(edges(removed.outputs), [(key("c"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 4);
    assert_eq!(actor.active_bindings(), 1);
    assert_eq!(actor.physical_repeat_target(device(1), key("a")), None);
    actor.handle(input(3, 120, 1, "a", KeyEdge::Up)).unwrap();
    assert!(drain(&mut actor, 120).is_empty());
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn cold_root_budget_rejection_does_not_consume_sequence_or_identity_and_can_retry_same_sequence() {
    let mut snapshot = (*cfg(|extras| {
        extras.simultaneous.as_mut().unwrap().after_key_up = vec![key("z").into(); 512]
    }))
    .clone();
    snapshot.complex[0].to = vec![key("c").into(); 512];
    snapshot.simple = vec![rule("q", vec![key("b").into(); 64])];
    let cfg = Arc::new(snapshot);
    assert!(cfg.timed_stage_source_bounds()[&key("q")] > MAX_OUTPUT_OWNERS);
    let mut actor = PhysicalStageController::new_with_chord_model(cfg).unwrap();
    assert!(matches!(
        actor.handle(input(1, 100, 1, "q", KeyEdge::Down)),
        Err(PhysicalControllerError::Engine(EngineError::ResourceLimit))
    ));
    assert!(!actor.is_failed());
    assert_eq!(actor.reserved_sources(), 0);
    assert_eq!(actor.active_bindings(), 0);
    actor.handle(input(1, 100, 1, "x", KeyEdge::Down)).unwrap();
    assert_eq!(drain(&mut actor, 100), [(key("x"), KeyEdge::Down)]);
    actor.reset(MonoTime(110)).unwrap();
    assert_eq!(actor.reserved_sources(), 0);
}
#[test]
fn hot_reload_updates_new_bound_but_keeps_old_group_lease_until_old_after_up_finishes() {
    let cfg = cfg(|extras| {
        extras.simultaneous.as_mut().unwrap().after_key_up = vec![Action {
            hold_down_ns: 50,
            ..key("z").into()
        }]
    });
    let mut actor = PhysicalStageController::new_with_chord_model(cfg).unwrap();
    actor.handle(input(1, 100, 1, "a", KeyEdge::Down)).unwrap();
    drain(&mut actor, 100);
    actor.handle(input(2, 110, 2, "b", KeyEdge::Down)).unwrap();
    drain(&mut actor, 110);
    assert_eq!(actor.reserved_sources(), 8);
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    actor.handle(input(3, 120, 1, "a", KeyEdge::Up)).unwrap();
    drain(&mut actor, 120);
    actor.handle(input(4, 130, 2, "b", KeyEdge::Up)).unwrap();
    assert_eq!(drain(&mut actor, 130), [(key("z"), KeyEdge::Down)]);
    assert_eq!(actor.reserved_sources(), 8);
    assert_eq!(drain(&mut actor, 180), [(key("z"), KeyEdge::Up)]);
    assert_eq!(actor.reserved_sources(), 0);
    actor.handle(input(5, 190, 1, "a", KeyEdge::Down)).unwrap();
    assert_eq!(actor.reserved_sources(), 3);
    assert_eq!(drain(&mut actor, 190), [(key("a"), KeyEdge::Down)]);
    actor.reset(MonoTime(200)).unwrap();
}
#[test]
fn reset_while_window_waits_reclaims_source_lease_without_ever_emitting_up() {
    let cfg = cfg(|_| {});
    let mut actor = PhysicalStageController::new_with_chord_model(cfg).unwrap();
    actor.handle(input(1, 100, 1, "a", KeyEdge::Down)).unwrap();
    drain(&mut actor, 100);
    assert_eq!(actor.reserved_sources(), 3);
    let reset = actor.reset(MonoTime(110)).unwrap();
    assert!(reset.outputs.is_empty());
    assert_eq!(actor.reserved_sources(), 0);
    assert_eq!(actor.active_bindings(), 0);
    assert_eq!(actor.pending_bindings(), 0);
}
