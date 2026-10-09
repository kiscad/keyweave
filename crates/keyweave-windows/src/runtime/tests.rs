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
        vec![(0x30, 8), (0x30, 10), (0x2e, 8), (0x2e, 10)]
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
    assert_eq!(actor.engine.state().pending_deadlines, 0);
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
    assert_eq!(actor.engine.state().pending_deadlines, 0);
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
