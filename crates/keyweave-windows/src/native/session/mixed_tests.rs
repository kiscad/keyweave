#![allow(clippy::unwrap_used)]
use super::super::{MixedCaptureSession, MixedShutdownReport};
use super::*;
use crate::{
    mixed::NativePacket,
    native::hook_thread::{
        test_mouse_install_failure, test_mouse_record, test_registered_kinds, test_send,
    },
    pump::Phase,
};
use keyweave_types::ConfigGeneration;
#[derive(Default)]
struct Trace {
    calls: Vec<Vec<NativePacket>>,
    inject: Option<(u32, u32, usize)>,
}
struct Backend(Rc<RefCell<Trace>>);
impl SendMixedBackend for Backend {
    fn submit_mixed(&mut self, packets: &[NativePacket]) -> Result<Insertion, SubmitError> {
        let mut trace = self.0.borrow_mut();
        trace.calls.push(packets.to_vec());
        if let Some((message, flags, tag)) = trace.inject.take() {
            assert_eq!(test_mouse_record(message, 0, flags, tag), 9);
        }
        Ok(Insertion {
            accepted: packets.len(),
            last_error: None,
        })
    }
}
fn config(generation: u64) -> Arc<ConfigSnapshot> {
    Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(generation),
        name: "mixed".into(),
        simple: vec![],
        complex: vec![],
    })
}
fn install() -> (MixedCaptureSession<Backend>, Rc<RefCell<Trace>>) {
    let trace = Rc::new(RefCell::new(Trace::default()));
    let session = MixedCaptureSession::install(
        Backend(trace.clone()),
        InjectionTag::new(43).unwrap(),
        IdentityPolicy::CanonicalScan,
        config(1),
    )
    .unwrap();
    (session, trace)
}
fn active() -> (MixedCaptureSession<Backend>, Rc<RefCell<Trace>>) {
    let (mut session, trace) = install();
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    (session, trace)
}
#[test]
fn mixed_startup_real_poll_config_and_two_hook_shutdown() {
    let (mut session, trace) = install();
    assert_eq!(test_registered_kinds(), [13, 14]);
    assert_eq!(session.progress_epoch(), 0);
    assert_eq!(session.progress_phase(), None);
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert_eq!(session.progress_epoch(), 1);
    assert_eq!(session.phase(), ActorPhase::Active);
    assert_eq!(
        session.progress_phase(),
        Some(keyweave_progress::ProgressPhase::Active)
    );
    assert!(session.shutdown_report().is_none());
    session.activate_config(config(2)).unwrap();
    assert_eq!(session.config_generation().unwrap(), ConfigGeneration(2));
    assert_eq!(test_mouse_record(0x201, 0, 0, 0), 1);
    assert_eq!(session.poll().unwrap(), SessionStep::Continue);
    assert!(
        trace
            .borrow()
            .calls
            .iter()
            .flatten()
            .any(|p| matches!(p,NativePacket::Mouse(p)if p.flags()==2))
    );
    let report: &MixedShutdownReport = session.shutdown();
    assert!(report.unhook.is_ok() && report.mouse_unhook.is_ok() && report.recovery.is_ok());
    assert!(!report.access_lost && !report.untracked_input && !report.session_changed);
    assert!(report.application_provider.is_none());
    assert_eq!(report.notifications, NotificationCleanup::default());
    assert_eq!(test_mouse_record(0x201, 0, 0, 0), 9);
    assert_eq!(test_send(0x1e, false), 9);
    assert_eq!(session.phase(), ActorPhase::Stopped);
    assert_eq!(
        session.progress_phase(),
        Some(keyweave_progress::ProgressPhase::Stopped)
    );
    assert!(session.poll().is_err());
    assert!(session.config_generation().is_err());
    let count = trace.borrow().calls.len();
    session.shutdown();
    assert_eq!(trace.borrow().calls.len(), count);
}
#[test]
fn passive_mouse_binding_blocks_activation_until_its_real_up() {
    let (mut session, trace) = install();
    assert_eq!(test_mouse_record(0x20b, 1 << 16, 0, 0), 9);
    assert_eq!(session.poll().unwrap(), SessionStep::Waiting);
    assert_eq!(session.phase(), ActorPhase::Standby);
    assert!(trace.borrow().calls.is_empty());
    assert_eq!(test_mouse_record(0x20c, 1 << 16, 0, 0), 9);
    assert_eq!(session.poll().unwrap(), SessionStep::Activated);
    assert!(session.shutdown().recovery.is_ok());
}
#[test]
fn lost_mouse_access_lease_disables_both_and_forbids_recovery_ups() {
    let (mut session, trace) = active();
    assert_eq!(test_mouse_record(0x201, 0, 0, 0), 1);
    *session.access.borrow_mut() = AccessLease::new();
    assert_eq!(test_mouse_record(0x202, 0, 0, 0), 9);
    assert!(matches!(
        session.hook.fault(),
        Some(HookFault::AccessLease(AccessError::Ungranted))
    ));
    let report = session.shutdown();
    assert!(report.access_lost);
    assert!(report.recovery.is_err());
    assert_eq!(trace.borrow().calls.len(), 1);
    assert_eq!(session.actor.borrow().output().phase(), Phase::Quarantined);
}
#[test]
fn external_mouse_injection_and_session_notification_preserve_accepted_button() {
    for notification in [false, true] {
        std::thread::spawn(move || {
            let (mut session, trace) = active();
            assert_eq!(test_mouse_record(0x201, 0, 0, 0), 1);
            if notification {
                crate::native::session_notifications::api::send(7, 1);
            } else {
                assert_eq!(test_mouse_record(0x204, 0, 1, 44), 9);
            }
            let report = session.shutdown();
            assert!(report.recovery.is_err());
            assert_eq!(trace.borrow().calls.len(), 1);
            assert_eq!(
                session.actor.borrow().output().ledger().submitted().len(),
                1
            );
        })
        .join()
        .unwrap();
    }
}
#[test]
fn own_mouse_reentry_is_bypassed_but_physical_reentry_quarantines() {
    for own in [true, false] {
        std::thread::spawn(move || {
            let (mut session, trace) = active();
            trace.borrow_mut().inject = Some((0x204, if own { 1 } else { 0 }, 43));
            assert_eq!(test_mouse_record(0x201, 0, 0, 0), if own { 1 } else { 9 });
            if own {
                assert!(session.shutdown().recovery.is_ok());
            } else {
                let report = session.shutdown();
                assert!(report.untracked_input);
                assert!(report.recovery.is_err());
                assert_eq!(trace.borrow().calls.len(), 1);
            }
        })
        .join()
        .unwrap();
    }
}
#[test]
fn mouse_install_failure_rolls_back_passive_keyboard_without_injection() {
    test_mouse_install_failure();
    let trace = Rc::new(RefCell::new(Trace::default()));
    let result = MixedCaptureSession::install(
        Backend(trace.clone()),
        InjectionTag::new(43).unwrap(),
        IdentityPolicy::CanonicalScan,
        config(1),
    );
    assert!(matches!(
        result,
        Err(SessionError::MouseInstall {
            keyboard_unhook: Ok(()),
            ..
        })
    ));
    assert!(trace.borrow().calls.is_empty());
    assert_eq!(test_send(0x1e, false), 9);
}
