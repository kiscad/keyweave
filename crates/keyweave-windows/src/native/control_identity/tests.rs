#![allow(clippy::unwrap_used)]
use super::*;
use api::{MOCK, Mock};
fn reset(change: impl FnOnce(&mut Mock)) {
    POISONED.set(false);
    MOCK.with(|m| {
        let mut value = Mock::default();
        change(&mut value);
        *m.borrow_mut() = value;
    });
}
fn clear() {
    MOCK.with(|m| {
        let m = m.borrow();
        assert!(m.handles.is_empty());
        assert!(m.strings.is_empty());
        assert!(m.descriptors.is_empty());
    });
}
#[test]
fn real_capture_checks_stable_token_scope_and_allocates_only_explicit_user_dacl() {
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    assert!(identity.same_primary_token_scope_of(42).unwrap());
    clear();
    assert!(!format!("{identity:?}").contains("S-1-"));
    let mut descriptor = identity.security_descriptor().unwrap();
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.sddl, "D:P(A;;GA;;;S-1-5-21-100)(A;;GA;;;SY)");
        assert!(!m.sddl.contains(";;;OW)"));
        assert!(!m.sddl.contains(";;;WD)"));
        assert_eq!(m.descriptors.len(), 1);
    });
    descriptor.close().unwrap();
    descriptor.close().unwrap();
    clear();
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    MOCK.with(|m| m.borrow_mut().fail_descriptor = true);
    assert_eq!(
        identity.security_descriptor().err().unwrap().operation,
        Some(Error::Api {
            call: "ConvertStringSecurityDescriptorToSecurityDescriptorW(control)",
            code: 1336
        })
    );
    clear();
}
#[test]
fn same_sid_in_another_logon_or_terminal_session_is_not_same_control_scope() {
    for kind in 0..3 {
        reset(|_| {});
        let identity = ControlIdentity::capture().unwrap();
        MOCK.with(|m| {
            let mut m = m.borrow_mut();
            match kind {
                0 => m.peer_sid = true,
                1 => m.peer_logon = true,
                _ => m.peer_session = true,
            }
        });
        assert!(!identity.same_primary_token_scope_of(42).unwrap());
        clear();
    }
}
#[test]
fn malformed_pointer_length_sid_text_and_changed_statistics_never_return_a_principal() {
    for kind in 0..9 {
        reset(|m| match kind {
            0 => m.bad_pointer = true,
            1 => m.bad_length = true,
            2 => m.bad_sid = true,
            3 => m.wrong_text = true,
            4 => m.changed = true,
            5 => m.session_zero = true,
            6 => m.fail_class = 1,
            7 => m.token_type_bad = true,
            _ => m.short_stats = true,
        });
        assert!(ControlIdentity::capture().is_err(), "case {kind}");
        clear();
    }
}
#[test]
fn impersonation_or_unknown_thread_token_failure_does_not_fall_back_to_process_user() {
    reset(|m| m.impersonating = true);
    let error = ControlIdentity::capture().err().unwrap();
    assert_eq!(error.operation, Some(Error::Impersonating));
    clear();
    reset(|m| m.thread_error = 5);
    let error = ControlIdentity::capture().err().unwrap();
    assert_eq!(
        error.operation,
        Some(Error::Api {
            call: "OpenThreadToken(control)",
            code: 5
        })
    );
    clear();
}
#[test]
fn operation_error_and_independent_cleanup_failure_are_preserved_and_poison_new_capture() {
    reset(|m| {
        m.fail_class = 1;
        m.fail_close = true;
    });
    let failure = ControlIdentity::capture().err().unwrap();
    assert_eq!(
        failure.operation,
        Some(Error::Api {
            call: "GetTokenInformation(control)",
            code: 5
        })
    );
    assert_eq!(failure.token_close, Some(6));
    let calls = MOCK.with(|m| m.borrow().calls);
    assert_eq!(
        ControlIdentity::capture().err().unwrap().operation,
        Some(Error::Poisoned)
    );
    MOCK.with(|m| assert_eq!(m.borrow().calls, calls));
    reset(|m| m.fail_free = true);
    let failure = ControlIdentity::capture().err().unwrap();
    assert_eq!(failure.operation, None);
    assert_eq!(failure.sid_text_free, Some(5));
    assert!(POISONED.get());
}
#[test]
fn descriptor_failure_retains_owner_and_caller_change_or_callback_never_creates_security() {
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    let mut descriptor = identity.security_descriptor().unwrap();
    MOCK.with(|m| m.borrow_mut().fail_free = true);
    assert!(descriptor.close().is_err());
    MOCK.with(|m| assert_eq!(m.borrow().descriptors.len(), 1));
    MOCK.with(|m| m.borrow_mut().fail_free = false);
    descriptor.close().unwrap();
    clear();
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    MOCK.with(|m| m.borrow_mut().current_session = 2);
    assert_eq!(
        identity.security_descriptor().err().unwrap().operation,
        Some(Error::CallerChanged)
    );
    MOCK.with(|m| assert!(m.borrow().descriptors.is_empty()));
    reset(|_| {});
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(
            ControlIdentity::capture().err().unwrap().operation,
            Some(Error::InCallback)
        )
    });
    MOCK.with(|m| assert_eq!(m.borrow().calls, 0));
}
#[test]
fn peer_process_handle_is_closed_on_identity_error_and_does_not_authorize_a_pid_alone() {
    for after in [false, true] {
        reset(|_| {});
        let identity = ControlIdentity::capture().unwrap();
        MOCK.with(|m| {
            let mut m = m.borrow_mut();
            m.wrong_process = !after;
            m.wrong_process_after = after;
        });
        assert!(identity.same_primary_token_scope_of(42).is_err());
        clear();
        assert!(identity.same_primary_token_scope_of(0).is_err());
        clear();
        assert!(identity.same_primary_token_scope_of(41).is_err());
        clear();
    }
}

#[test]
fn peer_lookup_preserves_token_and_process_close_errors_without_unbounded_retries() {
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.peer_only_failure = true;
        m.fail_class = 1;
        m.fail_close = true;
    });
    let failure = identity.same_primary_token_scope_of(42).unwrap_err();
    assert_eq!(
        failure.operation,
        Some(Error::Api {
            call: "GetTokenInformation(control)",
            code: 5
        })
    );
    assert_eq!(failure.token_close, Some(6));
    assert_eq!(failure.process_close, Some(6));
    assert!(POISONED.get());
    MOCK.with(|m| assert_eq!(m.borrow().handles.len(), 2));
}
#[test]
fn empty_descriptor_drop_in_callback_does_not_poison_a_clean_thread() {
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    let mut descriptor = identity.security_descriptor().unwrap();
    descriptor.close().unwrap();
    super::super::hook_thread::with_callback_borrow(|| drop(descriptor));
    assert!(!POISONED.get());
    clear();
}

#[test]
fn live_descriptor_drop_in_callback_defers_free_and_poison_blocks_further_allocation() {
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    let descriptor = identity.security_descriptor().unwrap();
    super::super::hook_thread::with_callback_borrow(|| drop(descriptor));
    assert!(POISONED.get());
    MOCK.with(|m| assert_eq!(m.borrow().descriptors.len(), 1));
    assert_eq!(
        ControlIdentity::capture().err().unwrap().operation,
        Some(Error::Poisoned)
    );
}
