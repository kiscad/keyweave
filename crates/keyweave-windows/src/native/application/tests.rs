#![allow(clippy::unwrap_used)]
use super::*;
use api::{MOCK, Mock};
#[test]
fn known_path_keeps_utf16_and_never_invents_bundle_identifier() {
    MOCK.with(|m| {
        *m.borrow_mut() = Mock {
            path: "C:\\应用\\editor.exe".encode_utf16().collect(),
            ..Default::default()
        }
    });
    let context = sample().unwrap().unwrap();
    assert_eq!(context.file_path.as_deref(), Some("C:\\应用\\editor.exe"));
    assert_eq!(context.bundle_identifier, None);
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!((m.queries, m.closes, m.live), (1, 1, false));
    });
}
#[test]
fn failed_or_changed_metadata_is_unknown_and_closes_acquired_owner() {
    for mode in 0..3 {
        MOCK.with(|m| {
            *m.borrow_mut() = Mock {
                open: mode != 0,
                query: mode != 1,
                changed: mode == 2,
                ..Default::default()
            }
        });
        assert_eq!(sample().unwrap(), None);
        MOCK.with(|m| {
            let m = m.borrow();
            assert_eq!(m.closes, usize::from(mode != 0));
            assert!(!m.live);
        });
    }
}
#[test]
fn malformed_or_oversized_native_strings_are_not_truncated_or_lossy() {
    for mock in [
        Mock {
            path: vec![0xd800],
            ..Default::default()
        },
        Mock {
            path: vec![65, 0, 66],
            ..Default::default()
        },
        Mock {
            terminate: false,
            ..Default::default()
        },
        Mock {
            count: Some(CAPACITY as u32),
            ..Default::default()
        },
        Mock {
            count: Some(0),
            ..Default::default()
        },
        Mock {
            path: vec![0x4e00; 1400],
            ..Default::default()
        },
    ] {
        MOCK.with(|m| *m.borrow_mut() = mock);
        assert_eq!(sample().unwrap(), None);
        MOCK.with(|m| assert_eq!(m.borrow().closes, 1));
    }
}
#[test]
fn close_failure_is_not_masked_by_sampling_failure_and_is_not_retried() {
    MOCK.with(|m| {
        *m.borrow_mut() = Mock {
            close: false,
            query: false,
            ..Default::default()
        }
    });
    assert_eq!(
        sample(),
        Err(ProbeError::Api {
            call: "CloseHandle(application)",
            code: 5
        })
    );
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!((m.closes, m.live), (1, true));
    });
}
#[test]
fn actual_utf16_sample_matches_utf8_path_conditions_without_fabricating_bundle_id() {
    use keyweave_engine::{Condition, Pattern, Truth};
    use keyweave_types::{ContextSnapshot, DeviceInstanceId, MonoTime};
    MOCK.with(|m| {
        *m.borrow_mut() = Mock {
            path: "C:\\应用\\editor.exe".encode_utf16().collect(),
            ..Default::default()
        }
    });
    let application = sample().unwrap().unwrap();
    let context = ContextSnapshot {
        revision: 1,
        provider: "Win32 API mock".into(),
        observed_at_ns: MonoTime(0),
        valid_until_ns: MonoTime(100),
        application: Some(application),
        ..Default::default()
    };
    let condition = Condition::Application {
        file_paths: vec![Pattern::compile(r"^C:\\应用\\editor\.exe$").unwrap()],
        bundle_identifiers: vec![],
        unless: false,
    };
    let device = DeviceInstanceId {
        id: 1,
        generation: 1,
    };
    assert_eq!(
        condition.evaluate(&context, device, MonoTime(1)),
        Truth::True
    );
    assert_eq!(
        condition.evaluate(&context, device, MonoTime(100)),
        Truth::Unknown
    );
    assert_eq!(context.application.unwrap().bundle_identifier, None);
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!((m.queries, m.closes, m.live), (1, 1, false));
    });
}
