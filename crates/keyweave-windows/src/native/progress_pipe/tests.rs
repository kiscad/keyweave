#![allow(clippy::unwrap_used)]
use super::*;
use std::mem::{align_of, offset_of};
fn pair() -> (PipeReader, PipeWriter) {
    PipeReader::pair().unwrap()
}
#[test]
fn job_transfer_retains_both_endpoint_close_errors_and_can_retry() {
    use super::super::job_process::{JobError, JobProcessState, OwnedJobProcess};
    let (reader, _parent_writer) = pair();
    let (_parent_reader, writer) = pair();
    api::MOCK.with(|m| m.borrow_mut().fail = Some("close"));
    let mut failure = OwnedJobProcess::spawn_suspended_with_progress(
        &std::env::temp_dir().join("worker.exe"),
        &[],
        reader,
        writer,
    )
    .unwrap_err();
    assert_eq!(failure.cause, JobError::ChannelCleanup);
    let report = failure.channel_cleanup.as_ref().unwrap();
    for endpoint in [report.reader, report.writer] {
        assert_eq!(endpoint.pipe_close_error, Some(5));
        assert_eq!(endpoint.event_close_error, Some(5));
        assert!(!endpoint.completion_pending);
    }
    assert_eq!(report.stdout_close_error, None);
    let owner = failure.owner.as_mut().unwrap();
    assert_eq!(owner.state(), JobProcessState::StopRequested);
    assert_eq!(owner.resume(), Err(JobError::State));
    api::MOCK.with(|m| m.borrow_mut().fail = None);
    assert!(owner.finish_channel_transfer().unwrap().complete());
    assert_eq!(owner.resume(), Err(JobError::State));
    assert!(!failure.channel_cleanup.as_ref().unwrap().complete());
}
#[test]
fn pending_transfer_cancellation_keeps_buffers_until_confirmed_completion() {
    use super::super::job_process::{JobError, JobProcessState, OwnedJobProcess};
    let (mut reader, _parent_writer) = pair();
    let (_parent_reader, writer) = pair();
    reader.read(&mut [0u8; 24]).unwrap_err();
    api::MOCK.with(|m| m.borrow_mut().defer_cancel = true);
    let mut failure = OwnedJobProcess::spawn_suspended_with_progress(
        &std::env::temp_dir().join("worker.exe"),
        &[],
        reader,
        writer,
    )
    .unwrap_err();
    assert_eq!(failure.cause, JobError::PipeState);
    let report = failure.channel_cleanup.as_ref().unwrap();
    assert!(report.reader.completion_pending);
    assert!(report.writer.complete());
    api::MOCK.with(|m| {
        let m = m.borrow();
        assert!(!m.closes.contains(&100) && !m.closes.contains(&301));
    });
    let owner = failure.owner.as_mut().unwrap();
    assert_eq!(owner.state(), JobProcessState::Prepared);
    assert_eq!(owner.poll_exit(std::time::Duration::ZERO).unwrap(), None);
    api::MOCK.with(|m| m.borrow_mut().defer_cancel = false);
    assert!(owner.finish_channel_transfer().unwrap().complete());
    assert!(!POISONED.get());
}
#[test]
fn factory_checks_both_endpoints_before_connecting() {
    let (mut reader, mut writer) = pair();
    api::MOCK.with(|m| {
        let m = m.borrow();
        let calls: Vec<_> = m
            .calls
            .iter()
            .copied()
            .filter(|name| {
                name.starts_with("info-") || name.starts_with("state-") || *name == "connect"
            })
            .collect();
        assert_eq!(
            calls,
            [
                "info-reader",
                "state-reader",
                "info-writer",
                "state-writer",
                "connect"
            ]
        );
        assert!(!m.calls.contains(&"read") && !m.calls.contains(&"write"));
    });
    assert!(reader.close().unwrap().complete());
    assert!(writer.close().unwrap().complete());
}
#[test]
fn factory_rejects_changed_metadata_and_closes_without_starting_io() {
    for handle in [100, 200] {
        // flags: wrong endpoint, message type, unknown type; instance limits;
        // state: NOWAIT, message read mode, unknown mode.
        for (flags, instances, state) in [
            (Some(8 | u32::from(handle != 100)), None, None),
            (Some(8 | u32::from(handle == 100) | 4), None, None),
            (Some(u32::from(handle == 100)), None, None),
            (Some(16), None, None),
            (None, Some(0), None),
            (None, Some(2), None),
            (None, Some(255), None),
            (None, None, Some(1)),
            (None, None, Some(2)),
            (None, None, Some(4)),
        ] {
            api::MOCK.with(|m| {
                let mut m = m.borrow_mut();
                *m = api::Mock::default();
                m.metadata_handle = Some(handle);
                m.pipe_flags = flags;
                m.pipe_instances = instances;
                m.pipe_state = state;
            });
            let Err(cause) = PipeReader::pair() else {
                panic!("invalid pipe admitted")
            };
            assert_eq!(cause.kind(), io::ErrorKind::InvalidData);
            api::MOCK.with(|m| {
                let m = m.borrow();
                assert!(!m.calls.contains(&"connect") && !m.calls.contains(&"cancel"));
                assert!(!m.calls.contains(&"read") && !m.calls.contains(&"write"));
                assert_eq!(m.closes.len(), 4);
                for handle in [100, 200, 301, 302] {
                    assert!(m.closes.contains(&handle));
                }
            });
            assert!(!POISONED.get());
        }
    }
}
#[test]
fn metadata_api_failures_preserve_primary_error_and_close_endpoints() {
    for fail in ["info-reader", "info-writer", "state-reader", "state-writer"] {
        api::MOCK.with(|m| {
            let mut m = m.borrow_mut();
            *m = api::Mock::default();
            m.fail = Some(fail);
        });
        let Err(cause) = PipeReader::pair() else {
            panic!("failed query admitted")
        };
        assert_eq!(cause.raw_os_error(), Some(5));
        api::MOCK.with(|m| {
            let m = m.borrow();
            assert_eq!(m.closes.len(), 4);
            assert!(!m.calls.contains(&"connect") && !m.calls.contains(&"cancel"));
        });
        assert!(!POISONED.get());
    }
}
#[test]
fn overlapped_security_layouts_match_sdk() {
    let wide = size_of::<usize>() == 8;
    assert_eq!(size_of::<Overlapped>(), if wide { 32 } else { 20 });
    assert_eq!(align_of::<Overlapped>(), size_of::<usize>());
    assert_eq!(offset_of!(Overlapped, offset), 2 * size_of::<usize>());
    assert_eq!(offset_of!(Overlapped, event), if wide { 24 } else { 16 });
    assert_eq!(size_of::<SecurityAttributes>(), if wide { 24 } else { 12 });
    assert_eq!(
        offset_of!(SecurityAttributes, descriptor),
        if wide { 8 } else { 4 }
    );
    assert_eq!(
        offset_of!(SecurityAttributes, inherit),
        if wide { 16 } else { 8 }
    );
}
#[test]
fn read_returns_wouldblock_then_delivers_partial_cached_result_without_resubmission() {
    let (mut reader, _writer) = pair();
    let mut full = [0u8; 40];
    assert_eq!(
        reader.read(&mut full).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        reader.read(&mut full).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.payload = b"abcdef".to_vec();
        m.completion = Some(Ok(6));
    });
    let mut short = [0u8; 2];
    assert_eq!(reader.read(&mut short).unwrap(), 2);
    assert_eq!(&short, b"ab");
    assert_eq!(reader.read(&mut full).unwrap(), 4);
    assert_eq!(&full[..4], b"cdef");
    assert_eq!(
        api::MOCK.with(|m| m.borrow().calls.iter().filter(|&&s| s == "read").count()),
        1
    );
    assert!(api::MOCK.with(|m| m.borrow().waits.is_empty()));
}
#[test]
fn pending_write_requires_original_prefix_and_acknowledges_only_completed_count() {
    let (_reader, mut writer) = pair();
    let bytes = b"0123456789";
    assert_eq!(
        writer.write(bytes).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        writer.write(b"changed").unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(
        writer.flush().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(
        writer.write(bytes).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    api::MOCK.with(|m| m.borrow_mut().completion = Some(Ok(3)));
    assert_eq!(writer.write(bytes).unwrap(), 3);
    assert_eq!(
        api::MOCK.with(|m| m.borrow().writes.clone()),
        [bytes.to_vec()]
    );
    assert_eq!(
        writer.write(&bytes[3..]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    api::MOCK.with(|m| m.borrow_mut().completion = Some(Ok(7)));
    assert_eq!(writer.write(&bytes[3..]).unwrap(), 7);
    assert!(writer.flush().is_ok());
}
#[test]
fn cancellation_pending_retains_owner_and_close_can_be_retried() {
    let (mut reader, _writer) = pair();
    reader.read(&mut [0u8; 24]).unwrap_err();
    api::MOCK.with(|m| m.borrow_mut().defer_cancel = true);
    let report = reader.close().unwrap();
    assert!(report.completion_pending);
    assert!(!report.complete());
    assert!(!api::MOCK.with(|m| m.borrow().closes.contains(&100)));
    assert_eq!(
        reader.read(&mut [0u8; 24]).unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    api::MOCK.with(|m| m.borrow_mut().defer_cancel = false);
    assert!(reader.close().unwrap().complete());
    assert_eq!(
        api::MOCK.with(|m| m.borrow().closes.iter().filter(|&&h| h == 100).count()),
        1
    );
    assert!(api::MOCK.with(|m| m.borrow().waits.iter().all(|&ms| ms <= 20)));
}
#[test]
fn ambiguous_completion_error_keeps_flight_for_cancellation() {
    let (mut reader, _writer) = pair();
    reader.read(&mut [0u8; 24]).unwrap_err();
    api::MOCK.with(|m| {
        let mut m = m.borrow_mut();
        m.completion = Some(Err(6));
        m.error_signaled = false;
    });
    assert_eq!(
        reader.read(&mut [0u8; 24]).unwrap_err().raw_os_error(),
        Some(6)
    );
    assert!(reader.0.flight.is_some());
    assert!(reader.close().unwrap().complete());
    assert!(api::MOCK.with(|m| m.borrow().calls.contains(&"cancel")));
}
#[test]
fn broken_pipe_completion_is_eof_and_excessive_completion_is_terminal() {
    for code in [109, 0] {
        std::thread::spawn(move || {
            let (mut reader, _writer) = pair();
            reader.read(&mut [0u8; 24]).unwrap_err();
            api::MOCK.with(|m| {
                m.borrow_mut().completion = Some(if code == 109 { Err(109) } else { Ok(65) })
            });
            if code == 109 {
                assert_eq!(reader.read(&mut [0u8; 24]).unwrap(), 0);
                assert_eq!(reader.read(&mut [0u8; 24]).unwrap(), 0);
            } else {
                assert_eq!(
                    reader.read(&mut [0u8; 24]).unwrap_err().kind(),
                    io::ErrorKind::InvalidData
                );
            }
            assert!(reader.close().unwrap().complete());
        })
        .join()
        .unwrap();
    }
}
#[test]
fn creation_failures_roll_back_only_owned_resources() {
    for fail in [
        "rng",
        "security",
        "server",
        "client",
        "event",
        "connect",
        "free-security",
    ] {
        std::thread::spawn(move || {
            api::MOCK.with(|m| m.borrow_mut().fail = Some(fail));
            assert!(PipeReader::pair().is_err());
            let closes = api::MOCK.with(|m| m.borrow().closes.clone());
            if fail == "client" || fail == "event" {
                assert!(closes.contains(&100));
            }
            if fail == "connect" || fail == "free-security" {
                assert!(closes.contains(&100) && closes.contains(&200));
            }
        })
        .join()
        .unwrap();
    }
}
#[test]
fn callback_rejects_construction_and_io_without_native_calls() {
    let (mut reader, mut writer) = pair();
    let before = api::MOCK.with(|m| m.borrow().calls.len());
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(PipeReader::pair().is_err());
        assert!(reader.read(&mut [0u8; 1]).is_err());
        assert!(writer.write(b"a").is_err());
        assert!(writer.flush().is_err());
        assert!(reader.close().is_err());
        assert!(writer.close().is_err());
    });
    assert_eq!(api::MOCK.with(|m| m.borrow().calls.len()), before);
}
#[test]
fn unresolved_drop_keeps_pending_memory_and_handles_and_poisons_new_pair() {
    let (mut reader, writer) = pair();
    reader.read(&mut [0u8; 24]).unwrap_err();
    api::MOCK.with(|m| m.borrow_mut().defer_cancel = true);
    drop(reader);
    assert!(POISONED.get());
    assert!(PipeReader::pair().is_err());
    assert!(!api::MOCK.with(|m| m.borrow().closes.contains(&100)));
    drop(writer);
}
#[test]
fn pending_connect_finishes_before_read_can_start() {
    api::MOCK.with(|m| m.borrow_mut().connect_pending = true);
    let (mut reader, _writer) = pair();
    assert_eq!(
        reader.read(&mut [0u8; 24]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    api::MOCK.with(|m| m.borrow_mut().completion = Some(Ok(73)));
    assert_eq!(
        reader.read(&mut [0u8; 24]).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(!api::MOCK.with(|m| m.borrow().calls.contains(&"read")));
    reader.read(&mut [0u8; 24]).unwrap_err();
    assert!(api::MOCK.with(|m| m.borrow().calls.contains(&"read")));
}

#[test]
fn cancel_and_handle_close_errors_are_reported_and_resources_stay_retryable() {
    let (mut reader, _writer) = pair();
    reader.read(&mut [0u8; 24]).unwrap_err();
    api::MOCK.with(|m| m.borrow_mut().fail = Some("cancel"));
    let first = reader.close().unwrap();
    assert_eq!(first.cancel_error, Some(5));
    assert!(first.completion_pending);
    assert!(reader.0.flight.is_some());
    api::MOCK.with(|m| m.borrow_mut().fail = Some("close"));
    let second = reader.close().unwrap();
    assert!(!second.completion_pending);
    assert_eq!(second.pipe_close_error, Some(5));
    assert_eq!(second.event_close_error, Some(5));
    assert!(!reader.0.handle.is_null() && !reader.0.event.is_null());
    api::MOCK.with(|m| m.borrow_mut().fail = None);
    assert!(reader.close().unwrap().complete());
    assert!(reader.0.handle.is_null() && reader.0.event.is_null());
    assert_eq!(first.cancel_error, Some(5));
}
