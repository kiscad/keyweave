#![allow(clippy::unwrap_used)]
use super::super::{
    hook_thread::{test_fail_unhook, test_quit, test_send},
    input_probe::test_probe,
};
use super::*;
use crate::{
    keys::KeyboardPacket,
    pump::{Insertion, Phase, SubmitError},
};
use keyweave_types::ConfigGeneration;
#[derive(Default)]
struct Trace {
    calls: Vec<Vec<KeyboardPacket>>,
    unknown: bool,
    zero_recovery: bool,
}
struct Backend(Rc<RefCell<Trace>>);
impl SendBackend for Backend {
    fn submit(&mut self, packets: &[KeyboardPacket]) -> Result<Insertion, SubmitError> {
        let mut trace = self.0.borrow_mut();
        // Native callback must have been disabled before any cleanup packet.
        if packets[0].flags & 2 != 0 && super::super::hook_thread::outside_callback() {
            assert_eq!(test_send(0x30, false), 9);
        }
        trace.calls.push(packets.to_vec());
        if trace.unknown {
            return Err(SubmitError::Unknown);
        }
        Ok(Insertion {
            accepted: if trace.zero_recovery && packets[0].flags & 2 != 0 {
                0
            } else {
                packets.len()
            },
            last_error: None,
        })
    }
}
fn config() -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "test".into(),
        simple: vec![],
        complex: vec![],
    })
}
fn install() -> (CaptureSession<Backend>, Rc<RefCell<Trace>>) {
    let trace = Rc::new(RefCell::new(Trace::default()));
    let session = CaptureSession::install(
        Backend(trace.clone()),
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
        config(),
    )
    .unwrap();
    (session, trace)
}
#[test]
fn passive_binding_cannot_capture_a_repeat_then_active_output_closes_first() {
    let (mut session, trace) = install();
    assert_eq!(test_send(0x1e, false), 9);
    assert_eq!(test_send(0x1e, false), 9);
    assert_eq!(session.poll().unwrap(), SessionStep::Waiting);
    assert_eq!(session.phase(), ActorPhase::Standby);
    assert!(trace.borrow().calls.is_empty());
    assert_eq!(test_send(0x1e, true), 9);
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(trace.borrow().calls.len(), 1);
    let report = session.shutdown();
    assert!(report.unhook.is_ok() && report.recovery.is_ok());
    assert_eq!(trace.borrow().calls.len(), 2);
    assert_eq!(trace.borrow().calls[1][0].flags & 2, 2);
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert!(session.shutdown().recovery.is_ok());
    assert_eq!(trace.borrow().calls.len(), 2);
    assert!(matches!(session.poll(), Err(SessionError::Closed)));
}
#[test]
fn unsupported_pass_binding_is_part_of_neutral_gate() {
    let (mut session, trace) = install();
    assert_eq!(test_send(0x45, false), 9); // special sequence, no actor identity
    assert_eq!(session.hook.held_keys().unwrap(), 1);
    assert_eq!(session.poll().unwrap(), SessionStep::Waiting);
    assert_eq!(test_send(0x45, true), 9);
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert!(trace.borrow().calls.is_empty());
    session.shutdown();
}
#[test]
fn preexisting_system_hold_waits_without_actor_mutation() {
    test_probe(i16::MIN, true);
    let (mut session, trace) = install();
    assert_eq!(session.poll().unwrap(), SessionStep::Waiting);
    assert_eq!(session.hook.held_keys().unwrap(), 0);
    test_probe(0, true);
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert!(trace.borrow().calls.is_empty());
    session.shutdown();
}
#[test]
fn sampler_callback_down_blocks_activation_even_when_idle_observed() {
    let (mut session, _) = install();
    let result = session.try_activate(
        || {
            assert_eq!(test_send(0x1e, false), 9);
            Ok(InputSample::IdleObserved)
        },
        |clock| clock.now(),
    );
    assert_eq!(result.unwrap(), SessionStep::Waiting);
    assert_eq!(session.phase(), ActorPhase::Standby);
    test_send(0x1e, true);
    session.shutdown();
}
#[test]
fn callback_full_tap_passes_and_leaves_no_binding() {
    let (mut session, trace) = install();
    assert_eq!(
        session
            .try_activate(
                || {
                    assert_eq!(test_send(0x1e, false), 9);
                    assert_eq!(test_send(0x1e, true), 9);
                    Ok(InputSample::IdleObserved)
                },
                |clock| clock.now()
            )
            .unwrap(),
        SessionStep::Activated
    );
    assert!(trace.borrow().calls.is_empty());
    session.shutdown();
}
#[test]
fn deadline_is_rechecked_after_slow_probe_and_no_actor_is_started() {
    let (mut session, trace) = install();
    let reads = Cell::new(0);
    let deadline = session.deadline;
    let error = session.try_activate(
        || Ok(InputSample::IdleObserved),
        |_| {
            reads.set(reads.get() + 1);
            Some(if reads.get() == 1 {
                MonoTime(0)
            } else {
                deadline
            })
        },
    );
    assert!(matches!(error, Err(SessionError::Timeout)));
    assert_eq!(session.phase(), ActorPhase::Standby);
    assert!(trace.borrow().calls.is_empty());
    session.shutdown();
}
#[test]
fn expired_poll_shuts_down_and_preserves_timeout() {
    let (mut session, trace) = install();
    session.deadline = MonoTime(0);
    assert!(matches!(session.poll(), Err(SessionError::Timeout)));
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
    assert_eq!(test_send(0x1e, false), 9);
    assert!(trace.borrow().calls.is_empty());
}
#[test]
fn probe_failure_closes_hook_before_any_capture() {
    let (mut session, trace) = install();
    test_probe(0, false);
    assert!(matches!(
        session.poll(),
        Err(SessionError::Probe(ProbeError::InactiveDesktop))
    ));
    assert_eq!(test_send(0x1e, false), 9);
    assert!(session.shutdown_report().unwrap().unhook.is_ok());
    assert!(trace.borrow().calls.is_empty());
}
#[test]
fn unhook_failure_still_disables_dispatch_and_recovers_accepted_down() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(test_send(0x1e, false), 1);
    test_fail_unhook();
    let report = session.shutdown();
    assert!(matches!(report.unhook, Err(HookSessionError::Unhook(5))));
    assert!(report.recovery.is_ok());
    assert_eq!(trace.borrow().calls.len(), 2);
    assert_eq!(test_send(0x1e, true), 9);
}
#[test]
fn unknown_receipt_quarantines_and_is_never_falsely_cleared() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    trace.borrow_mut().unknown = true;
    assert_eq!(test_send(0x1e, false), 9);
    assert!(session.poll().is_err());
    assert!(matches!(
        session.shutdown_report().unwrap().recovery,
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(session.actor.borrow().output().phase(), Phase::Quarantined);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn known_zero_recovery_has_eight_attempt_bound_and_retains_submitted() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    trace.borrow_mut().zero_recovery = true;
    assert!(matches!(
        session.shutdown().recovery,
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(trace.borrow().calls.len(), 9);
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
}
#[test]
fn active_poll_avoids_system_sampling_and_quit_performs_recovery() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    test_probe(0, false); // active polling must not use startup probe
    assert_eq!(session.poll().unwrap(), SessionStep::Continue);
    test_quit();
    assert_eq!(session.poll().unwrap(), SessionStep::Quit);
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
    assert_eq!(trace.borrow().calls.len(), 2);
}
#[test]
fn drop_only_disables_hook_without_claiming_output_cleanup() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    drop(session);
    assert_eq!(test_send(0x1e, true), 9);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn configuration_switch_keeps_old_down_binding_and_only_changes_new_down() {
    use keyweave_engine::{Mapping, MappingExtras};
    use keyweave_types::{KEYBOARD_PAGE, KeyId};
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    let mut updated = (*config()).clone();
    updated.generation = ConfigGeneration(2);
    updated.simple.push(Mapping {
        id: "a-c".into(),
        from: KeyId::new(KEYBOARD_PAGE, 4),
        to: vec![KeyId::new(KEYBOARD_PAGE, 6).into()],
        allow_extra_modifiers: true,
        extras: MappingExtras::default(),
    });
    session.activate_config(Arc::new(updated)).unwrap();
    assert_eq!(test_send(0x1e, true), 1);
    assert_eq!(test_send(0x1e, false), 1);
    let trace = trace.borrow();
    assert_eq!(trace.calls[0][0].scan, 0x1e);
    assert_eq!(trace.calls[1][0].scan, 0x1e);
    assert_eq!(trace.calls[1][0].flags & 2, 2);
    assert_eq!(trace.calls[2][0].scan, 0x2e);
    drop(trace);
    session.shutdown();
    assert!(matches!(
        session.activate_config(config()),
        Err(SessionError::Closed)
    ));
}
#[test]
fn unsupported_config_fails_before_probe_or_hook_registration() {
    use keyweave_engine::{Mapping, MappingExtras};
    use keyweave_types::{CONSUMER_PAGE, KEYBOARD_PAGE, KeyId};
    test_probe(0, false);
    let trace = Rc::new(RefCell::new(Trace::default()));
    let mut invalid = (*config()).clone();
    invalid.simple.push(Mapping {
        id: "unsupported".into(),
        from: KeyId::new(KEYBOARD_PAGE, 4),
        to: vec![KeyId::new(CONSUMER_PAGE, 0xcd).into()],
        allow_extra_modifiers: true,
        extras: MappingExtras::default(),
    });
    let result = CaptureSession::install(
        Backend(trace.clone()),
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
        Arc::new(invalid),
    );
    assert!(matches!(
        result,
        Err(SessionError::Actor(RuntimeError::Key(_)))
    ));
    assert!(trace.borrow().calls.is_empty());
    test_probe(0, true);
    let (mut valid, _) = install(); // no TLS registration was consumed
    valid.shutdown();
}
#[test]
fn initial_probe_error_never_registers_hook_or_mutates_output() {
    test_probe(0, false);
    let trace = Rc::new(RefCell::new(Trace::default()));
    let result = CaptureSession::install(
        Backend(trace.clone()),
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
        config(),
    );
    assert!(matches!(
        result,
        Err(SessionError::Probe(ProbeError::InactiveDesktop))
    ));
    assert!(trace.borrow().calls.is_empty());
    test_probe(0, true);
    let (mut valid, _) = install();
    valid.shutdown();
}
// Deterministic callback/control delivery without sleeping. Win32 remains the
// replacement API; test fixture assumes neutral and starts the actor explicitly.
fn timed_session(
    config: Arc<ConfigSnapshot>,
) -> (
    CaptureSession<Backend>,
    Rc<RefCell<Trace>>,
    Rc<Cell<MonoTime>>,
) {
    let trace = Rc::new(RefCell::new(Trace::default()));
    let actor = Rc::new(RefCell::new(
        SessionActor::new(
            Backend(trace.clone()),
            InjectionTag::new(1).unwrap(),
            config,
        )
        .unwrap(),
    ));
    actor.borrow_mut().start_after_neutral().unwrap();
    let emergency = Rc::new(RefCell::new(EmergencyChord::new()));
    let enabled = Rc::new(Cell::new(true));
    let now = Rc::new(Cell::new(MonoTime(0)));
    let handler_time = now.clone();
    let handler = SessionHandler {
        actor: actor.clone(),
        emergency: emergency.clone(),
        enabled: enabled.clone(),
        clock: move || Some(handler_time.get()),
    };
    let hook = HookSession::install(
        Box::new(handler),
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
    )
    .unwrap_or_else(|(error, _)| panic!("{error}"));
    (
        CaptureSession {
            actor,
            emergency,
            enabled,
            hook,
            clock: SessionClock::new(),
            deadline: MonoTime(STARTUP_NS),
            shutdown: None,
        },
        trace,
        now,
    )
}
fn press_emergency() {
    for scan in [0x1d, 0x38, 0x2a, 0x58] {
        assert_eq!(test_send(scan, false), 1);
    }
}
#[test]
fn emergency_idle_control_wakes_before_actor_timer_and_stops_native_hook() {
    use keyweave_engine::{Mapping, MappingExtras};
    use keyweave_types::{KEYBOARD_PAGE, KeyId};
    let mut config = (*config()).clone();
    config.complex.push(Mapping {
        id: "held-f12".into(),
        from: KeyId::new(KEYBOARD_PAGE, 0x45),
        to: vec![KeyId::new(KEYBOARD_PAGE, 4).into()],
        allow_extra_modifiers: true,
        extras: MappingExtras {
            if_held_down: vec![KeyId::new(KEYBOARD_PAGE, 5).into()],
            held_threshold_ns: crate::emergency::HOLD_NS,
            ..MappingExtras::default()
        },
    });
    let (mut session, trace, now) = timed_session(Arc::new(config));
    press_emergency();
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let at = crate::emergency::HOLD_NS;
    assert_eq!(
        active_wait(&actor, &emergency, Some(MonoTime(at - 1)))
            .unwrap()
            .timeout_ms(),
        1
    );
    now.set(MonoTime(at));
    let result = session
        .hook
        .poll_with_control(&mut || active_wait(&actor, &emergency, Some(now.get())));
    assert_eq!(
        result,
        Err(HookSessionError::Callback(HookFault::Emergency))
    );
    assert_eq!(test_send(0x1e, false), 9); // native disabled already
    assert_eq!(session.poll().unwrap(), SessionStep::Emergency);
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert!(
        !trace
            .borrow()
            .calls
            .iter()
            .flatten()
            .any(|packet| packet.scan == 0x30)
    ); // held B timer never delivered
    let cleanup = trace.borrow().calls.last().unwrap().clone();
    assert_eq!(cleanup[0].scan, 0x1e); // ordinary key before modifier releases
    assert!(cleanup.iter().all(|packet| packet.flags & 2 != 0));
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        0
    );
}
#[test]
fn exact_deadline_physical_up_cannot_cancel_emergency_or_reach_actor() {
    let (mut session, trace, now) = timed_session(config());
    press_emergency();
    let before = trace.borrow().calls.len();
    now.set(MonoTime(crate::emergency::HOLD_NS));
    assert_eq!(test_send(0x58, true), 9);
    assert_eq!(session.hook.fault(), Some(HookFault::Emergency));
    assert_eq!(trace.borrow().calls.len(), before); // no engine up before stop
    assert_eq!(session.poll().unwrap(), SessionStep::Emergency);
    assert_eq!(trace.borrow().calls.len(), before + 1);
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
}
#[test]
fn early_physical_release_cancels_emergency_and_is_captured_normally() {
    let (mut session, _, now) = timed_session(config());
    press_emergency();
    now.set(MonoTime(crate::emergency::HOLD_NS - 1));
    assert_eq!(test_send(0x58, true), 1);
    assert_eq!(session.emergency.borrow().next_deadline(), None);
    assert!(
        active_wait(
            &session.actor,
            &session.emergency,
            Some(MonoTime(crate::emergency::HOLD_NS))
        )
        .is_ok()
    );
    assert_eq!(session.hook.fault(), None);
    session.shutdown();
}
#[test]
fn injected_chord_never_arms_or_enters_actor() {
    use super::super::hook_thread::test_record;
    use crate::keys::HookRecord;
    let (mut session, trace, _) = timed_session(config());
    for flags in [0x10, 0x12] {
        for extra_info in [0, 1] {
            for scan in [0x1d, 0x38, 0x2a, 0x58] {
                assert_eq!(
                    test_record(HookRecord {
                        message: 0x100,
                        vk_code: 0x41,
                        scan_code: scan,
                        flags,
                        extra_info
                    }),
                    9
                );
            }
        }
    }
    assert_eq!(session.emergency.borrow().next_deadline(), None);
    assert_eq!(session.hook.held_keys().unwrap(), 0);
    assert!(trace.borrow().calls.is_empty());
    session.shutdown();
}
#[test]
fn remapping_emergency_keys_does_not_change_physical_detector() {
    use keyweave_engine::{Mapping, MappingExtras};
    use keyweave_types::{KEYBOARD_PAGE, KeyId};
    let mut config = (*config()).clone();
    for usage in [0xe0, 0xe2, 0xe1, 0x45] {
        config.simple.push(Mapping {
            id: format!("map-{usage}"),
            from: KeyId::new(KEYBOARD_PAGE, usage),
            to: vec![KeyId::new(KEYBOARD_PAGE, 4).into()],
            allow_extra_modifiers: true,
            extras: MappingExtras::default(),
        });
    }
    let (mut session, trace, now) = timed_session(Arc::new(config));
    press_emergency();
    assert_eq!(
        session.emergency.borrow().next_deadline(),
        Some(MonoTime(crate::emergency::HOLD_NS))
    );
    assert!(
        trace
            .borrow()
            .calls
            .iter()
            .flatten()
            .all(|packet| packet.scan == 0x1e)
    );
    now.set(MonoTime(crate::emergency::HOLD_NS));
    assert_eq!(test_send(0x58, false), 9); // repeat doesn't rearm, due wins
    assert_eq!(session.poll().unwrap(), SessionStep::Emergency);
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
}
#[test]
fn emergency_detector_clock_error_stops_and_reports_error_not_success() {
    let (mut session, trace, now) = timed_session(config());
    now.set(MonoTime(10));
    assert_eq!(test_send(0x1d, false), 1);
    now.set(MonoTime(9));
    assert_eq!(test_send(0x38, false), 9);
    assert!(matches!(
        session.poll(),
        Err(SessionError::EmergencyDetector(
            EmergencyError::ClockBackwards
        ))
    ));
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
    assert_eq!(trace.borrow().calls.len(), 2);
}
#[test]
fn emergency_preserves_unhook_error_and_bounded_recovery_failure() {
    let (mut session, trace, now) = timed_session(config());
    press_emergency();
    trace.borrow_mut().zero_recovery = true;
    test_fail_unhook();
    now.set(MonoTime(crate::emergency::HOLD_NS));
    assert_eq!(test_send(0x58, true), 9);
    assert_eq!(session.poll().unwrap(), SessionStep::Emergency);
    let report = session.shutdown_report().unwrap();
    assert!(matches!(report.unhook, Err(HookSessionError::Unhook(5))));
    assert!(matches!(report.recovery, Err(RuntimeError::Unresolved)));
    assert_eq!(trace.borrow().calls.len(), 12); // four accepted down, eight cleanup attempts
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        4
    );
    assert_eq!(session.phase(), ActorPhase::Unresolved);
}
#[test]
fn passive_chord_never_arms_and_neutral_activation_starts_with_empty_detector() {
    let (mut session, trace) = install();
    for scan in [0x1d, 0x38, 0x2a, 0x58] {
        assert_eq!(test_send(scan, false), 9);
    }
    assert_eq!(session.poll().unwrap(), SessionStep::Waiting);
    assert_eq!(session.emergency.borrow().next_deadline(), None);
    for scan in [0x1d, 0x38, 0x2a, 0x58] {
        assert_eq!(test_send(scan, true), 9);
    }
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert_eq!(session.emergency.borrow().next_deadline(), None);
    assert!(trace.borrow().calls.is_empty());
    session.shutdown();
}
#[test]
fn active_control_clock_failure_preserves_actor_reason_and_disables_native_hook() {
    let (mut session, trace, _) = timed_session(config());
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    assert_eq!(
        session
            .hook
            .poll_with_control(&mut || active_wait(&actor, &emergency, None)),
        Err(HookSessionError::Callback(HookFault::Handler))
    );
    assert!(matches!(
        actor.borrow().failure(),
        Some(RuntimeError::Clock)
    ));
    assert_eq!(test_send(0x1e, false), 9);
    assert!(session.poll().is_err());
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
    assert!(trace.borrow().calls.is_empty());
}
