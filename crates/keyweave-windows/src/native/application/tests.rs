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
