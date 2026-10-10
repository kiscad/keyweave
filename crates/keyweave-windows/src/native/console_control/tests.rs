#![allow(clippy::unwrap_used)]
use super::*;
use api::{MOCK, Mock};
// Production state is process-global; all mutations in these unit tests are
// serialized, and reset only for isolated mocks, never in the production API.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn reset() {
    CLAIMED.store(false, Ordering::Release);
    REQUESTED.store(false, Ordering::Release);
    MOCK.with(|m| *m.borrow_mut() = Mock::default());
}
#[test]
fn lifetime_once_and_static_event_semantics_survive_explicit_close() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    let mut control = ConsoleControl::install().unwrap();
    assert!(control.available());
    assert!(!control.requested());
    assert_eq!(
        ConsoleControl::install().unwrap_err(),
        ConsoleError::Claimed
    );
    for event in [2, 5, 6, 99] {
        assert_eq!(handler(event), 0);
        assert!(!control.requested());
    }
    assert_eq!(handler(0), 1);
    assert!(control.requested());
    assert_eq!(handler(1), 1);
    control.close().unwrap();
    control.close().unwrap();
    assert!(control.requested());
    assert_eq!(handler(0), 1);
    assert_eq!(
        ConsoleControl::install().unwrap_err(),
        ConsoleError::Claimed
    );
    MOCK.with(|m| {
        assert_eq!(
            m.borrow().calls,
            vec!["presence", "enable", "add", "remove"]
        )
    });
}
#[test]
fn callback_rejection_does_not_consume_claim_or_mutate_registration() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(
            ConsoleControl::install().unwrap_err(),
            ConsoleError::InCallback
        )
    });
    assert!(!CLAIMED.load(Ordering::Acquire));
    MOCK.with(|m| assert!(m.borrow().calls.is_empty()));
    let mut control = ConsoleControl::install().unwrap();
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(control.close(), Err(ConsoleError::InCallback))
    });
    MOCK.with(|m| assert_eq!(m.borrow().calls, vec!["presence", "enable", "add"]));
    control.close().unwrap();
}
#[test]
fn failure_cannot_reclaim_and_remove_failure_retains_owner_for_retry() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    MOCK.with(|m| {
        m.borrow_mut().fail_enable = true;
        m.borrow_mut().error = 5;
    });
    assert_eq!(
        ConsoleControl::install().unwrap_err(),
        ConsoleError::Api {
            call: "SetConsoleCtrlHandler enable",
            code: 5
        }
    );
    assert_eq!(
        ConsoleControl::install().unwrap_err(),
        ConsoleError::Claimed
    );
    MOCK.with(|m| assert_eq!(m.borrow().calls, vec!["presence", "enable"]));
    reset();
    MOCK.with(|m| {
        m.borrow_mut().fail_add = true;
        m.borrow_mut().error = 5;
    });
    assert_eq!(
        ConsoleControl::install().unwrap_err(),
        ConsoleError::Api {
            call: "SetConsoleCtrlHandler add",
            code: 5
        }
    );
    assert_eq!(
        ConsoleControl::install().unwrap_err(),
        ConsoleError::Claimed
    );
    reset();
    let mut control = ConsoleControl::install().unwrap();
    MOCK.with(|m| m.borrow_mut().fail_remove = true);
    assert!(control.close().is_err());
    assert!(control.registered);
    MOCK.with(|m| m.borrow_mut().fail_remove = false);
    control.close().unwrap();
    assert!(!control.registered);
}
#[test]
fn explicit_no_console_is_unavailable_but_other_probe_failure_is_error() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    MOCK.with(|m| {
        m.borrow_mut().presence = Some(0);
        m.borrow_mut().error = 6;
    });
    let mut control = ConsoleControl::install().unwrap();
    assert!(!control.available());
    assert!(!control.requested());
    control.close().unwrap();
    MOCK.with(|m| assert_eq!(m.borrow().calls, vec!["presence"]));
    drop(control);
    reset();
    MOCK.with(|m| {
        m.borrow_mut().presence = Some(0);
        m.borrow_mut().error = 5;
    });
    assert_eq!(
        ConsoleControl::install().unwrap_err(),
        ConsoleError::Api {
            call: "GetConsoleProcessList",
            code: 5
        }
    );
    assert!(CLAIMED.load(Ordering::Acquire));
}
#[test]
fn os_thread_atomic_visibility_and_drop_unregister_are_bounded() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    let control = ConsoleControl::install().unwrap();
    std::thread::spawn(|| assert_eq!(handler(1), 1))
        .join()
        .unwrap();
    assert!(control.requested());
    drop(control);
    MOCK.with(|m| {
        assert_eq!(
            m.borrow().calls,
            vec!["presence", "enable", "add", "remove"]
        )
    });
    assert!(CLAIMED.load(Ordering::Acquire));
}
#[test]
fn private_diagnostic_rejects_existing_console_and_bad_event_and_retains_producer_fault() {
    let _serial = SERIAL.lock().unwrap();
    reset();
    assert_eq!(private_console_allowed(), Err(ConsoleError::Attached));
    assert!(!CLAIMED.load(Ordering::Acquire));
    assert_eq!(signal_event(0), Ok(0));
    assert_eq!(signal_event(1), Ok(1));
    assert_eq!(signal_event(2), Err(ConsoleError::Event));
    MOCK.with(|m| {
        m.borrow_mut().presence = Some(0);
        m.borrow_mut().error = 6;
    });
    assert!(private_console_allowed().is_ok());
    assert_eq!(
        producer_result(Ok(Err(ConsoleError::Api {
            call: "GenerateConsoleCtrlEvent",
            code: 5
        }))),
        Err(ConsoleError::Api {
            call: "GenerateConsoleCtrlEvent",
            code: 5
        })
    );
    assert_eq!(
        producer_result(Err(Box::new("producer panic"))),
        Err(ConsoleError::Thread)
    );
}
