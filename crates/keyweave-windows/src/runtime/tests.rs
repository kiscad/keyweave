#![allow(clippy::unwrap_used)]
use super::*;
use crate::{
    hook::HookDispatcher,
    keys::{HookRecord, IdentityPolicy},
    pump::{Insertion, SubmitError},
};
use keyweave_engine::{Mapping, MappingExtras};
use std::collections::VecDeque;
struct Backend {
    calls: Rc<RefCell<Vec<Vec<keys::KeyboardPacket>>>>,
    replies: VecDeque<Result<Insertion, SubmitError>>,
    zero: bool,
}
impl SendBackend for Backend {
    fn submit(&mut self, packets: &[keys::KeyboardPacket]) -> Result<Insertion, SubmitError> {
        self.calls.borrow_mut().push(packets.to_vec());
        self.replies.pop_front().unwrap_or(Ok(Insertion {
            accepted: if self.zero { 0 } else { packets.len() },
            last_error: None,
        }))
    }
}
fn key(usage: u16) -> KeyId {
    KeyId::new(KEYBOARD_PAGE, usage)
}
fn mapping(from: u16, to: u16) -> Mapping {
    Mapping {
        id: "test".into(),
        from: key(from),
        to: vec![key(to).into()],
        allow_extra_modifiers: true,
        extras: MappingExtras::default(),
    }
}
fn config(generation: u64, maps: Vec<Mapping>) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "test".into(),
        simple: maps,
        complex: vec![],
    })
}
fn record(up: bool) -> HookRecord {
    HookRecord {
        message: if up { 0x101 } else { 0x100 },
        vk_code: 0x41,
        scan_code: 0x1e,
        flags: if up { 0x80 } else { 0 },
        extra_info: 0,
    }
}
fn event(usage: u16, edge: KeyEdge) -> HookEvent {
    HookEvent {
        identity: keys::ResolvedKey {
            key: key(usage),
            known_aliases: false,
        },
        edge,
        binding: if edge == KeyEdge::Down {
            None
        } else {
            Some(CaptureDecision::Capture)
        },
    }
}
type Calls = Rc<RefCell<Vec<Vec<keys::KeyboardPacket>>>>;
fn application_condition(unless: bool) -> Condition {
    Condition::Application {
        bundle_identifiers: vec![],
        file_paths: vec![keyweave_engine::Pattern::compile("^editor$").unwrap()],
        unless,
    }
}
fn context(revision: u64, at: u64, until: u64, path: Option<&str>) -> ContextSnapshot {
    ContextSnapshot {
        revision,
        provider: "test-only".into(),
        observed_at_ns: MonoTime(at),
        valid_until_ns: MonoTime(until),
        application: path.map(|path| ApplicationContext {
            bundle_identifier: None,
            file_path: Some(path.into()),
        }),
        device_inventory_scope: DeviceInventoryScope::Partial,
        ..Default::default()
    }
}
fn actor(
    config: Arc<ConfigSnapshot>,
    replies: Vec<Result<Insertion, SubmitError>>,
) -> (SessionActor<Backend>, Calls) {
    let calls = Rc::new(RefCell::new(vec![]));
    let backend = Backend {
        calls: calls.clone(),
        replies: replies.into(),
        zero: false,
    };
    let mut actor = SessionActor::new(backend, InjectionTag::new(1).unwrap(), config).unwrap();
    actor.start_after_neutral().unwrap();
    (actor, calls)
}
#[test]
fn dispatcher_engine_output_pipeline_captures_passthrough_once() {
    let (actor, calls) = actor(config(1, vec![]), vec![]);
    let actor = Rc::new(RefCell::new(actor));
    let handler = ActorHandler {
        actor: actor.clone(),
        clock: || Some(MonoTime(0)),
    };
    let mut dispatcher = HookDispatcher::new(
        handler,
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
    );
    assert_eq!(
        dispatcher.dispatch(0, Some(record(false))),
        CaptureDecision::Capture
    );
    assert_eq!(
        dispatcher.dispatch(0, Some(record(true))),
        CaptureDecision::Capture
    );
    assert_eq!(
        calls
            .borrow()
            .iter()
            .flatten()
            .map(|packet| (packet.scan, packet.flags))
            .collect::<Vec<_>>(),
        vec![(0x1e, 8), (0x1e, 10)]
    );
    actor.borrow_mut().stop_after_unhook().unwrap();
    assert_eq!(actor.borrow().phase(), ActorPhase::Stopped);
}
#[test]
fn context_updates_affect_new_input_without_rematching_held_owners() {
    let mut map = mapping(4, 5);
    map.extras.conditions = vec![application_condition(false)];
    let (mut actor, calls) = actor(config(1, vec![map]), vec![]);
    actor
        .context_changed(context(1, 0, 100, Some("editor")), MonoTime(0))
        .unwrap();
    assert!(calls.borrow().is_empty());
    actor.key(event(4, KeyEdge::Down), MonoTime(1)).unwrap();
    actor
        .context_changed(context(2, 2, 100, None), MonoTime(2))
        .unwrap();
    assert_eq!(
        actor.output.ledger().desired(),
        &std::collections::BTreeSet::from([key(5)])
    );
    actor.key(event(4, KeyEdge::Up), MonoTime(3)).unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(4)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(5)).unwrap();
    assert_eq!(actor.seq, 6);
    assert_eq!(
        calls
            .borrow()
            .iter()
            .flatten()
            .map(|p| (p.scan, p.flags))
            .collect::<Vec<_>>(),
        vec![(0x30, 8), (0x30, 10), (0x1e, 8), (0x1e, 10)]
    );
    actor.stop_after_unhook().unwrap();
}
#[test]
fn expired_or_missing_context_does_not_enable_unless() {
    let mut map = mapping(4, 5);
    map.extras.conditions = vec![application_condition(true)];
    let (mut actor, calls) = actor(config(1, vec![map]), vec![]);
    for (revision, at, until, path) in [(1, 0, 2, Some("other")), (2, 4, 10, None)] {
        actor
            .context_changed(context(revision, at, until, path), MonoTime(at))
            .unwrap();
        let now = if revision == 1 { 2 } else { at };
        actor.key(event(4, KeyEdge::Down), MonoTime(now)).unwrap();
        actor.key(event(4, KeyEdge::Up), MonoTime(now + 1)).unwrap();
    }
    assert!(calls.borrow().iter().flatten().all(|p| p.scan == 0x1e));
    actor.stop_after_unhook().unwrap();
}
#[test]
fn context_at_held_deadline_runs_due_actions_with_previous_snapshot() {
    let mut map = mapping(4, 5);
    let mut held: keyweave_engine::Action = key(6).into();
    held.conditions = vec![application_condition(false)];
    map.extras.if_held_down = vec![held];
    map.extras.held_threshold_ns = 10;
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "context held".into(),
        simple: vec![],
        complex: vec![map],
    });
    let (mut actor, calls) = actor(cfg, vec![]);
    actor
        .context_changed(context(1, 0, 100, Some("editor")), MonoTime(0))
        .unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    actor
        .context_changed(context(2, 10, 100, None), MonoTime(10))
        .unwrap();
    assert_eq!(
        actor.output.ledger().desired(),
        &std::collections::BTreeSet::from([key(6)])
    );
    assert_eq!(actor.seq, 4); // Context, down, timer, context share one sequence.
    actor.key(event(4, KeyEdge::Up), MonoTime(11)).unwrap();
    assert_eq!(
        calls
            .borrow()
            .iter()
            .flatten()
            .map(|p| (p.scan, p.flags))
            .collect::<Vec<_>>(),
        vec![(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 10)]
    );
    actor.stop_after_unhook().unwrap();
}
#[test]
fn invalid_context_stops_actor_and_retains_output_for_unhook_first_recovery() {
    let (mut actor, calls) = actor(config(1, vec![]), vec![]);
    actor
        .context_changed(context(1, 0, 100, None), MonoTime(0))
        .unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(1)).unwrap();
    let before = actor.seq;
    assert!(matches!(
        actor.context_changed(context(1, 2, 100, None), MonoTime(2)),
        Err(RuntimeError::Engine(EngineError::InvalidContext))
    ));
    assert_eq!(actor.seq, before);
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert_eq!(calls.borrow().len(), 1);
    assert!(actor.output.ledger().submitted().contains(&key(4)));
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.phase(), ActorPhase::Stopped);
    assert!(actor.output.ledger().submitted().is_empty());
}
#[test]
fn held_binding_uses_old_config_then_new_down_uses_new_config() {
    let (mut actor, calls) = actor(config(1, vec![mapping(4, 5)]), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    actor.activate(config(2, vec![mapping(4, 6)])).unwrap();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(1)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(2)).unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(3)).unwrap();
    actor.stop_after_unhook().unwrap();
    assert_eq!(
        calls
            .borrow()
            .iter()
            .flatten()
            .map(|packet| (packet.scan, packet.flags))
            .collect::<Vec<_>>(),
        vec![(0x30, 8), (0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 10)]
    );
}
#[test]
fn held_deadline_runs_before_same_time_input_and_does_not_survive_stop() {
    let mut map = mapping(4, 5);
    map.extras.if_held_down = vec![key(6).into()];
    map.extras.held_threshold_ns = 10;
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "held".into(),
        simple: vec![],
        complex: vec![map],
    });
    let (mut actor, calls) = actor(cfg, vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    assert_eq!(actor.next_wakeup(), Some(MonoTime(10)));
    actor.key(event(7, KeyEdge::Down), MonoTime(10)).unwrap();
    let scans = calls
        .borrow()
        .iter()
        .flatten()
        .map(|packet| packet.scan)
        .collect::<Vec<_>>();
    assert!(
        scans.iter().position(|scan| *scan == 0x2e).unwrap()
            < scans.iter().position(|scan| *scan == 0x20).unwrap()
    );
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.engine.legacy().state().pending_deadlines, 0);
    assert!(actor.next_wakeup().is_none());
}
#[test]
fn partial_native_output_stops_then_recovers_only_accepted_prefix() {
    let mut map = mapping(4, 5);
    map.to.push(key(6).into());
    let (mut actor, calls) = actor(
        config(1, vec![map]),
        vec![Ok(Insertion {
            accepted: 1,
            last_error: Some(0),
        })],
    );
    assert!(matches!(
        actor.key(event(4, KeyEdge::Down), MonoTime(0)),
        Err(RuntimeError::Incomplete)
    ));
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert_eq!(actor.output().ledger().submitted().len(), 1);
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.phase(), ActorPhase::Stopped);
    assert_eq!(calls.borrow()[1].len(), 1);
    assert_eq!(
        (calls.borrow()[1][0].scan, calls.borrow()[1][0].flags),
        (0x30, 10)
    );
}
#[test]
fn unknown_acceptance_preserves_ledger_and_does_not_guess_cleanup() {
    let (mut actor, calls) = actor(
        config(1, vec![]),
        vec![
            Ok(Insertion {
                accepted: 1,
                last_error: None,
            }),
            Err(SubmitError::Unknown),
        ],
    );
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    assert!(actor.key(event(5, KeyEdge::Down), MonoTime(1)).is_err());
    assert!(matches!(
        actor.stop_after_unhook(),
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(actor.phase(), ActorPhase::Unresolved);
    assert_eq!(actor.output().ledger().submitted().len(), 1);
    assert_eq!(calls.borrow().len(), 2);
}
#[test]
fn persistent_zero_recovery_is_bounded_without_clearing_submitted() {
    let mut replies = vec![Ok(Insertion {
        accepted: 1,
        last_error: None,
    })];
    replies.extend(std::iter::repeat_n(
        Ok(Insertion {
            accepted: 0,
            last_error: Some(0),
        }),
        MAX_RECOVERY,
    ));
    let (mut actor, calls) = actor(config(1, vec![]), replies);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    assert!(matches!(
        actor.stop_after_unhook(),
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(calls.borrow().len(), 1 + MAX_RECOVERY);
    assert_eq!(actor.output().ledger().submitted().len(), 1);
    assert!(actor.start_after_neutral().is_err());
}
#[test]
fn rejected_config_and_time_failure_preserve_original_binding_and_trusted_clock() {
    let (mut actor, calls) = actor(config(1, vec![mapping(4, 5)]), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(5)).unwrap();
    assert!(actor.activate(config(2, vec![mapping(4, 0x32)])).is_err());
    actor.key(event(4, KeyEdge::Up), MonoTime(6)).unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(7)).unwrap();
    assert!(matches!(actor.tick(MonoTime(4)), Err(RuntimeError::Time)));
    assert_eq!(actor.now, MonoTime(7));
    actor.stop_after_unhook().unwrap();
    assert!(
        calls
            .borrow()
            .iter()
            .flatten()
            .all(|packet| packet.scan == 0x30)
    );
}

#[test]
fn clock_provider_failure_stops_and_preserves_diagnostic() {
    let (actor, calls) = actor(config(1, vec![]), vec![]);
    let actor = Rc::new(RefCell::new(actor));
    let mut handler = ActorHandler {
        actor: actor.clone(),
        clock: || None,
    };
    assert_eq!(
        handler.handle(event(4, KeyEdge::Down)),
        Err(HookFault::Handler)
    );
    assert_eq!(actor.borrow().phase(), ActorPhase::StopRequired);
    assert!(matches!(
        actor.borrow().failure(),
        Some(RuntimeError::Clock)
    ));
    actor.borrow_mut().stop_after_unhook().unwrap();
    assert!(calls.borrow().is_empty());
}

#[test]
fn exhausted_callback_budget_keeps_unsent_timer_edges_out_of_recovery() {
    let mut map = mapping(4, 5);
    map.extras.if_held_down = vec![key(6).into()];
    map.extras.held_threshold_ns = 10;
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "budget".into(),
        simple: vec![],
        complex: vec![map],
    });
    let (mut actor, calls) = actor(cfg, vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    actor.advance(MonoTime(10)).unwrap();
    // Model one operation remaining in the callback's shared work budget.
    // Timer changes desired state, but its two edges must not be sent after
    // exhaustion. Cleanup must release only the earlier accepted b down.
    let result = actor.due(&mut (MAX_WORK - 1));
    assert!(matches!(actor.result(result), Err(RuntimeError::Budget)));
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert_eq!(actor.output().ledger().submitted().len(), 1);
    assert_eq!(calls.borrow().len(), 1);
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.engine.legacy().state().pending_deadlines, 0);
    assert!(actor.output().ledger().submitted().is_empty());
    assert_eq!(calls.borrow()[1][0].scan, 0x30);
    assert_eq!(calls.borrow()[1][0].flags, 10);
}

#[test]
fn control_delivers_held_action_without_another_input_event() {
    use crate::reactor::SessionControl;
    let mut map = mapping(4, 5);
    map.extras.if_held_down = vec![key(6).into()];
    map.extras.held_threshold_ns = 10;
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "idle".into(),
        simple: vec![],
        complex: vec![map],
    });
    let (mut actor, calls) = actor(cfg, vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    let actor = Rc::new(RefCell::new(actor));
    let mut times = VecDeque::from([MonoTime(9), MonoTime(10)]);
    let mut control = SessionControl {
        actor: actor.clone(),
        clock: || times.pop_front(),
    };
    assert_eq!(control.next_wait().unwrap().timeout_ms(), 1);
    assert_eq!(calls.borrow().len(), 1);
    assert_eq!(control.next_wait().unwrap().timeout_ms(), 20);
    assert_eq!(calls.borrow().last().unwrap().last().unwrap().scan, 0x2e);
    assert!(actor.borrow().next_wakeup().is_none());
    actor.borrow_mut().stop_after_unhook().unwrap();
}

