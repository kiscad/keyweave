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
    inject_during_submit: Option<crate::keys::HookRecord>,
    notify_during_submit: Option<(usize, isize)>,
    revoke_during_submit: bool,
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
        if std::mem::take(&mut trace.revoke_during_submit) {
            super::super::hook_thread::revoke_access();
        }
        if let Some((event, id)) = trace.notify_during_submit.take() {
            super::super::session_notifications::api::send(event, id);
        }
        if let Some(record) = trace.inject_during_submit.take() {
            assert_eq!(super::super::hook_thread::test_record(record), 9);
        }
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
fn application_config() -> Arc<ConfigSnapshot> {
    let mut cfg = (*config()).clone();
    cfg.generation = ConfigGeneration(2);
    cfg.complex.push(keyweave_engine::Mapping {
        id: "app".into(),
        from: keyweave_types::KeyId::new(7, 4),
        to: vec![keyweave_types::KeyId::new(7, 5).into()],
        allow_extra_modifiers: true,
        extras: keyweave_engine::MappingExtras {
            conditions: vec![keyweave_engine::Condition::Application {
                bundle_identifiers: vec![],
                file_paths: vec![keyweave_engine::Pattern::compile("editor[.]exe$").unwrap()],
                unless: false,
            }],
            ..Default::default()
        },
    });
    Arc::new(cfg)
}
#[test]
fn active_poll_delivers_native_application_and_failed_query_clears_it() {
    super::super::application::test_sample_mode(true, true);
    let (mut session, trace) = install();
    session.activate_config(application_config()).unwrap();
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert_eq!(session.poll().unwrap(), SessionStep::Continue);
    assert_eq!(session.application.revision, 1);
    assert_eq!(session.progress_epoch(), 2); // Context doesn't mint a new epoch.
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(trace.borrow().calls[0][0].scan, 0x30);
    super::super::application::test_sample_mode(false, true);
    session.application.next = None; // Force the refresh boundary without sleep.
    assert_eq!(session.poll().unwrap(), SessionStep::Continue);
    assert_eq!(session.application.revision, 2);
    assert_eq!(test_send(0x1e, true), 1);
    assert_eq!(trace.borrow().calls[1][0].scan, 0x30); // Saved old binding.
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(trace.borrow().calls[2][0].scan, 0x1e); // Fresh Unknown.
    session.shutdown();
}
#[test]
fn application_close_failure_stops_without_claiming_revoked_input_access() {
    super::super::application::test_sample_mode(false, false);
    let (mut session, trace) = install();
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert_eq!(test_send(0x1e, false), 1);
    assert!(matches!(
        session.poll(),
        Err(SessionError::Hook(HookSessionError::Callback(
            HookFault::Handler
        )))
    ));
    let report = session.shutdown_report().unwrap();
    assert_eq!(
        report.application_provider,
        Some(ProbeError::Api {
            call: "CloseHandle(application)",
            code: 5
        })
    );
    assert!(!report.access_lost);
    assert!(report.unhook.is_ok() && report.recovery.is_ok());
    assert_eq!(trace.borrow().calls.len(), 2);
    assert!(matches!(
        session.actor.borrow().failure(),
        Some(RuntimeError::ApplicationProvider(_))
    ));
}
#[test]
fn application_refresh_is_throttled_and_query_time_does_not_extend_validity() {
    let (mut session, trace, now) = timed_session(application_config());
    let mut times = std::collections::VecDeque::from([MonoTime(0), MonoTime(100_000_000)]);
    refresh_application(
        &mut session.application,
        &session.actor,
        &session.emergency,
        &session.access,
        || times.pop_front(),
        || {
            assert_eq!(test_send(0x1e, false), 1); // Reentrant input may borrow actor.
            assert_eq!(test_send(0x1e, true), 1);
            Ok(Some(keyweave_types::ApplicationContext {
                bundle_identifier: None,
                file_path: Some("editor.exe".into()),
            }))
        },
    )
    .unwrap();
    assert!(
        trace
            .borrow()
            .calls
            .iter()
            .flatten()
            .all(|p| p.scan == 0x1e)
    );
    refresh_application(
        &mut session.application,
        &session.actor,
        &session.emergency,
        &session.access,
        || Some(MonoTime(150_000_000)),
        || panic!("early metadata query"),
    )
    .unwrap();
    assert_eq!(session.application.revision, 1);
    // Access is independently renewed; app metadata retains its original expiry.
    session
        .access
        .borrow_mut()
        .renew_after_probe(MonoTime(400_000_000))
        .unwrap();
    now.set(MonoTime(500_000_000));
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(trace.borrow().calls.last().unwrap()[0].scan, 0x1e);
    session.shutdown();
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
    let enabled = Rc::new(Cell::new(true));
    let actor = Rc::new(RefCell::new(
        SessionActor::new(
            GuardedBackend {
                backend: Backend(trace.clone()),
                enabled: enabled.clone(),
            },
            InjectionTag::new(1).unwrap(),
            config,
        )
        .unwrap(),
    ));
    actor.borrow_mut().start_after_neutral().unwrap();
    let emergency = Rc::new(RefCell::new(EmergencyChord::new()));
    let access = Rc::new(RefCell::new(AccessLease::new()));
    access.borrow_mut().renew_after_probe(MonoTime(0)).unwrap();
    let now = Rc::new(Cell::new(MonoTime(0)));
    let handler_time = now.clone();
    let handler = SessionHandler {
        actor: actor.clone(),
        emergency: emergency.clone(),
        access: access.clone(),
        enabled: enabled.clone(),
        clock: move || Some(handler_time.get()),
    };
    let mut hook = HookSession::install(
        Box::new(handler),
        InjectionTag::new(1).unwrap(),
        IdentityPolicy::CanonicalScan,
    )
    .unwrap_or_else(|(error, _)| panic!("{error}"));
    hook.guard_untracked_input().unwrap();
    (
        CaptureSession {
            application: ApplicationRefresh::default(),
            session: own_session().unwrap(),
            progress_epoch: 0,
            notifications: SessionNotifications::install().unwrap(),
            actor,
            emergency,
            access,
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
    let access = session.access.clone();
    let at = crate::emergency::HOLD_NS;
    keep_access_valid(&session);
    assert_eq!(
        active_wait(&actor, &emergency, &access, Some(MonoTime(at - 1)))
            .unwrap()
            .timeout_ms(),
        1
    );
    now.set(MonoTime(at));
    let result = session.hook.poll_with_control(&mut || {
        monitored_wait(
            &actor,
            &emergency,
            &access,
            || Some(now.get()),
            || panic!("due emergency must precede metadata query"),
        )
    });
    assert_eq!(
        result,
        Err(HookSessionError::Callback(HookFault::Emergency))
    );
    assert!(matches!(
        session.hook.held_keys(),
        Err(HookSessionError::Callback(HookFault::Emergency))
    ));
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
    keep_access_valid(&session);
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
    keep_access_valid(&session);
    now.set(MonoTime(crate::emergency::HOLD_NS - 1));
    assert_eq!(test_send(0x58, true), 1);
    assert_eq!(session.emergency.borrow().next_deadline(), None);
    assert!(
        active_wait(
            &session.actor,
            &session.emergency,
            &session.access,
            Some(MonoTime(crate::emergency::HOLD_NS))
        )
        .is_ok()
    );
    assert_eq!(session.hook.fault(), None);
    session.shutdown();
}
#[test]
fn own_injected_chord_never_arms_or_enters_actor() {
    use super::super::hook_thread::test_record;
    use crate::keys::HookRecord;
    let (mut session, trace, _) = timed_session(config());
    for flags in [0x10, 0x12] {
        for extra_info in [1] {
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
    keep_access_valid(&session);
    now.set(MonoTime(crate::emergency::HOLD_NS));
    assert_eq!(test_send(0x58, false), 9); // repeat doesn't rearm, due wins
    assert_eq!(session.poll().unwrap(), SessionStep::Emergency);
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert!(matches!(
        session.shutdown_report().unwrap().recovery,
        Err(RuntimeError::Unresolved)
    ));
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
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert!(matches!(
        session.shutdown_report().unwrap().recovery,
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(trace.borrow().calls.len(), 1); // no blind up after forwarded Alt down
}
#[test]
fn emergency_preserves_unhook_error_and_bounded_recovery_failure() {
    let (mut session, trace, now) = timed_session(config());
    press_emergency();
    trace.borrow_mut().zero_recovery = true;
    test_fail_unhook();
    keep_access_valid(&session);
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
    let access = session.access.clone();
    assert_eq!(
        session
            .hook
            .poll_with_control(&mut || active_wait(&actor, &emergency, &access, None)),
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

// These earlier successful metadata proofs keep the emergency-focused fixture's
// lease valid; dedicated lease tests below exercise real monitored_wait renewal.
fn keep_access_valid(session: &CaptureSession<Backend>) {
    for now in [300_000_000, 600_000_000, 900_000_000] {
        session
            .access
            .borrow_mut()
            .renew_after_probe(MonoTime(now))
            .unwrap();
    }
}
fn monitored_step(
    session: &mut CaptureSession<Backend>,
    now: MonoTime,
) -> Result<LoopStep, HookSessionError> {
    let identity = session.session;
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    session.hook.poll_with_control(&mut || {
        monitored_wait(
            &actor,
            &emergency,
            &access,
            || Some(now),
            || check_input_access_in_session(identity),
        )
    })
}
#[test]
fn metadata_refresh_preserves_held_binding_and_never_queries_keys() {
    use super::super::input_probe::test_access_counts;
    let (mut session, trace, now) = timed_session(config());
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(
        monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS - 1)).unwrap(),
        LoopStep::Continue
    );
    assert_eq!(test_access_counts(), (0, 0));
    assert_eq!(
        monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)).unwrap(),
        LoopStep::Continue
    );
    assert_eq!(test_access_counts(), (0, 2));
    assert_eq!(session.hook.held_keys().unwrap(), 1);
    now.set(MonoTime(crate::access::REFRESH_NS + 1));
    assert_eq!(test_send(0x1e, true), 1);
    assert_eq!(trace.borrow().calls.len(), 2);
    session.shutdown();
}
#[test]
fn refreshed_lease_deadline_participates_in_message_wait() {
    let (mut session, _, _) = timed_session(config());
    assert_eq!(
        monitored_wait(
            &session.actor,
            &session.emergency,
            &session.access,
            || Some(MonoTime(crate::access::REFRESH_NS - 1)),
            || panic!("early probe")
        )
        .unwrap()
        .timeout_ms(),
        1
    );
    assert_eq!(
        monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)).unwrap(),
        LoopStep::Continue
    );
    assert_eq!(
        session.access.borrow().next_refresh(),
        Some(MonoTime(2 * crate::access::REFRESH_NS))
    );
    session.shutdown();
}
#[test]
fn higher_integrity_stops_and_preserves_accepted_output_without_delivery() {
    let (mut session, trace, _) = timed_session(config());
    test_send(0x1e, false);
    super::super::input_probe::test_elevated();
    assert_eq!(
        monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)),
        Err(HookSessionError::Callback(HookFault::InputAccess(
            ProbeError::HigherIntegrity
        )))
    );
    assert!(matches!(
        session.poll(),
        Err(SessionError::Probe(ProbeError::HigherIntegrity))
    ));
    assert!(matches!(
        session.shutdown_report().unwrap().recovery,
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_eq!(test_send(0x1e, false), 9);
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn active_desktop_loss_is_reported_without_new_key_state_observation() {
    let (mut session, trace, _) = timed_session(config());
    test_send(0x1e, false);
    test_probe(0, false);
    assert!(monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)).is_err());
    assert!(matches!(
        session.poll(),
        Err(SessionError::Probe(ProbeError::InactiveDesktop))
    ));
    assert!(matches!(
        session.shutdown_report().unwrap().recovery,
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(super::super::input_probe::test_access_counts().0, 0);
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn expired_callback_stops_before_actor_and_late_control_does_not_probe() {
    let (mut session, trace, now) = timed_session(config());
    now.set(MonoTime(crate::access::LEASE_NS));
    assert_eq!(test_send(0x1e, false), 9);
    assert_eq!(
        session
            .poll()
            .err()
            .map(|error| matches!(error, SessionError::AccessLease(AccessError::Expired))),
        Some(true)
    );
    assert!(trace.borrow().calls.is_empty());
    assert_eq!(super::super::input_probe::test_access_counts(), (0, 0));
}
#[test]
fn late_successful_probe_cannot_renew_or_tick_actor() {
    let (mut session, trace, _) = timed_session(config());
    let before = session.access.borrow().clone();
    let times = RefCell::new(std::collections::VecDeque::from([
        MonoTime(crate::access::REFRESH_NS),
        MonoTime(crate::access::LEASE_NS),
    ]));
    let result = monitored_wait(
        &session.actor,
        &session.emergency,
        &session.access,
        || times.borrow_mut().pop_front(),
        || Ok(()),
    );
    assert_eq!(result, Err(HookFault::AccessLease(AccessError::Expired)));
    assert_eq!(*session.access.borrow(), before);
    assert!(trace.borrow().calls.is_empty());
    session.shutdown();
}
#[test]
fn metadata_callback_fault_wins_over_probe_result_and_blocks_tick_and_wait() {
    use super::super::hook_thread::{test_record, test_wait_calls};
    use crate::keys::HookRecord;
    let (mut session, trace, _) = timed_session(config());
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    let before = access.borrow().clone();
    let result = session.hook.poll_with_control(&mut || {
        monitored_wait(
            &actor,
            &emergency,
            &access,
            || Some(MonoTime(crate::access::REFRESH_NS)),
            || {
                assert_eq!(
                    test_record(HookRecord {
                        message: 0x100,
                        vk_code: 0x41,
                        scan_code: 0,
                        flags: 0,
                        extra_info: 0
                    }),
                    9
                );
                Err(ProbeError::HigherIntegrity)
            },
        )
    });
    assert_eq!(result, Err(HookSessionError::Callback(HookFault::Record)));
    assert_eq!(*access.borrow(), before);
    assert_eq!(test_wait_calls(), 0);
    assert!(trace.borrow().calls.is_empty());
    assert!(session.poll().is_err());
}
#[test]
fn native_control_success_cannot_resume_after_callback_fault() {
    use super::super::hook_thread::{test_record, test_wait_calls};
    use crate::keys::HookRecord;
    let (mut session, _, _) = timed_session(config());
    let result = session.hook.poll_with_control(&mut || {
        test_record(HookRecord {
            message: 0x100,
            vk_code: 0x41,
            scan_code: 0,
            flags: 0,
            extra_info: 0,
        });
        Ok(WaitPlan::for_deadline(MonoTime(0), None))
    });
    assert_eq!(result, Err(HookSessionError::Callback(HookFault::Record)));
    assert_eq!(test_wait_calls(), 0);
    assert!(session.poll().is_err());
}
#[test]
fn backwards_probe_completion_does_not_grant_a_new_lease() {
    let (mut session, _, _) = timed_session(config());
    let before = session.access.borrow().clone();
    let times = RefCell::new(std::collections::VecDeque::from([
        MonoTime(crate::access::REFRESH_NS),
        MonoTime(crate::access::REFRESH_NS - 1),
    ]));
    assert_eq!(
        monitored_wait(
            &session.actor,
            &session.emergency,
            &session.access,
            || times.borrow_mut().pop_front(),
            || Ok(())
        ),
        Err(HookFault::AccessLease(AccessError::ClockBackwards))
    );
    assert_eq!(*session.access.borrow(), before);
    session.shutdown();
}
#[test]
fn healthy_sent_callback_during_probe_can_borrow_actor_and_preserves_its_binding() {
    let (mut session, trace, now) = timed_session(config());
    now.set(MonoTime(crate::access::REFRESH_NS));
    let result = monitored_wait(
        &session.actor,
        &session.emergency,
        &session.access,
        || Some(now.get()),
        || {
            assert_eq!(test_send(0x1e, false), 1);
            check_input_access()
        },
    );
    assert!(result.is_ok());
    assert_eq!(session.hook.held_keys().unwrap(), 1);
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_eq!(
        session.access.borrow().next_refresh(),
        Some(MonoTime(2 * crate::access::REFRESH_NS))
    );
    session.shutdown();
    assert_eq!(trace.borrow().calls.len(), 2);
}
#[test]
fn callback_expiry_during_probe_prevents_successful_result_from_renewing() {
    let (mut session, trace, now) = timed_session(config());
    let before = session.access.borrow().clone();
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    let clock_reads = Cell::new(0);
    let result = session.hook.poll_with_control(&mut || {
        monitored_wait(
            &actor,
            &emergency,
            &access,
            || {
                clock_reads.set(clock_reads.get() + 1);
                Some(MonoTime(crate::access::REFRESH_NS))
            },
            || {
                now.set(MonoTime(crate::access::LEASE_NS));
                assert_eq!(test_send(0x1e, false), 9);
                Ok(())
            },
        )
    });
    assert_eq!(
        result,
        Err(HookSessionError::Callback(HookFault::AccessLease(
            AccessError::Expired
        )))
    );
    assert_eq!(*access.borrow(), before);
    assert_eq!(clock_reads.get(), 1); // stopped before after-probe clock/tick
    assert!(trace.borrow().calls.is_empty());
    assert!(matches!(
        session.poll(),
        Err(SessionError::AccessLease(AccessError::Expired))
    ));
}

fn external_record() -> crate::keys::HookRecord {
    crate::keys::HookRecord {
        message: 0x100,
        vk_code: 0x42,
        scan_code: 0x30,
        flags: 0x10,
        extra_info: 42,
    }
}
#[test]
fn unsupported_active_input_stops_cleanly_when_no_output_was_submitted() {
    let (mut session, trace) = install();
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert_eq!(test_send(0x45, false), 9);
    assert!(matches!(
        session.poll(),
        Err(SessionError::Hook(HookSessionError::Callback(
            HookFault::UntrackedInput
        )))
    ));
    let report = session.shutdown_report().unwrap();
    assert!(report.untracked_input && report.unhook.is_ok() && report.recovery.is_ok());
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert!(trace.borrow().calls.is_empty());
}
#[test]
fn external_input_while_output_is_held_never_releases_possibly_shared_down() {
    let (mut session, trace) = install();
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(super::super::hook_thread::test_record(external_record()), 9);
    assert!(session.poll().is_err());
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert!(matches!(
        session.shutdown_report().unwrap().recovery,
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(session.actor.borrow().output().phase(), Phase::Quarantined);
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
    assert!(session.shutdown().untracked_input);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn unsupported_physical_pass_with_owned_output_quarantines_without_heuristic_aliases() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(test_send(0x1d, false), 1);
    assert_eq!(test_send(0x45, false), 9);
    assert_eq!(session.hook.fault(), Some(HookFault::UntrackedInput));
    session.shutdown();
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn own_injected_input_bypasses_even_during_dispatcher_borrow_and_does_not_poison_recovery() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(test_send(0x1e, false), 1);
    let mut own = external_record();
    own.extra_info = 1;
    assert_eq!(
        super::super::hook_thread::with_callback_borrow(|| super::super::hook_thread::test_record(
            own
        )),
        9
    );
    assert_eq!(session.hook.fault(), None);
    assert!(!session.hook.untracked_input());
    let report = session.shutdown();
    assert!(!report.untracked_input && report.recovery.is_ok());
    assert_eq!(trace.borrow().calls.len(), 2);
}
#[test]
fn physical_marker_without_injected_flag_is_still_captured() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    let mut physical = external_record();
    physical.extra_info = 1;
    physical.flags = 0;
    assert_eq!(super::super::hook_thread::test_record(physical), 1);
    assert!(!session.hook.untracked_input());
    assert_eq!(trace.borrow().calls.len(), 1);
    assert!(session.shutdown().recovery.is_ok());
}
#[test]
fn foreign_unicode_and_lower_integrity_injections_cannot_enter_engine_or_emergency() {
    // One permanently stopped session per test thread; subsequent records pass
    // while the sticky flag and first error remain available until shutdown.
    let (mut session, trace) = install();
    session.poll().unwrap();
    let mut foreign = external_record();
    foreign.vk_code = 0xe7;
    foreign.scan_code = 0;
    foreign.flags = 0x12;
    assert_eq!(super::super::hook_thread::test_record(foreign), 9);
    assert_eq!(test_send(0x58, false), 9);
    assert!(session.emergency.borrow().next_deadline().is_none());
    assert!(trace.borrow().calls.is_empty());
    assert!(session.shutdown().untracked_input);
    assert_eq!(session.phase(), ActorPhase::Stopped);
}
#[test]
fn external_input_after_another_fault_is_sticky_and_preserves_primary_error() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    assert_eq!(
        session
            .hook
            .poll_with_control(&mut || Err(HookFault::Handler)),
        Err(HookSessionError::Callback(HookFault::Handler))
    );
    assert_eq!(test_send(0x30, false), 9);
    assert_eq!(session.hook.fault(), Some(HookFault::Handler));
    assert!(session.hook.untracked_input());
    assert!(matches!(
        session.poll(),
        Err(SessionError::Hook(HookSessionError::Callback(
            HookFault::Handler
        )))
    ));
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn external_reentry_during_submission_confirms_prior_receipt_but_rejects_next_batch() {
    use keyweave_engine::{Mapping, MappingExtras};
    use keyweave_types::{KEYBOARD_PAGE, KeyId};
    let mut cfg = (*config()).clone();
    cfg.simple.push(Mapping {
        id: "long".into(),
        from: KeyId::new(KEYBOARD_PAGE, 4),
        to: (0..200)
            .map(|i| KeyId::new(KEYBOARD_PAGE, 5 + (i % 2)).into())
            .collect(),
        allow_extra_modifiers: true,
        extras: MappingExtras::default(),
    });
    let (mut session, trace, _) = timed_session(Arc::new(cfg));
    assert_eq!(test_send(0x1d, false), 1);
    trace.borrow_mut().inject_during_submit = Some(external_record());
    assert_eq!(test_send(0x1e, false), 9);
    assert_eq!(trace.borrow().calls.len(), 2);
    assert_eq!(trace.borrow().calls[1].len(), 256);
    assert_eq!(session.hook.fault(), Some(HookFault::UntrackedInput));
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
    assert!(session.poll().is_err());
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert!(session.actor.borrow().next_wakeup().is_none());
    assert_eq!(trace.borrow().calls.len(), 2); // no next native batch or cleanup up
}

#[test]
fn passive_foreign_full_tap_does_not_activate_guard_or_mutate_engine() {
    let (mut session, trace) = install();
    let mut foreign = external_record();
    assert_eq!(super::super::hook_thread::test_record(foreign), 9);
    foreign.message = 0x101;
    foreign.flags |= 0x80;
    assert_eq!(super::super::hook_thread::test_record(foreign), 9);
    assert_eq!(session.hook.fault(), None);
    assert!(!session.hook.untracked_input());
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert!(trace.borrow().calls.is_empty());
    assert!(!session.shutdown().untracked_input);
}
#[test]
fn malformed_forwarded_input_preserves_record_fault_and_sets_recovery_risk() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    let mut invalid = external_record();
    invalid.flags = 0;
    invalid.scan_code = 0;
    assert_eq!(super::super::hook_thread::test_record(invalid), 9);
    assert_eq!(session.hook.fault(), Some(HookFault::Record));
    assert!(session.hook.untracked_input());
    assert!(session.poll().is_err());
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn physical_reentry_sets_separate_risk_without_borrowing_dispatcher_twice() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    assert_eq!(
        super::super::hook_thread::with_callback_borrow(|| test_send(0x30, false)),
        9
    );
    assert_eq!(session.hook.fault(), Some(HookFault::Reentrant));
    assert!(session.hook.untracked_input());
    assert!(session.poll().is_err());
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn untracked_input_keeps_unhook_error_and_does_not_attempt_any_cleanup() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    super::super::hook_thread::test_record(external_record());
    test_fail_unhook();
    let report = session.shutdown();
    assert!(report.untracked_input);
    assert!(matches!(report.unhook, Err(HookSessionError::Unhook(5))));
    assert!(matches!(report.recovery, Err(RuntimeError::Unresolved)));
    assert_eq!(trace.borrow().calls.len(), 1);
}

#[test]
fn disconnected_wts_session_rejects_startup_before_hook_registration() {
    use super::super::session_info::api as wts;
    wts::reset(|m| m.state = 4);
    let trace = Rc::new(RefCell::new(Trace::default()));
    assert!(matches!(
        CaptureSession::install(
            Backend(trace.clone()),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::CanonicalScan,
            config()
        ),
        Err(SessionError::Probe(ProbeError::InactiveSession))
    ));
    assert!(trace.borrow().calls.is_empty());
    assert_eq!(wts::counts(), (1, 1, 0));
    wts::reset(|_| {});
    let (mut valid, _) = install();
    valid.shutdown(); // no registration was leaked
}
#[test]
fn changed_wts_baseline_cannot_activate_even_when_new_session_is_unlocked() {
    use super::super::session_info::api as wts;
    let (mut session, trace) = install();
    wts::MOCK.with(|m| m.borrow_mut().logon += 1);
    assert!(matches!(
        session.poll(),
        Err(SessionError::Probe(ProbeError::Changed))
    ));
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert!(trace.borrow().calls.is_empty());
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
}
#[test]
fn active_wts_disconnect_stops_before_tick_and_preserves_output_after_unhook() {
    use super::super::session_info::api as wts;
    // Test each terminal condition in its own native test thread (TLS is poisoned).
    let (mut session, trace, _) = timed_session(config());
    assert_eq!(test_send(0x1e, false), 1);
    wts::MOCK.with(|m| m.borrow_mut().state = 4);
    assert_eq!(
        monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)),
        Err(HookSessionError::Callback(HookFault::InputAccess(
            ProbeError::InactiveSession
        )))
    );
    assert!(matches!(
        session.poll(),
        Err(SessionError::Probe(ProbeError::InactiveSession))
    ));
    let report = session.shutdown_report().unwrap();
    assert!(!report.untracked_input && matches!(report.recovery, Err(RuntimeError::Unresolved)));
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);

    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn active_wts_logon_change_is_compared_to_initial_identity_on_refresh() {
    use super::super::session_info::api as wts;
    let (mut session, trace, _) = timed_session(config());
    test_send(0x1e, false);
    wts::MOCK.with(|m| m.borrow_mut().logon += 1);
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    let identity = session.session;
    assert_eq!(
        session.hook.poll_with_control(&mut || monitored_wait(
            &actor,
            &emergency,
            &access,
            || Some(MonoTime(crate::access::REFRESH_NS)),
            || check_input_access_in_session(identity)
        )),
        Err(HookSessionError::Callback(HookFault::InputAccess(
            ProbeError::Changed
        )))
    );
    assert!(matches!(
        session.poll(),
        Err(SessionError::Probe(ProbeError::Changed))
    ));
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn foreign_callback_in_wts_query_wins_and_retains_untracked_recovery_risk() {
    use super::super::session_info::api as wts;
    let (mut session, trace, _) = timed_session(config());
    test_send(0x1e, false);
    wts::MOCK.with(|m| m.borrow_mut().callback = Some(external_record()));
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    let identity = session.session;
    assert_eq!(
        session.hook.poll_with_control(&mut || monitored_wait(
            &actor,
            &emergency,
            &access,
            || Some(MonoTime(crate::access::REFRESH_NS)),
            || check_input_access_in_session(identity)
        )),
        Err(HookSessionError::Callback(HookFault::UntrackedInput))
    );
    assert!(session.poll().is_err());
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
    let (queries, freed, live) = wts::counts();
    assert_eq!(queries, freed);
    assert_eq!(live, 0);
}

#[test]
fn active_wts_lock_stops_without_scanning_keys_and_does_not_renew_lease() {
    use super::super::session_info::api as wts;
    let (mut session, trace, _) = timed_session(config());
    test_send(0x1e, false);
    let before = session.access.borrow().clone();
    wts::MOCK.with(|m| m.borrow_mut().flags = 0);
    assert_eq!(
        monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)),
        Err(HookSessionError::Callback(HookFault::InputAccess(
            ProbeError::LockedSession
        )))
    );
    assert_eq!(*session.access.borrow(), before);
    assert_eq!(super::super::input_probe::test_access_counts().0, 0);
    assert!(matches!(
        session.poll(),
        Err(SessionError::Probe(ProbeError::LockedSession))
    ));
    assert!(matches!(
        session.shutdown_report().unwrap().recovery,
        Err(RuntimeError::Unresolved)
    ));
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_access_loss_evidence(&session, &trace);
}

#[test]
fn passive_session_notification_stops_before_activation_and_closes_resources() {
    let (mut session, trace) = install();
    super::super::session_notifications::api::send(7, 1);
    assert_eq!(test_send(0x1e, false), 9);
    assert!(matches!(
        session.poll(),
        Err(SessionError::Hook(HookSessionError::Callback(
            HookFault::SessionChanged
        )))
    ));
    let report = session.shutdown_report().unwrap();
    assert!(report.session_changed && !report.untracked_input);
    assert_eq!(report.notifications, NotificationCleanup::default());
    assert!(report.recovery.is_ok() && report.unhook.is_ok());
    assert!(trace.borrow().calls.is_empty());
}
#[test]
fn own_notification_preserves_submitted_and_does_not_send_ups_to_changed_session() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(test_send(0x1e, false), 1);
    super::super::session_notifications::api::send(4, 1);
    assert_eq!(session.hook.fault(), Some(HookFault::SessionChanged));
    super::super::session_notifications::api::send(8, 1);
    assert!(session.poll().is_err());
    let report = session.shutdown_report().unwrap();
    assert!(report.session_changed && !report.untracked_input);
    assert!(matches!(report.recovery, Err(RuntimeError::Unresolved)));
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
    assert_eq!(session.actor.borrow().output().phase(), Phase::Quarantined);
}
#[test]
fn other_session_notification_keeps_original_lease_and_mapping() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    super::super::session_notifications::api::send(7, 2);
    assert!(!session.notifications.access_lost());
    assert_eq!(session.hook.fault(), None);
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(test_send(0x1e, true), 1);
    assert!(!session.shutdown().session_changed);
    assert_eq!(trace.borrow().calls.len(), 2);
}
#[test]
fn session_change_during_submit_is_borrow_free_and_preserves_real_receipt() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    trace.borrow_mut().notify_during_submit = Some((7, 1));
    assert_eq!(test_send(0x1e, false), 9);
    assert_eq!(session.hook.fault(), Some(HookFault::SessionChanged));
    assert!(session.notifications.access_lost());
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
    assert!(session.poll().is_err());
    assert!(session.shutdown_report().unwrap().session_changed);
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn session_latch_survives_earlier_fault_and_empty_ledger_stops_without_submission() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(super::super::hook_thread::test_record(external_record()), 9);
    super::super::session_notifications::api::send(6, 1);
    assert_eq!(session.hook.fault(), Some(HookFault::UntrackedInput));
    assert!(session.poll().is_err());
    let report = session.shutdown_report().unwrap();
    assert!(report.session_changed && report.untracked_input && report.recovery.is_ok());
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert!(trace.borrow().calls.is_empty());
}
#[test]
fn notification_cleanup_errors_are_reported_without_hiding_hook_or_recovery() {
    let (mut session, _) = install();
    session.poll().unwrap();
    super::super::session_notifications::api::fail("unregister");
    let report = session.shutdown();
    assert_eq!(report.notifications.unregister, Some(5));
    assert!(report.unhook.is_ok() && report.recovery.is_ok());
    assert!(!report.session_changed);
    assert_eq!(session.shutdown().notifications.unregister, Some(5));
}
#[test]
fn notification_reentry_during_recovery_rejects_next_submission() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(test_send(0x1e, false), 1);
    trace.borrow_mut().zero_recovery = true;
    trace.borrow_mut().notify_during_submit = Some((7, 1));
    let report = session.shutdown();
    assert!(report.session_changed && matches!(report.recovery, Err(RuntimeError::Unresolved)));
    // One real rejected recovery API receipt, then the gate prevents all retries.
    assert_eq!(trace.borrow().calls.len(), 2);
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
}
#[test]
fn notification_before_hook_tls_initialization_cannot_be_erased_by_install() {
    super::super::hook_thread::test_notify_before_install();
    let (mut session, trace) = install();
    assert!(session.notifications.access_lost());
    assert_eq!(session.phase(), ActorPhase::Standby);
    assert!(matches!(
        session.poll(),
        Err(SessionError::Hook(HookSessionError::Callback(
            HookFault::SessionChanged
        )))
    ));
    assert!(session.shutdown_report().unwrap().session_changed);
    assert!(session.shutdown_report().unwrap().recovery.is_ok());
    assert!(trace.borrow().calls.is_empty());
}
fn assert_access_loss_evidence(session: &CaptureSession<Backend>, trace: &Rc<RefCell<Trace>>) {
    let report = session.shutdown_report().unwrap();
    assert!(report.access_lost && !report.session_changed);
    assert!(report.unhook.is_ok());
    assert_eq!(report.notifications, NotificationCleanup::default());
    assert!(matches!(report.recovery, Err(RuntimeError::Unresolved)));
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
    assert_eq!(session.actor.borrow().output().phase(), Phase::Quarantined);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn callback_lease_expiry_with_saved_down_does_not_inject_recovery_up() {
    let (mut session, trace, now) = timed_session(config());
    assert_eq!(test_send(0x1e, false), 1);
    now.set(MonoTime(crate::access::LEASE_NS));
    assert_eq!(test_send(0x1e, true), 9);
    assert_eq!(
        session.hook.fault(),
        Some(HookFault::AccessLease(AccessError::Expired))
    );
    assert!(!session.hook.untracked_input());
    assert!(matches!(
        session.poll(),
        Err(SessionError::AccessLease(AccessError::Expired))
    ));
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn control_lease_expiry_with_saved_down_stops_without_metadata_query_or_delivery() {
    let (mut session, trace, _) = timed_session(config());
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(
        monitored_step(&mut session, MonoTime(crate::access::LEASE_NS)),
        Err(HookSessionError::Callback(HookFault::AccessLease(
            AccessError::Expired
        )))
    );
    assert_eq!(super::super::input_probe::test_access_counts(), (0, 0));
    assert!(session.poll().is_err());
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn late_probe_success_with_saved_down_preserves_ledger_instead_of_recovering() {
    let (mut session, trace, _) = timed_session(config());
    assert_eq!(test_send(0x1e, false), 1);
    let times = RefCell::new(std::collections::VecDeque::from([
        MonoTime(crate::access::REFRESH_NS),
        MonoTime(crate::access::LEASE_NS),
    ]));
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    assert_eq!(
        session.hook.poll_with_control(&mut || monitored_wait(
            &actor,
            &emergency,
            &access,
            || times.borrow_mut().pop_front(),
            || Ok(())
        )),
        Err(HookSessionError::Callback(HookFault::AccessLease(
            AccessError::Expired
        )))
    );
    assert!(session.poll().is_err());
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn secondary_metadata_failure_retains_access_loss_and_original_callback_fault() {
    let (mut session, trace, _) = timed_session(config());
    assert_eq!(test_send(0x1e, false), 1);
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    let before = access.borrow().clone();
    assert_eq!(
        session.hook.poll_with_control(&mut || monitored_wait(
            &actor,
            &emergency,
            &access,
            || Some(MonoTime(crate::access::REFRESH_NS)),
            || {
                assert_eq!(super::super::hook_thread::test_record(external_record()), 9);
                Err(ProbeError::HigherIntegrity)
            }
        )),
        Err(HookSessionError::Callback(HookFault::UntrackedInput))
    );
    assert_eq!(*access.borrow(), before);
    assert!(session.poll().is_err());
    assert!(session.shutdown_report().unwrap().untracked_input);
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn later_readonly_probe_success_does_not_clear_loss_or_restore_recovery() {
    let (mut session, trace, _) = timed_session(config());
    assert_eq!(test_send(0x1e, false), 1);
    test_probe(0, false);
    assert!(monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)).is_err());
    test_probe(0, true);
    assert!(check_input_access().is_ok());
    assert!(super::super::hook_thread::access_lost());
    assert!(session.poll().is_err());
    assert_access_loss_evidence(&session, &trace);
}
#[test]
fn access_failure_with_known_empty_ledger_stops_without_backend_call() {
    let (mut session, trace, _) = timed_session(config());
    test_probe(0, false);
    assert!(monitored_step(&mut session, MonoTime(crate::access::REFRESH_NS)).is_err());
    assert!(session.poll().is_err());
    let report = session.shutdown_report().unwrap();
    assert!(report.access_lost && !report.session_changed && report.recovery.is_ok());
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert!(trace.borrow().calls.is_empty());
}
#[test]
fn secondary_access_failure_cannot_turn_unknown_empty_prefix_into_recovered_stop() {
    let (mut session, trace, now) = timed_session(config());
    now.set(MonoTime(crate::access::REFRESH_NS));
    trace.borrow_mut().unknown = true;
    let actor = session.actor.clone();
    let emergency = session.emergency.clone();
    let access = session.access.clone();
    assert!(
        session
            .hook
            .poll_with_control(&mut || monitored_wait(
                &actor,
                &emergency,
                &access,
                || Some(MonoTime(crate::access::REFRESH_NS)),
                || {
                    assert_eq!(test_send(0x1e, false), 9);
                    Err(ProbeError::InactiveDesktop)
                }
            ))
            .is_err()
    );
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_eq!(session.actor.borrow().output().phase(), Phase::Quarantined);
    assert!(session.poll().is_err());
    let report = session.shutdown_report().unwrap();
    assert!(report.access_lost && matches!(report.recovery, Err(RuntimeError::Unresolved)));
    assert!(
        session
            .actor
            .borrow()
            .output()
            .ledger()
            .submitted()
            .is_empty()
    );
    assert_eq!(session.actor.borrow().output().phase(), Phase::Quarantined);
    assert_eq!(trace.borrow().calls.len(), 1);
}
#[test]
fn access_revoked_during_recovery_blocks_all_subsequent_native_calls() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(test_send(0x1e, false), 1);
    trace.borrow_mut().zero_recovery = true;
    trace.borrow_mut().revoke_during_submit = true;
    let report = session.shutdown();
    assert!(report.access_lost && !report.session_changed);
    assert!(matches!(report.recovery, Err(RuntimeError::Unresolved)));
    assert_eq!(trace.borrow().calls.len(), 2);
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
    struct Passive;
    impl HookHandler for Passive {
        fn handle(&mut self, _: HookEvent) -> Result<CaptureDecision, HookFault> {
            Ok(CaptureDecision::Pass)
        }
    }
    assert!(matches!(
        HookSession::install(
            Box::new(Passive),
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::CanonicalScan
        ),
        Err((HookSessionError::Busy, _))
    ));
}
struct ProgressWriter {
    bytes: Rc<RefCell<Vec<u8>>>,
    chunk: usize,
}
impl std::io::Write for ProgressWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.chunk == 0 {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(self.chunk);
        self.bytes.borrow_mut().extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[test]
fn progress_challenge_requires_a_new_complete_session_poll() {
    use crate::progress::{Challenge, ProgressClient, ProgressPhase, Receipt};
    let (mut session, _) = install();
    session.poll().unwrap();
    assert_eq!(session.progress_epoch(), 1);
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut client = ProgressClient::new(
        std::io::Cursor::new(Challenge { run_id: 7, id: 1 }.encode()),
        ProgressWriter {
            bytes: bytes.clone(),
            chunk: 40,
        },
        7,
    )
    .unwrap();
    assert!(client.receive(&session).unwrap());
    assert!(!client.publish(&session).unwrap());
    assert_eq!(test_send(0x1e, false), 1);
    assert_eq!(test_send(0x1e, true), 1);
    assert!(!client.publish(&session).unwrap());
    assert!(!client.flush().unwrap());
    assert!(bytes.borrow().is_empty());
    session.poll().unwrap();
    assert!(client.publish(&session).unwrap());
    let receipt = Receipt::decode(&bytes.borrow()).unwrap();
    assert_eq!(
        (receipt.baseline, receipt.epoch, receipt.phase),
        (1, 2, ProgressPhase::Active)
    );
    assert!(!client.publish(&session).unwrap());
    session.shutdown();
    assert_eq!(session.progress_epoch(), 2);
}
#[test]
fn passive_poll_does_not_publish_until_real_activation() {
    use crate::progress::{Challenge, ProgressClient, Receipt};
    let (mut session, _) = install();
    test_probe(i16::MIN, true);
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut client = ProgressClient::new(
        std::io::Cursor::new(Challenge { run_id: 7, id: 1 }.encode()),
        ProgressWriter {
            bytes: bytes.clone(),
            chunk: 40,
        },
        7,
    )
    .unwrap();
    assert!(client.receive(&session).unwrap());
    assert_eq!(session.poll().unwrap(), SessionStep::Waiting);
    assert_eq!(session.progress_epoch(), 1);
    assert!(!client.publish(&session).unwrap());
    assert!(bytes.borrow().is_empty());
    test_probe(0, true);
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert!(client.publish(&session).unwrap());
    assert_eq!(Receipt::decode(&bytes.borrow()).unwrap().epoch, 2);
    session.shutdown();
}
#[test]
fn partial_progress_receipt_retains_original_poll_and_next_challenge_baseline() {
    use crate::progress::{Challenge, ProgressClient, Receipt};
    let (mut session, _) = install();
    session.poll().unwrap();
    let requests: Vec<_> = [
        Challenge { run_id: 7, id: 1 },
        Challenge { run_id: 7, id: 2 },
    ]
    .into_iter()
    .flat_map(Challenge::encode)
    .collect();
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut client = ProgressClient::new(
        std::io::Cursor::new(requests),
        ProgressWriter {
            bytes: bytes.clone(),
            chunk: 3,
        },
        7,
    )
    .unwrap();
    client.receive(&session).unwrap();
    session.poll().unwrap();
    assert!(!client.publish(&session).unwrap());
    session.poll().unwrap();
    assert!(!client.receive(&session).unwrap());
    for _ in 0..13 {
        client.flush().unwrap();
    }
    assert_eq!(Receipt::decode(&bytes.borrow()).unwrap().epoch, 2);
    client.receive(&session).unwrap();
    assert!(!client.publish(&session).unwrap());
    session.poll().unwrap();
    client.publish(&session).unwrap();
    for _ in 0..13 {
        client.flush().unwrap();
    }
    let r = Receipt::decode(&bytes.borrow()[40..]).unwrap();
    assert_eq!((r.baseline, r.epoch, r.challenge.id), (3, 4, 2));
    session.shutdown();
}
#[test]
fn stopped_progress_does_not_claim_output_recovered_or_process_exited() {
    use crate::progress::{Challenge, ProgressClient, ProgressPhase, Receipt};
    let (mut session, _) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut client = ProgressClient::new(
        std::io::Cursor::new(Challenge { run_id: 7, id: 1 }.encode()),
        ProgressWriter {
            bytes: bytes.clone(),
            chunk: 40,
        },
        7,
    )
    .unwrap();
    client.receive(&session).unwrap();
    super::super::session_notifications::api::send(7, 1);
    assert!(session.poll().is_err());
    assert_eq!(session.phase(), ActorPhase::Unresolved);
    assert!(client.publish(&session).unwrap());
    let r = Receipt::decode(&bytes.borrow()).unwrap();
    assert_eq!(
        (r.baseline, r.epoch, r.phase),
        (1, 2, ProgressPhase::Stopped)
    );
    assert!(matches!(session.poll(), Err(SessionError::Closed)));
    assert_eq!(session.progress_epoch(), 2);
    assert_eq!(
        session.actor.borrow().output().ledger().submitted().len(),
        1
    );
}
#[test]
fn wouldblock_progress_write_does_not_mint_a_reply_or_epoch() {
    use crate::progress::{Challenge, ProgressClient};
    let (mut session, _) = install();
    session.poll().unwrap();
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut client = ProgressClient::new(
        std::io::Cursor::new(Challenge { run_id: 7, id: 1 }.encode()),
        ProgressWriter {
            bytes: bytes.clone(),
            chunk: 0,
        },
        7,
    )
    .unwrap();
    client.receive(&session).unwrap();
    session.poll().unwrap();
    for _ in 0..64 {
        assert!(!client.publish(&session).unwrap());
        assert!(!client.receive(&session).unwrap());
    }
    assert_eq!(session.progress_epoch(), 2);
    assert!(bytes.borrow().is_empty());
    session.shutdown();
}
#[test]
fn exhausted_progress_stops_before_native_poll_and_never_wraps_counter() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    test_send(0x1e, false);
    session.progress_epoch = u64::MAX;
    assert!(matches!(
        session.poll(),
        Err(SessionError::ProgressExhausted)
    ));
    assert_eq!(session.progress_epoch(), u64::MAX);
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert_eq!(trace.borrow().calls.len(), 2);
    assert!(session.shutdown_report().unwrap().unhook.is_ok());
}
#[test]
fn progress_io_is_rejected_in_callback_before_read_write_or_actor_borrow() {
    use crate::progress::{Challenge, ClientError, ProgressClient};
    let (mut session, _) = install();
    session.poll().unwrap();
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut client = ProgressClient::new(
        std::io::Cursor::new(Challenge { run_id: 7, id: 1 }.encode()),
        ProgressWriter {
            bytes: bytes.clone(),
            chunk: 40,
        },
        7,
    )
    .unwrap();
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(matches!(
            client.receive(&session),
            Err(ClientError::InCallback)
        ));
        assert!(matches!(
            client.publish(&session),
            Err(ClientError::InCallback)
        ));
        assert!(matches!(client.flush(), Err(ClientError::InCallback)));
        assert!(matches!(
            ProgressClient::new(std::io::empty(), std::io::sink(), 7),
            Err(ClientError::InCallback)
        ));
    });
    assert!(bytes.borrow().is_empty());
    assert!(client.receive(&session).unwrap());
    assert!(!client.publish(&session).unwrap());
    session.poll().unwrap();
    assert!(client.publish(&session).unwrap());
    session.shutdown();
}

