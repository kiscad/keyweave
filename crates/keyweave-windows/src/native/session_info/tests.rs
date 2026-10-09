use super::*;
use std::mem::{align_of, offset_of};
#[test]
fn unicode_wts_layout_matches_sdk_in_both_pointer_widths() {
    assert_eq!(size_of::<Level1>(), 224);
    assert_eq!(align_of::<Level1>(), 8);
    assert_eq!(size_of::<InfoEx>(), 232);
    assert_eq!(align_of::<InfoEx>(), 8);
    assert_eq!(offset_of!(InfoEx, level), 0);
    assert_eq!(offset_of!(InfoEx, data), 8);
    assert_eq!(offset_of!(Level1, session_id), 0);
    assert_eq!(offset_of!(Level1, state), 4);
    assert_eq!(offset_of!(Level1, flags), 8);
    assert_eq!(offset_of!(Level1, station), 12);
    assert_eq!(offset_of!(Level1, user), 78);
    assert_eq!(offset_of!(Level1, domain), 120);
    assert_eq!(offset_of!(Level1, logon), 160);
}
#[test]
fn local_unlocked_active_session_has_stable_identity_and_frees_native_buffer() {
    api::reset(|m| {
        m.caller = 7;
        m.foreground = 7;
    });
    assert_eq!(
        own_session(),
        Ok(SessionIdentity {
            id: 7,
            logon: 1_000_000
        })
    );
    assert_eq!(api::counts(), (1, 1, 0));
    assert_eq!(
        check_foreground(
            100,
            SessionIdentity {
                id: 7,
                logon: 1_000_000
            }
        ),
        Ok(())
    );
}
#[test]
fn inactive_or_locked_or_unknown_sessions_are_not_access_grants() {
    for state in 1..=10 {
        api::reset(|m| m.state = state);
        assert_eq!(own_session(), Err(ProbeError::InactiveSession));
        assert_eq!(api::counts(), (1, 1, 0));
    }
    for (flags, expected) in [
        (0, ProbeError::LockedSession),
        (-1, ProbeError::UnknownSession),
        (2, ProbeError::UnknownSession),
    ] {
        api::reset(|m| m.flags = flags);
        assert_eq!(own_session(), Err(expected));
        assert_eq!(api::counts(), (1, 1, 0));
    }
    for logon in [0, -1] {
        api::reset(|m| m.logon = logon);
        assert_eq!(own_session(), Err(ProbeError::UnknownSession));
        assert_eq!(api::counts(), (1, 1, 0));
    }
}
#[test]
fn unavailable_service_and_pid_query_errors_preserve_native_error() {
    api::reset(|m| m.fail_pid = true);
    assert_eq!(
        own_session(),
        Err(ProbeError::Api {
            call: "ProcessIdToSessionId",
            code: 5
        })
    );
    assert_eq!(api::counts(), (0, 0, 0));
    api::reset(|m| m.fail_query = true);
    assert_eq!(
        own_session(),
        Err(ProbeError::Api {
            call: "WTSQuerySessionInformationW",
            code: 5
        })
    );
    assert_eq!(api::counts(), (1, 0, 0));
}
#[test]
fn service_reserved_and_foreign_sessions_are_rejected() {
    for id in [0, u32::MAX] {
        api::reset(|m| m.caller = id);
        assert_eq!(own_session(), Err(ProbeError::InactiveSession));
        assert_eq!(api::counts(), (0, 0, 0));
    }
    api::reset(|m| m.returned_id = Some(2));
    assert_eq!(own_session(), Err(ProbeError::Changed));
    assert_eq!(api::counts(), (1, 1, 0));
    api::reset(|m| m.foreground = 2);
    assert_eq!(
        check_foreground(
            100,
            SessionIdentity {
                id: 1,
                logon: 1_000_000
            }
        ),
        Err(ProbeError::Changed)
    );
}
#[test]
fn malformed_buffers_are_freed_once_and_never_read_as_partial_layout() {
    for bytes in [0, 1, 4, 20, 231, 233, u32::MAX] {
        api::reset(|m| m.bytes = bytes);
        assert_eq!(own_session(), Err(ProbeError::Malformed));
        assert_eq!(api::counts(), (1, 1, 0));
    }
    for level in [0, 2, u32::MAX] {
        api::reset(|m| m.level = level);
        assert_eq!(own_session(), Err(ProbeError::Malformed));
        assert_eq!(api::counts(), (1, 1, 0));
    }
    api::reset(|m| m.null = true);
    assert_eq!(own_session(), Err(ProbeError::Malformed));
    assert_eq!(api::counts(), (1, 0, 0));
}
#[test]
fn wts_queries_are_forbidden_inside_hook_callback_before_native_calls() {
    api::reset(|_| {});
    assert_eq!(
        super::super::hook_thread::with_callback_borrow(own_session),
        Err(ProbeError::InCallback)
    );
    assert_eq!(api::counts(), (0, 0, 0));
    api::MOCK.with(|m| assert_eq!(m.borrow().processes, 0));
}
