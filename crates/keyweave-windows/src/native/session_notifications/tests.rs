#![allow(clippy::unwrap_used)]
use super::*;
use std::mem::{align_of, offset_of, size_of};
fn install() -> SessionNotifications {
    SessionNotifications::install().unwrap()
}
#[test]
fn class_matches_both_sdk_layouts() {
    let wide = size_of::<usize>() == 8;
    assert_eq!(size_of::<WindowClass>(), if wide { 72 } else { 40 });
    assert_eq!(align_of::<WindowClass>(), size_of::<usize>());
    assert_eq!(offset_of!(WindowClass, procedure), if wide { 8 } else { 4 });
    assert_eq!(
        offset_of!(WindowClass, class_extra),
        if wide { 16 } else { 8 }
    );
    assert_eq!(
        offset_of!(WindowClass, window_extra),
        if wide { 20 } else { 12 }
    );
    assert_eq!(offset_of!(WindowClass, module), if wide { 24 } else { 16 });
    assert_eq!(offset_of!(WindowClass, name), if wide { 64 } else { 36 });
}
#[test]
fn owned_resources_close_in_order_once_and_clean_thread_can_reuse() {
    let mut window = install();
    assert!(!window.access_lost());
    assert_eq!(window.close(), NotificationCleanup::default());
    assert_eq!(window.close(), NotificationCleanup::default());
    drop(window);
    assert_eq!(
        api::MOCK.with(|m| m.borrow().calls.clone()),
        [
            "module",
            "class",
            "window",
            "register",
            "unregister",
            "destroy",
            "unclass"
        ]
    );
    let mut again = install();
    assert!(!again.access_lost());
    again.close();
}
#[test]
fn all_own_events_revoke_without_reactivation_and_other_session_is_ignored() {
    for event in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 15, 0, usize::MAX] {
        std::thread::spawn(move || {
            let mut window = install();
            api::send(event, 2);
            assert!(!window.access_lost());
            api::send(event, 1);
            assert!(window.access_lost());
            api::send(8, 1);
            assert!(window.access_lost());
            window.close();
            assert!(window.access_lost());
            assert!(matches!(
                SessionNotifications::install(),
                Err(NotificationError::Busy)
            ));
        })
        .join()
        .unwrap();
    }
}
#[test]
fn malformed_numeric_session_cannot_authorize_delivery() {
    let window = install();
    api::send(7, -1);
    assert!(window.access_lost());
}
#[test]
fn foreign_window_does_not_revoke_and_unexpected_destruction_does() {
    let mut window = install();
    // SAFETY: procedure only treats this lParam as an integer session ID.
    unsafe {
        procedure(4097usize as Handle, 0x2b1, 7, 1);
    }
    assert!(!window.access_lost());
    // SAFETY: scalar-only NCDESTROY callback on this test-owned window.
    unsafe {
        procedure(window.window, 0x82, 0, 0);
    }
    assert!(window.access_lost());
    assert!(window.close().unexpected_destroy);
    assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"unregister")));
    assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"destroy")));
}
#[test]
fn startup_errors_roll_back_only_acquired_prefix() {
    for (fail, expected) in [
        ("module", vec!["module"]),
        ("class", vec!["module", "class"]),
        ("window", vec!["module", "class", "window", "unclass"]),
        (
            "register",
            vec![
                "module", "class", "window", "register", "destroy", "unclass",
            ],
        ),
    ] {
        std::thread::spawn(move || {
            api::MOCK.with(|m| m.borrow_mut().fail = Some(fail));
            assert!(matches!(
                SessionNotifications::install(),
                Err(NotificationError::Api { code: 5, .. })
            ));
            assert_eq!(api::MOCK.with(|m| m.borrow().calls.clone()), expected);
        })
        .join()
        .unwrap();
    }
}
#[test]
fn registration_callback_stops_startup_and_frees_successful_registration() {
    api::MOCK.with(|m| m.borrow_mut().register_event = Some((7, 1)));
    assert!(matches!(
        SessionNotifications::install(),
        Err(NotificationError::Changed { .. })
    ));
    assert_eq!(
        api::MOCK.with(|m| m.borrow().calls.clone()),
        [
            "module",
            "class",
            "window",
            "register",
            "unregister",
            "destroy",
            "unclass"
        ]
    );
}
#[test]
fn cleanup_failures_are_retained_and_prevent_thread_reuse() {
    for fail in ["unregister", "destroy", "unclass"] {
        std::thread::spawn(move || {
            let mut window = install();
            api::MOCK.with(|m| m.borrow_mut().fail = Some(fail));
            let report = window.close();
            assert!(report.failed());
            assert_eq!(report.unregister.is_some(), fail == "unregister");
            assert_eq!(report.destroy.is_some(), fail == "destroy");
            assert_eq!(report.class.is_some(), fail == "unclass");
            assert_eq!(window.close(), report);
            assert!(matches!(
                SessionNotifications::install(),
                Err(NotificationError::Busy)
            ));
            if fail == "destroy" {
                assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"unclass")));
            }
        })
        .join()
        .unwrap();
    }
}
#[test]
fn callback_during_unregister_is_retained_and_callback_context_cannot_install() {
    let mut window = install();
    api::MOCK.with(|m| m.borrow_mut().unregister_event = Some((6, 1)));
    window.close();
    assert!(window.access_lost());
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(
            SessionNotifications::install().err(),
            Some(NotificationError::Probe(ProbeError::InCallback))
        );
    });
}
#[test]
fn failed_registration_preserves_rollback_error_and_busy_monitor_has_no_acquisition() {
    let window = install();
    let before = api::MOCK.with(|m| m.borrow().calls.len());
    assert!(matches!(
        SessionNotifications::install(),
        Err(NotificationError::Busy)
    ));
    assert_eq!(api::MOCK.with(|m| m.borrow().calls.len()), before);
    drop(window);
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.fail = Some("register");
        m.fail_also = Some("destroy");
    });
    assert!(matches!(
        SessionNotifications::install(),
        Err(NotificationError::Api {
            call: "WTSRegisterSessionNotification",
            code: 5,
            cleanup: NotificationCleanup {
                destroy: Some(5),
                unregister: None,
                class: None,
                unexpected_destroy: false
            }
        })
    ));
    assert!(matches!(
        SessionNotifications::install(),
        Err(NotificationError::Busy)
    ));
}
#[test]
fn unregister_reentry_can_destroy_window_and_cleanup_does_not_reuse_its_hwnd() {
    let mut window = install();
    api::MOCK.with(|m| m.borrow_mut().destroy_during_unregister = true);
    let report = window.close();
    assert!(report.unexpected_destroy && window.access_lost());
    assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"destroy")));
    assert_eq!(
        api::MOCK.with(|m| m.borrow().calls.last().copied()),
        Some("unclass")
    );
}
