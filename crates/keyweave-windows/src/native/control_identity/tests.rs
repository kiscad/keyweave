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

#[test]
fn message_scope_checks_sid_logon_session_and_restores_before_returning() {
    for different in 0..4 {
        reset(|m| {
            m.peer_sid = different == 1;
            m.peer_logon = different == 2;
            m.peer_session = different == 3;
        });
        let identity = ControlIdentity::capture().unwrap();
        assert_eq!(
            identity.authorize_message(123usize as Handle).unwrap(),
            different == 0
        );
        MOCK.with(|m| {
            let m = m.borrow();
            assert_eq!(m.impersonations, 1);
            assert_eq!(m.reversions, 1);
            assert!(!m.impersonating);
        });
        clear();
    }
}
#[test]
fn message_query_errors_malformed_tokens_and_panic_restore_without_process_fallback() {
    for bad in [
        "sid",
        "pointer",
        "length",
        "stats",
        "changed",
        "zero",
        "primary",
        "anonymous",
        "impersonation",
        "delegation",
    ] {
        reset(|m| m.message_bad = Some(bad));
        let identity = ControlIdentity::capture().unwrap();
        assert!(
            identity.authorize_message(123usize as Handle).is_err(),
            "{bad}"
        );
        MOCK.with(|m| {
            let m = m.borrow();
            assert_eq!(m.reversions, 1);
            assert!(!m.impersonating);
        });
        clear();
    }
    for class in [1, 10, 12] {
        reset(|m| m.message_class = class);
        let identity = ControlIdentity::capture().unwrap();
        assert!(matches!(
            identity
                .authorize_message(123usize as Handle)
                .unwrap_err()
                .operation,
            Some(Error::Api { code: 5, .. })
        ));
        MOCK.with(|m| {
            let m = m.borrow();
            assert_eq!(m.reversions, 1);
            assert!(!m.impersonating);
        });
        clear();
    }
    reset(|m| m.fail_message_open = true);
    let identity = ControlIdentity::capture().unwrap();
    assert!(identity.authorize_message(123usize as Handle).is_err());
    MOCK.with(|m| assert_eq!(m.borrow().reversions, 1));
    clear();
    reset(|m| m.panic_message_query = true);
    let identity = ControlIdentity::capture().unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || identity.authorize_message(123usize as Handle)
        ))
        .is_err()
    );
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.reversions, 1);
        assert!(!m.impersonating);
    });
    clear();
}
#[test]
fn impersonation_failure_never_runs_message_query_and_revert_failure_never_returns_normally() {
    reset(|m| m.fail_impersonate = true);
    let identity = ControlIdentity::capture().unwrap();
    assert!(matches!(
        identity
            .authorize_message(123usize as Handle)
            .unwrap_err()
            .operation,
        Some(Error::Api { code: 5, .. })
    ));
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.impersonations, 1);
        assert_eq!(m.reversions, 0);
        assert!(!m.impersonating);
    });
    clear();
    reset(|m| m.fail_revert = true);
    let identity = ControlIdentity::capture().unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || identity.authorize_message(123usize as Handle)
        ))
        .is_err()
    );
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.reversions, 1);
        assert!(m.impersonating);
    });
    clear();
}
#[test]
fn message_cleanup_failure_is_preserved_and_identity_is_still_restored() {
    for class in [0, 1] {
        reset(|m| {
            m.peer_only_failure = true;
            m.fail_close = true;
            m.message_class = class;
        });
        let identity = ControlIdentity::capture().unwrap();
        let error = identity.authorize_message(123usize as Handle).unwrap_err();
        assert_eq!(error.token_close, Some(6));
        if class == 0 {
            assert!(error.operation.is_none());
        } else {
            assert!(matches!(error.operation, Some(Error::Api { code: 5, .. })));
        }
        MOCK.with(|m| {
            let m = m.borrow();
            assert_eq!(m.reversions, 1);
            assert!(!m.impersonating);
        });
        assert!(matches!(
            ControlIdentity::capture().unwrap_err().operation,
            Some(Error::Poisoned)
        ));
    }
    reset(|m| m.message_bad = Some("free"));
    let identity = ControlIdentity::capture().unwrap();
    let error = identity.authorize_message(123usize as Handle).unwrap_err();
    assert_eq!(error.sid_text_free, Some(5));
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.reversions, 1);
        assert!(!m.impersonating);
    });
}
#[test]
fn caller_changes_existing_impersonation_and_callback_cannot_authorize_a_message() {
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    MOCK.with(|m| m.borrow_mut().current_session = 2);
    assert_eq!(
        identity
            .authorize_message(123usize as Handle)
            .unwrap_err()
            .operation,
        Some(Error::CallerChanged)
    );
    MOCK.with(|m| assert_eq!(m.borrow().impersonations, 0));
    clear();
    reset(|m| m.after_message_session = Some(2));
    let identity = ControlIdentity::capture().unwrap();
    assert_eq!(
        identity
            .authorize_message(123usize as Handle)
            .unwrap_err()
            .operation,
        Some(Error::CallerChanged)
    );
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.reversions, 1);
        assert!(!m.impersonating);
    });
    clear();
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    MOCK.with(|m| m.borrow_mut().impersonating = true);
    assert_eq!(
        identity
            .authorize_message(123usize as Handle)
            .unwrap_err()
            .operation,
        Some(Error::Impersonating)
    );
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.impersonations, 0);
        assert_eq!(m.reversions, 0);
        assert!(m.impersonating);
    });
    clear();
    reset(|_| {});
    let identity = ControlIdentity::capture().unwrap();
    let before = MOCK.with(|m| m.borrow().calls);
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(
            identity
                .authorize_message(123usize as Handle)
                .unwrap_err()
                .operation,
            Some(Error::InCallback)
        );
    });
    assert_eq!(before, MOCK.with(|m| m.borrow().calls));
    clear();
}

#[test]
fn server_instance_pin_checks_full_filetime_liveness_query_errors_and_callback() {
    use std::mem::{align_of, offset_of, size_of};
    assert_eq!(
        (
            size_of::<FileTime>(),
            align_of::<FileTime>(),
            offset_of!(FileTime, high)
        ),
        (8, 4, 4)
    );
    assert!(ControlServerInstance::from_parts(0, 1).is_err());
    assert!(ControlServerInstance::from_parts(42, 0).is_err());
    reset(|m| m.process_created = 0xfedc_ba98_7654_3210);
    let instance = ControlServerInstance::current().unwrap();
    assert_eq!(instance.process_id(), 42);
    assert_eq!(instance.creation_time(), 0xfedc_ba98_7654_3210);
    let identity = ControlIdentity::capture().unwrap();
    let mut peer = ServerPeer::open(&identity, instance).unwrap();
    peer.check(&identity).unwrap();
    peer.close().unwrap();
    clear();
    for mode in 0..4 {
        reset(|m| match mode {
            0 => m.process_wait = 0,
            1 => m.process_wait = u32::MAX,
            2 => m.fail_times = true,
            _ => m.process_created = 0,
        });
        let identity = ControlIdentity::capture().unwrap();
        let expected = ControlServerInstance::from_parts(42, 123456).unwrap();
        assert!(ServerPeer::open(&identity, expected).is_err());
        clear();
    }
    reset(|_| {});
    let before = MOCK.with(|m| m.borrow().calls);
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(ControlServerInstance::current().is_err())
    });
    assert_eq!(before, MOCK.with(|m| m.borrow().calls));
}
