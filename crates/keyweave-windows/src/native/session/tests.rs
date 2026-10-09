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
