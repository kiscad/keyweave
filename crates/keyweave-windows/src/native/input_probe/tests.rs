use super::*;
use std::mem::{align_of, offset_of};
fn reset(configure: impl FnOnce(&mut api::Mock)) {
    api::MOCK.with(|mock| {
        let mut mock = mock.borrow_mut();
        *mock = api::Mock::default();
        configure(&mut mock);
    });
}
fn assert_closed() {
    api::MOCK.with(|mock| {
        let mock = mock.borrow();
        if !mock.fail_close {
            assert_eq!(mock.live_handles(), 0);
        }
        assert!(mock.closed.iter().all(|(_, handle)| *handle != 1
            && *handle != 2
            && *handle as isize != -1
            && *handle as isize != -2));
    });
}
#[test]
fn visible_accessible_context_samples_high_bit_and_ignores_low_bit() {
    for (key, expected) in [
        (0, InputSample::IdleObserved),
        (1, InputSample::IdleObserved),
        (i16::MIN, InputSample::Held),
    ] {
        reset(|mock| mock.key = key);
        assert_eq!(sample_input_state(), Ok(expected));
        api::MOCK.with(|mock| {
            let mock = mock.borrow();
            assert_eq!(mock.key_reads, 254);
            assert_eq!(
                mock.closed.iter().filter(|(desktop, _)| *desktop).count(),
                2
            );
        });
        assert_closed();
    }
}
#[test]
fn invisible_inactive_and_denied_desktops_are_not_idle() {
    for mode in 0..3 {
        reset(|mock| match mode {
            0 => mock.visible = false,
            1 => mock.active = false,
            _ => mock.deny_desktop = true,
        });
        let expected = match mode {
            0 => ProbeError::NotInteractive,
            1 => ProbeError::InactiveDesktop,
            _ => ProbeError::Api {
                call: "OpenInputDesktop",
                code: 5,
            },
        };
        assert_eq!(sample_input_state(), Err(expected));
        api::MOCK.with(|mock| assert_eq!(mock.borrow().key_reads, 0));
        assert_closed();
    }
}
#[test]
fn elevated_foreground_and_impersonation_fail_before_key_sampling() {
    reset(|mock| mock.elevated = true);
    assert_eq!(sample_input_state(), Err(ProbeError::HigherIntegrity));
    api::MOCK.with(|mock| assert_eq!(mock.borrow().key_reads, 0));
    assert_closed();
    reset(|mock| mock.impersonating = true);
    assert_eq!(sample_input_state(), Err(ProbeError::Impersonating));
    api::MOCK.with(|mock| assert_eq!(mock.borrow().key_reads, 0));
    assert_closed();
}
#[test]
fn malformed_metadata_and_foreign_sid_pointer_are_rejected() {
    for mode in 0..3 {
        reset(|mock| match mode {
            0 => mock.bad_name = true,
            1 => mock.bad_sid = true,
            _ => mock.large_token = true,
        });
        assert_eq!(sample_input_state(), Err(ProbeError::Malformed));
        api::MOCK.with(|mock| assert_eq!(mock.borrow().key_reads, 0));
        assert_closed();
    }
}
#[test]
fn foreground_change_rejects_an_apparently_idle_sample() {
    reset(|mock| mock.foreground_changed = true);
    assert_eq!(sample_input_state(), Err(ProbeError::Changed));
    assert_closed();
}
#[test]
fn query_or_close_failures_preserve_api_error_and_release_owned_handles() {
    reset(|mock| mock.fail_label = true);
    assert_eq!(
        sample_input_state(),
        Err(ProbeError::Api {
            call: "GetTokenInformation(label)",
            code: 5
        })
    );
    assert_closed();
    reset(|mock| mock.fail_close = true);
    assert_eq!(
        sample_input_state(),
        Err(ProbeError::Api {
            call: "CloseHandle",
            code: 5
        })
    );
    assert_closed();
}
#[test]
fn callback_query_is_rejected_without_native_metadata_or_key_calls() {
    reset(|_| {});
    assert_eq!(
        super::super::hook_thread::with_callback_borrow(sample_input_state),
        Err(ProbeError::InCallback)
    );
    api::MOCK.with(|mock| {
        let mock = mock.borrow();
        assert_eq!(mock.key_reads, 0);
        assert!(mock.closed.is_empty());
    });
}
#[test]
fn user_flags_and_token_label_layouts_match_sdk_contract() {
    assert_eq!(size_of::<UserFlags>(), 12);
    assert_eq!(offset_of!(UserFlags, inherit), 0);
    assert_eq!(offset_of!(UserFlags, reserved), 4);
    assert_eq!(offset_of!(UserFlags, flags), 8);
    assert_eq!(
        size_of::<Label>(),
        if size_of::<usize>() == 8 { 16 } else { 8 }
    );
    assert_eq!(align_of::<Label>(), align_of::<usize>());
    assert_eq!(offset_of!(Label, sid), 0);
    assert_eq!(offset_of!(Label, attributes), size_of::<usize>());
}
#[test]
fn sid_parser_checks_revision_authority_attributes_and_entire_returned_span() {
    for mode in 0..6 {
        let mut buffer = [0u64; 32];
        let header = size_of::<Label>();
        let mut length = header + 12;
        let base = buffer.as_mut_ptr().cast::<u8>();
        // SAFETY: test fixture's aligned 256-byte buffer is live; header and
        // 12-byte embedded SID fit. The parser must reject malformed fields
        // without ever dereferencing the supplied SID pointer independently.
        unsafe {
            base.cast::<Label>().write(Label {
                sid: base.add(header) as usize,
                attributes: if mode == 3 { 0 } else { 0x20 },
            });
            let sid = std::slice::from_raw_parts_mut(base.add(header), 12);
            sid.copy_from_slice(&[1, 1, 0, 0, 0, 0, 0, 16, 0, 0x20, 0, 0]);
            match mode {
                0 => sid[0] = 0,
                1 => sid[1] = 2,
                2 => sid[7] = 5,
                4 => length -= 1,
                5 => length = 257,
                _ => {}
            }
        }
        assert_eq!(parse_integrity(&buffer, length), Err(ProbeError::Malformed));
    }
}
#[test]
fn runtime_access_checks_metadata_and_closes_owners_without_key_sampling() {
    reset(|mock| mock.key = i16::MIN);
    assert_eq!(check_input_access(), Ok(()));
    api::MOCK.with(|mock| {
        let mock = mock.borrow();
        assert_eq!(mock.key_reads, 0);
        assert_eq!(mock.foreground_reads, 2);
    });
    assert_closed();
}
#[test]
fn runtime_metadata_errors_cannot_be_mistaken_for_access() {
    for mode in 0..5 {
        reset(|mock| match mode {
            0 => mock.active = false,
            1 => mock.elevated = true,
            2 => mock.impersonating = true,
            3 => mock.foreground_flip = true,
            _ => mock.fail_close = true,
        });
        let expected = match mode {
            0 => ProbeError::InactiveDesktop,
            1 => ProbeError::HigherIntegrity,
            2 => ProbeError::Impersonating,
            3 => ProbeError::Changed,
            _ => ProbeError::Api {
                call: "CloseHandle",
                code: 5,
            },
        };
        assert_eq!(check_input_access(), Err(expected));
        api::MOCK.with(|mock| assert_eq!(mock.borrow().key_reads, 0));
        assert_closed();
    }
}
#[test]
fn runtime_access_query_is_refused_inside_callback_before_any_native_calls() {
    reset(|_| {});
    assert_eq!(
        super::super::hook_thread::with_callback_borrow(check_input_access),
        Err(ProbeError::InCallback)
    );
    api::MOCK.with(|mock| {
        let mock = mock.borrow();
        assert_eq!(mock.foreground_reads, 0);
        assert!(mock.closed.is_empty());
        assert_eq!(mock.key_reads, 0);
    });
}