fn packets(calls: &Calls) -> Vec<(u16, u32)> {
    calls
        .borrow()
        .iter()
        .flatten()
        .map(|p| (p.scan, p.flags))
        .collect()
}
#[test]
fn physical_repeat_uses_its_binding_not_last_global_output() {
    let (mut actor, calls) = actor(config(1, vec![mapping(4, 5)]), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    actor.key(event(6, KeyEdge::Down), MonoTime(1)).unwrap();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(2)).unwrap();
    assert_eq!(packets(&calls), vec![(0x30, 8), (0x2e, 8), (0x30, 8)]);
    assert_eq!(actor.engine.legacy().state().bindings, 2);
    assert_eq!(actor.output().ledger().submitted().len(), 2);
    actor.stop_after_unhook().unwrap();
    assert_eq!(packets(&calls)[3..], [(0x30, 10), (0x2e, 10)]);
}
#[test]
fn physical_repeat_sends_only_macro_terminal_and_never_replays_prefix() {
    let mut map = mapping(4, 5);
    map.to.push(key(6).into());
    let (mut actor, calls) = actor(config(1, vec![map]), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(1)).unwrap();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(2)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(3)).unwrap();
    assert_eq!(
        packets(&calls),
        vec![
            (0x30, 8),
            (0x30, 10),
            (0x2e, 8),
            (0x2e, 8),
            (0x2e, 8),
            (0x2e, 10)
        ]
    );
    assert!(actor.output().ledger().submitted().is_empty());
}
#[test]
fn physical_repeat_false_does_not_repeat_another_sources_same_output() {
    let mut tap = mapping(4, 5);
    tap.to[0].repeat = false;
    let (mut actor, calls) = actor(config(1, vec![tap]), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    actor.key(event(5, KeyEdge::Down), MonoTime(1)).unwrap();
    let before = calls.borrow().len();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(2)).unwrap();
    assert_eq!(calls.borrow().len(), before);
    actor.key(event(5, KeyEdge::Repeat), MonoTime(3)).unwrap();
    assert_eq!(packets(&calls).last(), Some(&(0x30, 8)));
    actor.stop_after_unhook().unwrap();
}
#[test]
fn source_repeat_at_held_deadline_targets_replacement_after_due_work() {
    let mut map = mapping(4, 5);
    map.extras.if_held_down = vec![key(6).into()];
    map.extras.held_threshold_ns = 10;
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "held".into(),
        simple: vec![],
        complex: vec![map],
    });
    let (mut actor, calls) = actor(cfg, vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(10)).unwrap();
    assert_eq!(
        packets(&calls),
        vec![(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 8)]
    );
    actor.stop_after_unhook().unwrap();
}
#[test]
fn repeat_failure_stops_and_recovers_single_accepted_down_without_new_owner() {
    let (mut actor, calls) = actor(
        config(1, vec![]),
        vec![
            Ok(Insertion {
                accepted: 1,
                last_error: None,
            }),
            Ok(Insertion {
                accepted: 0,
                last_error: Some(5),
            }),
        ],
    );
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    assert!(matches!(
        actor.key(event(4, KeyEdge::Repeat), MonoTime(1)),
        Err(RuntimeError::Incomplete)
    ));
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert_eq!(actor.output().ledger().submitted().len(), 1);
    actor.stop_after_unhook().unwrap();
    assert_eq!(packets(&calls), vec![(0x1e, 8), (0x1e, 8), (0x1e, 10)]);
    assert_eq!(actor.phase(), ActorPhase::Stopped);
}
#[test]
fn unknown_repeat_acceptance_is_quarantined_and_does_not_clear_known_down() {
    let (mut actor, calls) = actor(
        config(1, vec![]),
        vec![
            Ok(Insertion {
                accepted: 1,
                last_error: None,
            }),
            Err(SubmitError::Unknown),
        ],
    );
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    assert!(actor.key(event(4, KeyEdge::Repeat), MonoTime(1)).is_err());
    assert!(matches!(
        actor.stop_after_unhook(),
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(actor.output().ledger().submitted().len(), 1);
    assert_eq!(calls.borrow().len(), 2);
}
#[test]
fn dispatcher_repeats_physical_binding_but_bypasses_injected_reentry() {
    let (actor, calls) = actor(config(1, vec![]), vec![]);
    let actor = Rc::new(RefCell::new(actor));
    let handler = ActorHandler {
        actor: actor.clone(),
        clock: || Some(MonoTime(0)),
    };
    let mut dispatcher = HookDispatcher::new(
        handler,
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
    );
    assert_eq!(
        dispatcher.dispatch(0, Some(record(false))),
        CaptureDecision::Capture
    );
    let mut injected = record(false);
    injected.flags = 0x10;
    injected.extra_info = 1;
    assert_eq!(
        dispatcher.dispatch(0, Some(injected)),
        CaptureDecision::Pass
    );
    assert_eq!(
        dispatcher.dispatch(0, Some(record(false))),
        CaptureDecision::Capture
    );
    assert_eq!(
        dispatcher.dispatch(0, Some(record(true))),
        CaptureDecision::Capture
    );
    assert_eq!(packets(&calls), vec![(0x1e, 8), (0x1e, 8), (0x1e, 10)]);
    actor.borrow_mut().stop_after_unhook().unwrap();
}

#[test]
fn source_repeat_uses_current_modifiers_without_replaying_shortcut_scope() {
    let mut map = mapping(4, 5);
    map.to[0].modifiers = vec![key(0xe0)];
    let (mut actor, calls) = actor(config(1, vec![map]), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    assert_eq!(packets(&calls), vec![(0x1d, 8), (0x30, 8)]);
    actor.key(event(0xe1, KeyEdge::Down), MonoTime(1)).unwrap();
    assert_eq!(
        packets(&calls),
        vec![(0x1d, 8), (0x30, 8), (0x1d, 10), (0x2a, 8)]
    );
    let before = calls.borrow().len();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(2)).unwrap();
    assert_eq!(calls.borrow().len(), before + 1);
    assert_eq!(packets(&calls).last(), Some(&(0x30, 8)));
    assert!(actor.output().ledger().submitted().contains(&key(0xe1)));
    actor
        .key(event(0xe1, KeyEdge::Repeat), MonoTime(3))
        .unwrap();
    assert_eq!(calls.borrow().len(), before + 1);
    actor.stop_after_unhook().unwrap();
}

#[test]
fn untracked_input_with_empty_known_output_stops_without_any_native_submission() {
    let (mut actor, calls) = actor(config(1, vec![]), vec![]);
    actor.stop_for_untracked_input_after_unhook().unwrap();
    assert_eq!(actor.phase(), ActorPhase::Stopped);
    assert!(calls.borrow().is_empty());
    assert!(matches!(
        actor.failure(),
        Some(RuntimeError::UntrackedInput)
    ));
}
#[test]
fn untracked_input_retains_submitted_and_cancels_engine_timers_without_cleanup() {
    let mut map = mapping(4, 5);
    map.extras.if_held_down = vec![key(6).into()];
    map.extras.held_threshold_ns = 10;
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "held".into(),
        simple: vec![],
        complex: vec![map],
    });
    let (mut actor, calls) = actor(cfg, vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(0)).unwrap();
    assert!(matches!(
        actor.stop_for_untracked_input_after_unhook(),
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(actor.engine.legacy().state().pending_deadlines, 0);
    assert_eq!(actor.output().ledger().submitted().len(), 1);
    assert_eq!(actor.output().phase(), Phase::Quarantined);
    assert_eq!(calls.borrow().len(), 1);
    assert!(actor.start_after_neutral().is_err());
    assert!(actor.stop_after_unhook().is_err());
}
#[test]
fn untracked_input_cannot_turn_unknown_empty_prefix_into_clean_stop() {
    let (mut actor, calls) = actor(config(1, vec![]), vec![Err(SubmitError::Unknown)]);
    assert!(actor.key(event(4, KeyEdge::Down), MonoTime(0)).is_err());
    assert!(actor.output().ledger().submitted().is_empty());
    assert!(matches!(
        actor.stop_for_untracked_input_after_unhook(),
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(actor.phase(), ActorPhase::Unresolved);
    assert_eq!(calls.borrow().len(), 1);
}
#[test]
fn pure_preflight_rejects_device_conditions_and_special_outputs_in_all_actions() {
    let mut rule = mapping(4, 5);
    rule.to[0].conditions.push(Condition::Device {
        identifiers: vec![],
        unless: false,
        exists: false,
    });
    assert!(matches!(
        preflight_config(config(1, vec![rule])),
        Err(RuntimeError::DeviceCondition)
    ));
    let mut rule = mapping(4, 5);
    rule.extras.if_alone = vec![key(0x46).into()];
    assert!(matches!(
        preflight_config(config(1, vec![rule])),
        Err(RuntimeError::Key(KeyError::SpecialSequence))
    ));
}
#[test]
fn pure_preflight_shares_engine_timing_gate_and_does_not_construct_output() {
    preflight_config(config(1, vec![mapping(4, 5)])).unwrap();
    let mut rule = mapping(4, 5);
    rule.to[0].hold_down_ns = 1;
    assert!(matches!(
        preflight_config(config(1, vec![rule])),
        Err(RuntimeError::Engine(EngineError::UnsupportedActionTiming))
    ));
    let calls = Rc::new(RefCell::new(vec![]));
    let backend = Backend {
        calls: calls.clone(),
        replies: VecDeque::new(),
        zero: false,
    };
    let mut rule = mapping(4, 5);
    rule.to[0].hold_down_ns = 1;
    assert!(matches!(
        SessionActor::new(
            backend,
            InjectionTag::new(1).unwrap(),
            config(1, vec![rule])
        ),
        Err(RuntimeError::Engine(EngineError::UnsupportedActionTiming))
    ));
    assert!(calls.borrow().is_empty());
}
fn staged_actor(
    config: Arc<ConfigSnapshot>,
    replies: Vec<Result<Insertion, SubmitError>>,
) -> (SessionActor<Backend>, Calls) {
    let calls = Rc::new(RefCell::new(vec![]));
    let backend = Backend {
        calls: calls.clone(),
        replies: replies.into(),
        zero: false,
    };
    let mut actor =
        SessionActor::new_staged(backend, InjectionTag::new(1).unwrap(), config).unwrap();
    actor.start_after_neutral().unwrap();
    (actor, calls)
}
fn timed_config() -> Arc<ConfigSnapshot> {
    let mut rule = mapping(4, 5);
    rule.to = vec![
        keyweave_engine::Action {
            hold_down_ns: 50,
            ..key(5).into()
        },
        key(6).into(),
    ];
    config(1, vec![rule])
}

#[test]
fn explicit_mode_factory_retains_cold_lifecycle_and_selects_timing_engine() {
    for mode in [ExecutionMode::Legacy, ExecutionMode::StagedKeyboard] {
        let calls = Rc::new(RefCell::new(vec![]));
        let backend = Backend {
            calls: calls.clone(),
            replies: VecDeque::new(),
            zero: false,
        };
        let actor = SessionActor::new_with_mode(
            backend,
            InjectionTag::new(1).unwrap(),
            timed_config(),
            mode,
        );
        assert!(calls.borrow().is_empty());
        match mode {
            ExecutionMode::Legacy => assert!(matches!(
                actor,
                Err(RuntimeError::Engine(EngineError::UnsupportedActionTiming))
            )),
            ExecutionMode::StagedKeyboard => {
                let mut actor = actor.unwrap();
                assert_eq!(actor.execution_mode(), mode);
                assert_eq!(actor.phase(), ActorPhase::Standby);
                assert!(matches!(
                    actor.key(event(4, KeyEdge::Down), MonoTime(100)),
                    Err(RuntimeError::NotActive)
                ));
                assert!(calls.borrow().is_empty());
                actor.start_after_neutral().unwrap();
                actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
                actor.key(event(4, KeyEdge::Up), MonoTime(110)).unwrap();
                assert_eq!(packet_edges(&calls), [(0x30, 8)]);
                actor.tick(MonoTime(150)).unwrap();
                assert_eq!(
                    packet_edges(&calls),
                    [(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 10)]
                );
                actor.stop_after_unhook().unwrap();
            }
        }
    }
}
fn packet_edges(calls: &Calls) -> Vec<(u16, u32)> {
    calls
        .borrow()
        .iter()
        .flatten()
        .map(|packet| (packet.scan, packet.flags))
        .collect()
}
#[test]
fn staged_early_up_keeps_array_duration_and_no_timer_input_sequences_are_fabricated() {
    let (mut actor, calls) = staged_actor(timed_config(), vec![]);
    assert_eq!(
        actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap(),
        CaptureDecision::Capture
    );
    assert_eq!(packet_edges(&calls), [(0x30, 8)]);
    assert_eq!(actor.next_wakeup(), Some(MonoTime(150)));
    actor.key(event(4, KeyEdge::Up), MonoTime(110)).unwrap();
    actor.tick(MonoTime(149)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8)]);
    actor.tick(MonoTime(150)).unwrap();
    assert_eq!(
        packet_edges(&calls),
        [(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 10)]
    );
    assert_eq!(actor.seq, 2);
    assert!(actor.output().ledger().submitted().is_empty());
    assert_eq!(actor.next_wakeup(), None);
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.phase(), ActorPhase::Stopped);
}
#[test]
fn staged_repeat_waits_for_actual_saved_primary_and_accepted_desired_owner() {
    let (mut actor, calls) = staged_actor(timed_config(), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(110)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8)]);
    actor.tick(MonoTime(150)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8), (0x30, 10), (0x2e, 8)]);
    actor.key(event(4, KeyEdge::Repeat), MonoTime(160)).unwrap();
    assert_eq!(
        packet_edges(&calls),
        [(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 8)]
    );
    actor.key(event(4, KeyEdge::Up), MonoTime(170)).unwrap();
    assert_eq!(packet_edges(&calls).last(), Some(&(0x2e, 10)));
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_reload_preserves_old_array_and_up_while_new_physical_root_uses_new_snapshot() {
    let (mut actor, calls) = staged_actor(timed_config(), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.activate(config(2, vec![mapping(4, 7)])).unwrap();
    assert_eq!(actor.config_generation(), ConfigGeneration(2));
    actor.tick(MonoTime(150)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(170)).unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(180)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(190)).unwrap();
    assert_eq!(
        packet_edges(&calls),
        [
            (0x30, 8),
            (0x30, 10),
            (0x2e, 8),
            (0x2e, 10),
            (0x20, 8),
            (0x20, 10)
        ]
    );
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_known_zero_submission_stops_and_discards_future_array_before_ledger_recovery() {
    let (mut actor, calls) = staged_actor(
        timed_config(),
        vec![Ok(Insertion {
            accepted: 0,
            last_error: Some(5),
        })],
    );
    assert!(matches!(
        actor.key(event(4, KeyEdge::Down), MonoTime(100)),
        Err(RuntimeError::Incomplete)
    ));
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert!(actor.output().ledger().submitted().is_empty());
    actor.stop_after_unhook().unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8)]);
    assert_eq!(actor.phase(), ActorPhase::Stopped);
    assert!(matches!(
        actor.tick(MonoTime(150)),
        Err(RuntimeError::NotActive)
    ));
}
#[test]
fn staged_user_stop_after_partial_budget_drain_recovers_only_accepted_and_drops_future_work() {
    let mut rule = mapping(4, 5);
    rule.to = vec![
        keyweave_engine::Action {
            repeat: false,
            ..key(5).into()
        };
        512
    ];
    let (mut actor, calls) = staged_actor(config(1, vec![rule]), vec![]);
    assert_eq!(
        actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap(),
        CaptureDecision::Capture
    );
    assert_eq!(actor.phase(), ActorPhase::Active);
    let submitted = actor.output().ledger().submitted().clone();
    let before = packet_edges(&calls).len();
    actor.stop_after_unhook().unwrap();
    assert_eq!(packet_edges(&calls).len(), before + submitted.len());
    assert!(
        packet_edges(&calls)[before..]
            .iter()
            .all(|(_, flags)| *flags == 10)
    );
    assert!(actor.output().ledger().submitted().is_empty());
    assert!(matches!(actor.engine, Machine::Staged(_)));
    assert_eq!(actor.engine.next_deadline(), None);
}
#[test]
fn staged_preflight_is_explicit_and_default_native_timing_gate_is_retained() {
    assert!(preflight_staged_config(timed_config()).is_ok());
    assert!(matches!(
        preflight_config(timed_config()),
        Err(RuntimeError::Engine(EngineError::UnsupportedActionTiming))
    ));
}
fn complex_config(rule: Mapping) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "staged complex".into(),
        simple: vec![],
        complex: vec![rule],
    })
}
#[test]
fn staged_same_time_held_repeat_drains_old_up_and_replacement_before_repeat_submission() {
    let mut rule = mapping(4, 5);
    rule.extras.if_held_down = vec![key(6).into()];
    rule.extras.held_threshold_ns = 50;
    let (mut actor, calls) = staged_actor(complex_config(rule), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.key(event(4, KeyEdge::Repeat), MonoTime(150)).unwrap();
    assert_eq!(
        packet_edges(&calls),
        [(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 8)]
    );
    actor.key(event(4, KeyEdge::Up), MonoTime(160)).unwrap();
    assert_eq!(packet_edges(&calls).last(), Some(&(0x2e, 10)));
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_delayed_after_original_up_runs_once_and_stop_recovers_only_accepted_live_tap() {
    let mut rule = mapping(4, 5);
    rule.extras.delayed = Some(keyweave_engine::DelayedAction {
        delay_ns: 50,
        if_invoked: vec![keyweave_engine::Action {
            hold_down_ns: 10,
            ..key(6).into()
        }],
        if_canceled: vec![],
    });
    let (mut actor, calls) = staged_actor(complex_config(rule), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(110)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8), (0x30, 10)]);
    assert_eq!(actor.next_wakeup(), Some(MonoTime(150)));
    actor.tick(MonoTime(150)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8), (0x30, 10), (0x2e, 8)]);
    assert_eq!(actor.next_wakeup(), Some(MonoTime(160)));
    actor.stop_after_unhook().unwrap();
    assert_eq!(
        packet_edges(&calls),
        [(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 10)]
    );
    assert_eq!(actor.seq, 2);
    assert_eq!(actor.engine.next_deadline(), None);
}
#[test]
fn staged_delayed_cancellation_keeps_cancel_array_fifo_before_new_source_saved_down_and_up() {
    let mut rule = mapping(4, 5);
    rule.extras.delayed = Some(keyweave_engine::DelayedAction {
        delay_ns: 50,
        if_invoked: vec![key(6).into()],
        if_canceled: vec![keyweave_engine::Action {
            hold_down_ns: 10,
            ..key(7).into()
        }],
    });
    let (mut actor, calls) = staged_actor(complex_config(rule), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(110)).unwrap();
    actor.key(event(5, KeyEdge::Down), MonoTime(120)).unwrap();
    actor.key(event(5, KeyEdge::Up), MonoTime(125)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8), (0x30, 10), (0x20, 8)]);
    actor.tick(MonoTime(130)).unwrap();
    assert_eq!(
        packet_edges(&calls),
        [
            (0x30, 8),
            (0x30, 10),
            (0x20, 8),
            (0x20, 10),
            (0x30, 8),
            (0x30, 10)
        ]
    );
    actor.tick(MonoTime(150)).unwrap();
    assert_eq!(packet_edges(&calls).len(), 6);
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_unknown_acceptance_quarantines_known_old_owner_and_does_not_guess_recovery() {
    let (mut actor, calls) = staged_actor(
        timed_config(),
        vec![
            Ok(Insertion {
                accepted: 1,
                last_error: None,
            }),
            Err(SubmitError::Unknown),
        ],
    );
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    assert!(actor.tick(MonoTime(150)).is_err());
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert_eq!(actor.output().phase(), Phase::Quarantined);
    assert!(actor.output().ledger().submitted().contains(&key(5)));
    let count = calls.borrow().len();
    assert!(matches!(
        actor.stop_after_unhook(),
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(calls.borrow().len(), count);
    assert_eq!(actor.phase(), ActorPhase::Unresolved);
    assert!(actor.output().ledger().submitted().contains(&key(5)));
}
#[test]
fn staged_simple_consumed_scope_restores_saved_ordinary_descendant_through_accepted_pump() {
    let mut trigger = mapping(4, 0xe4);
    trigger
        .extras
        .from_modifiers
        .mandatory
        .insert(keyweave_engine::Modifier::LeftShift);
    let cfg = Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "staged source scope".into(),
        simple: vec![trigger],
        complex: vec![mapping(0xe1, 0x1b)],
    });
    let (mut actor, calls) = staged_actor(cfg, vec![]);
    actor
        .key(event(0xe1, KeyEdge::Down), MonoTime(100))
        .unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(110)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(120)).unwrap();
    actor.key(event(0xe1, KeyEdge::Up), MonoTime(130)).unwrap();
    assert_eq!(
        packet_edges(&calls),
        [
            (0x2d, 8),
            (0x2d, 10),
            (0x1d, 9),
            (0x1d, 11),
            (0x2d, 8),
            (0x2d, 10)
        ]
    );
    assert!(actor.output().ledger().submitted().is_empty());
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_hook_dispatcher_bypasses_own_injected_output_without_new_sequence_or_source() {
    let (actor, calls) = staged_actor(timed_config(), vec![]);
    let actor = Rc::new(RefCell::new(actor));
    let time = Rc::new(std::cell::Cell::new(100u64));
    let sampled = time.clone();
    let handler = ActorHandler {
        actor: actor.clone(),
        clock: move || Some(MonoTime(sampled.get())),
    };
    let mut dispatcher = HookDispatcher::new(
        handler,
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
    );
    assert_eq!(
        dispatcher.dispatch(0, Some(record(false))),
        CaptureDecision::Capture
    );
    assert_eq!(packet_edges(&calls), [(0x30, 8)]);
    let injected = HookRecord {
        message: 0x100,
        vk_code: 0x42,
        scan_code: 0x30,
        flags: 0x10,
        extra_info: 1,
    };
    assert_eq!(
        dispatcher.dispatch(0, Some(injected)),
        CaptureDecision::Pass
    );
    assert_eq!(actor.borrow().seq, 1);
    assert_eq!(packet_edges(&calls), [(0x30, 8)]);
    time.set(150);
    actor.borrow_mut().tick(MonoTime(150)).unwrap();
    time.set(160);
    assert_eq!(
        dispatcher.dispatch(0, Some(record(true))),
        CaptureDecision::Capture
    );
    assert_eq!(
        packet_edges(&calls),
        [(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 10)]
    );
    actor.borrow_mut().stop_after_unhook().unwrap();
}
#[test]
fn staged_context_change_at_held_deadline_runs_due_branch_with_previous_valid_snapshot() {
    let mut rule = mapping(4, 5);
    rule.extras.if_held_down = vec![keyweave_engine::Action {
        conditions: vec![application_condition(false)],
        ..key(6).into()
    }];
    rule.extras.held_threshold_ns = 49;
    let (mut actor, calls) = staged_actor(complex_config(rule), vec![]);
    actor
        .context_changed(context(1, 100, 300, Some("editor")), MonoTime(100))
        .unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(101)).unwrap();
    actor
        .context_changed(context(2, 150, 300, Some("other")), MonoTime(150))
        .unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8), (0x30, 10), (0x2e, 8)]);
    actor.key(event(4, KeyEdge::Up), MonoTime(160)).unwrap();
    assert_eq!(packet_edges(&calls).last(), Some(&(0x2e, 10)));
    actor.stop_after_unhook().unwrap();
}
fn large_staged_rule() -> Mapping {
    let mut rule = mapping(4, 5);
    rule.to = vec![
        keyweave_engine::Action {
            repeat: false,
            ..key(5).into()
        };
        512
    ];
    rule
}
#[test]
fn staged_large_array_and_early_up_continue_across_ticks_without_stopping_or_false_reload_ack() {
    let (mut actor, calls) = staged_actor(config(1, vec![large_staged_rule()]), vec![]);
    assert_eq!(
        actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap(),
        CaptureDecision::Capture
    );
    assert!(packet_edges(&calls).len() < 1024);
    actor.key(event(4, KeyEdge::Up), MonoTime(101)).unwrap();
    assert!(actor.queued_inputs() > 0);
    assert!(matches!(
        actor.activate(config(2, vec![])),
        Err(RuntimeError::StagedInputPending)
    ));
    assert_eq!(actor.config_generation(), ConfigGeneration(1));
    assert_eq!(actor.phase(), ActorPhase::Active);
    for _ in 0..100 {
        actor.tick(MonoTime(200)).unwrap();
        if actor.next_wakeup().is_none() {
            break;
        }
    }
    assert_eq!(packet_edges(&calls).len(), 1024);
    assert!(
        packet_edges(&calls)
            .chunks_exact(2)
            .all(|pair| pair == [(0x30, 8), (0x30, 10)])
    );
    assert!(actor.output().ledger().submitted().is_empty());
    assert_eq!(actor.queued_inputs(), 0);
    assert_eq!(actor.seq, 2);
    assert_eq!(actor.phase(), ActorPhase::Active);
    actor.activate(config(2, vec![])).unwrap();
    assert_eq!(actor.config_generation(), ConfigGeneration(2));
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_queued_key_context_key_order_keeps_snapshot_choice_across_late_tick() {
    let mut before = mapping(7, 8);
    before.extras.conditions = vec![application_condition(false)];
    let mut after = mapping(9, 10);
    after.extras.conditions = vec![application_condition(true)];
    let (mut actor, calls) =
        staged_actor(config(1, vec![large_staged_rule(), before, after]), vec![]);
    actor
        .context_changed(context(1, 90, 1000, Some("editor")), MonoTime(90))
        .unwrap();
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.key(event(7, KeyEdge::Down), MonoTime(101)).unwrap();
    actor
        .context_changed(context(2, 102, 1000, Some("other")), MonoTime(102))
        .unwrap();
    actor.key(event(9, KeyEdge::Down), MonoTime(103)).unwrap();
    for _ in 0..100 {
        actor.tick(MonoTime(200)).unwrap();
        if actor.next_wakeup().is_none() {
            break;
        }
    }
    assert_eq!(&packet_edges(&calls)[1024..], [(0x12, 8), (0x22, 8)]);
    actor.key(event(7, KeyEdge::Up), MonoTime(201)).unwrap();
    actor.key(event(9, KeyEdge::Up), MonoTime(202)).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(203)).unwrap();
    assert_eq!(&packet_edges(&calls)[1026..], [(0x12, 10), (0x22, 10)]);
    assert_eq!(actor.seq, 8);
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_fifo_overflow_is_stop_required_and_discards_unprocessed_frames_before_recovery() {
    let (mut actor, calls) = staged_actor(timed_config(), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    for _ in 0..MAX_PENDING_INPUTS {
        actor
            .enqueue_input(
                InputEvent::Key {
                    key: key(4),
                    edge: KeyEdge::Repeat,
                },
                MonoTime(110),
            )
            .unwrap();
    }
    assert!(matches!(
        actor.key(event(4, KeyEdge::Up), MonoTime(120)),
        Err(RuntimeError::Budget)
    ));
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert_eq!(actor.queued_inputs(), MAX_PENDING_INPUTS);
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.queued_inputs(), 0);
    assert_eq!(packet_edges(&calls), [(0x30, 8), (0x30, 10)]);
    assert_eq!(actor.seq, 1);
}
#[test]
fn staged_invalid_context_rejects_before_queue_storage_and_preserves_accepted_owner_for_stop() {
    let (mut actor, calls) = staged_actor(timed_config(), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    assert!(matches!(
        actor.context_changed(context(1, 121, 200, Some("editor")), MonoTime(120)),
        Err(RuntimeError::Staged(_))
    ));
    assert_eq!(actor.queued_inputs(), 0);
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert!(actor.output().ledger().submitted().contains(&key(5)));
    actor.stop_after_unhook().unwrap();
    assert_eq!(packet_edges(&calls), [(0x30, 8), (0x30, 10)]);
}
#[test]
fn staged_context_revision_validation_includes_unprocessed_queued_snapshot() {
    let (mut actor, _) = staged_actor(config(1, vec![large_staged_rule()]), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor
        .context_changed(context(1, 101, 1000, Some("editor")), MonoTime(101))
        .unwrap();
    assert_eq!(actor.queued_inputs(), 1);
    assert!(matches!(
        actor.context_changed(context(1, 102, 1000, Some("other")), MonoTime(102)),
        Err(RuntimeError::Staged(_))
    ));
    assert_eq!(actor.queued_inputs(), 1);
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.queued_inputs(), 0);
}

fn chord_config(all: bool) -> Arc<ConfigSnapshot> {
    use keyweave_engine::simultaneous::{KeyUpWhen, Simultaneous};
    let mut rule = mapping(4, 6);
    let mut group = Simultaneous::new(vec![key(4), key(5)]);
    group.threshold_ns = 50;
    if all {
        group.key_up_when = KeyUpWhen::All;
    }
    rule.extras.simultaneous = Some(group);
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "session chord".into(),
        simple: vec![],
        complex: vec![rule],
    })
}
#[test]
fn staged_chord_capture_repeat_and_all_up_use_actual_accepted_leaf() {
    let (mut actor, calls) = staged_actor(chord_config(true), vec![]);
    assert_eq!(
        actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap(),
        CaptureDecision::Capture
    );
    assert!(packet_edges(&calls).is_empty());
    actor.key(event(4, KeyEdge::Repeat), MonoTime(105)).unwrap();
    assert!(packet_edges(&calls).is_empty());
    actor.key(event(5, KeyEdge::Down), MonoTime(110)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x2e, 8)]);
    actor.key(event(5, KeyEdge::Repeat), MonoTime(115)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x2e, 8), (0x2e, 8)]);
    actor.key(event(4, KeyEdge::Up), MonoTime(120)).unwrap();
    assert_eq!(
        actor
            .output()
            .ledger()
            .submitted()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![key(6)]
    );
    actor.key(event(5, KeyEdge::Up), MonoTime(130)).unwrap();
    assert_eq!(packet_edges(&calls).last(), Some(&(0x2e, 10)));
    assert!(actor.output().ledger().submitted().is_empty());
    assert_eq!(actor.seq, 6);
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_chord_timeout_and_window_stop_use_actual_sent_ownership() {
    let (mut actor, calls) = staged_actor(chord_config(false), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.tick(MonoTime(150)).unwrap();
    assert!(packet_edges(&calls).is_empty());
    actor.tick(MonoTime(151)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x1e, 8)]);
    actor.key(event(4, KeyEdge::Up), MonoTime(160)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x1e, 8), (0x1e, 10)]);
    actor.stop_after_unhook().unwrap();
    let (mut actor, calls) = staged_actor(chord_config(false), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.stop_after_unhook().unwrap();
    assert_eq!(actor.phase(), ActorPhase::Stopped);
    assert!(packet_edges(&calls).is_empty());
}
#[test]
fn staged_chord_reload_retains_old_group_and_after_up_snapshot() {
    let mut cfg = (*chord_config(false)).clone();
    cfg.complex[0]
        .extras
        .simultaneous
        .as_mut()
        .unwrap()
        .after_key_up = vec![key(7).into()];
    let (mut actor, calls) = staged_actor(Arc::new(cfg), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.key(event(5, KeyEdge::Down), MonoTime(110)).unwrap();
    actor.activate(config(2, vec![])).unwrap();
    actor.key(event(4, KeyEdge::Up), MonoTime(120)).unwrap();
    assert_eq!(packet_edges(&calls), [(0x2e, 8), (0x2e, 10)]);
    actor.key(event(5, KeyEdge::Up), MonoTime(130)).unwrap();
    assert_eq!(
        packet_edges(&calls),
        [(0x2e, 8), (0x2e, 10), (0x20, 8), (0x20, 10)]
    );
    assert!(actor.output().ledger().submitted().is_empty());
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_chord_cold_preflight_keeps_legacy_and_windows_capability_gates() {
    assert!(preflight_staged_config(chord_config(false)).is_ok());
    assert!(preflight_config(chord_config(false)).is_err());
    let mut cfg = (*chord_config(false)).clone();
    cfg.complex[0]
        .extras
        .simultaneous
        .as_mut()
        .unwrap()
        .after_key_up = vec!["button1".parse::<KeyId>().unwrap().into()];
    assert!(preflight_staged_config(Arc::new(cfg)).is_err());
    let mut cfg = (*chord_config(false)).clone();
    cfg.simple = cfg.complex.clone();
    cfg.complex.clear();
    assert!(preflight_staged_config(Arc::new(cfg)).is_err());
}
#[test]
fn staged_chord_zero_acceptance_prevents_repeat_and_future_group_after_up() {
    let mut cfg = (*chord_config(false)).clone();
    cfg.complex[0]
        .extras
        .simultaneous
        .as_mut()
        .unwrap()
        .after_key_up = vec![key(7).into()];
    let (mut actor, calls) = staged_actor(
        Arc::new(cfg),
        vec![Ok(Insertion {
            accepted: 0,
            last_error: Some(5),
        })],
    );
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    assert!(matches!(
        actor.key(event(5, KeyEdge::Down), MonoTime(110)),
        Err(RuntimeError::Incomplete)
    ));
    assert_eq!(actor.phase(), ActorPhase::StopRequired);
    assert!(actor.output().ledger().submitted().is_empty());
    assert!(matches!(
        actor.key(event(5, KeyEdge::Repeat), MonoTime(115)),
        Err(RuntimeError::NotActive)
    ));
    actor.stop_after_unhook().unwrap();
    assert_eq!(packet_edges(&calls), [(0x2e, 8)]);
}
#[test]
fn staged_chord_stop_releases_accepted_main_without_firing_after_up() {
    let mut cfg = (*chord_config(false)).clone();
    cfg.complex[0]
        .extras
        .simultaneous
        .as_mut()
        .unwrap()
        .after_key_up = vec![key(7).into()];
    let (mut actor, calls) = staged_actor(Arc::new(cfg), vec![]);
    actor.key(event(4, KeyEdge::Down), MonoTime(100)).unwrap();
    actor.key(event(5, KeyEdge::Down), MonoTime(110)).unwrap();
    actor.stop_after_unhook().unwrap();
    assert_eq!(packet_edges(&calls), [(0x2e, 8), (0x2e, 10)]);
    assert!(actor.output().ledger().submitted().is_empty());
    assert_eq!(actor.next_wakeup(), None);
}