#[test]
fn config_control_rejects_callback_and_borrow_without_mutation_or_native_output() {
    let (mut session, trace) = install();
    session.poll().unwrap();
    assert_eq!(session.config_generation().unwrap(), ConfigGeneration(1));
    let mut next = (*config()).clone();
    next.generation = ConfigGeneration(2);
    let next = Arc::new(next);
    let before = trace.borrow().calls.len();
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(matches!(
            session.config_generation(),
            Err(SessionError::Probe(ProbeError::InCallback))
        ));
        assert!(matches!(
            session.activate_config(next.clone()),
            Err(SessionError::Probe(ProbeError::InCallback))
        ));
    });
    let actor = session.actor.clone();
    {
        let _borrow = actor.borrow_mut();
        assert!(matches!(
            session.config_generation(),
            Err(SessionError::ActorBorrow)
        ));
        assert!(matches!(
            session.activate_config(next.clone()),
            Err(SessionError::ActorBorrow)
        ));
    }
    assert_eq!(session.config_generation().unwrap(), ConfigGeneration(1));
    assert_eq!(trace.borrow().calls.len(), before);
    session.activate_config(next).unwrap();
    assert_eq!(session.config_generation().unwrap(), ConfigGeneration(2));
    session.shutdown();
    assert!(matches!(
        session.config_generation(),
        Err(SessionError::Closed)
    ));
}
