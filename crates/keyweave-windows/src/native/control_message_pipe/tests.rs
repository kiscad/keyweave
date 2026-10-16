#![allow(clippy::unwrap_used)]
use super::*;
use api::{MOCK, Mock, Mode};
const NAME: &str = r"\\.\pipe\keyweave-control-0123456789abcdef0123456789abcdef";
fn reset(change: impl FnOnce(&mut Mock)) {
    POISONED.set(false);
    super::super::control_identity::reset_pipe_test_identity();
    MOCK.with(|m| {
        let mut state = Mock::fresh();
        change(&mut state);
        *m.borrow_mut() = state;
    });
}
fn server() -> ControlMessagePipe {
    ControlMessagePipe::listen(NAME, &ControlIdentity::capture().unwrap()).unwrap()
}
fn change(f: impl FnOnce(&mut Mock)) {
    MOCK.with(|m| f(&mut m.borrow_mut()));
}
#[test]
fn immediate_message_io_keeps_boundaries_and_rearms_only_after_disconnect() {
    reset(|m| m.payload = vec![7; 32]);
    let mut pipe = server();
    let mut out = [0; MESSAGE_CAPACITY];
    assert!(matches!(pipe.poll_accept(), Poll::Ready(Ok(()))));
    assert_eq!(
        pipe.poll_read(&mut out).map(|r| r.unwrap()),
        Poll::Ready(ReadResult::Message {
            length: 32,
            more: false
        })
    );
    assert_eq!(&out[..32], &[7; 32]);
    assert!(matches!(pipe.poll_write(&[8; 64]), Poll::Ready(Ok(()))));
    assert!(matches!(pipe.poll_disconnect(), Poll::Ready(Ok(()))));
    assert!(matches!(pipe.poll_accept(), Poll::Ready(Ok(()))));
    assert!(pipe.close().complete());
    MOCK.with(|m| {
        let m = m.borrow();
        assert!(!m.calls.contains(&"result"));
        assert_eq!(m.writes, vec![vec![8; 64]]);
    });
}
#[test]
fn pending_storage_survives_owner_moves_and_preserves_more_data() {
    reset(|m| {
        m.read = Mode::Pending;
        m.payload = vec![3; 200];
    });
    let mut pipe = ControlMessagePipe::open(NAME).unwrap();
    let mut out = [9; MESSAGE_CAPACITY];
    assert!(pipe.poll_read(&mut out).is_pending());
    assert_eq!(out, [9; MESSAGE_CAPACITY]);
    let mut pipe = Box::new(pipe);
    assert!(pipe.poll_read(&mut out).is_pending());
    change(|m| m.completion = Some((234, 65)));
    assert_eq!(
        pipe.poll_read(&mut out).map(|r| r.unwrap()),
        Poll::Ready(ReadResult::Message {
            length: 65,
            more: true
        })
    );
    assert_eq!(out, [3; 65]);
    assert!(pipe.close().complete());
}
#[test]
fn immediate_more_data_empty_message_and_broken_peer_are_distinct() {
    for (payload, expected) in [
        (
            vec![4; 200],
            ReadResult::Message {
                length: 65,
                more: true,
            },
        ),
        (
            vec![],
            ReadResult::Message {
                length: 0,
                more: false,
            },
        ),
    ] {
        reset(|m| m.payload = payload);
        let mut pipe = ControlMessagePipe::open(NAME).unwrap();
        assert_eq!(
            pipe.poll_read(&mut [0; 65]).map(|r| r.unwrap()),
            Poll::Ready(expected)
        );
        assert!(pipe.close().complete());
    }
    for code in [109, 232, 233] {
        reset(|m| m.read = Mode::Error(code));
        let mut pipe = ControlMessagePipe::open(NAME).unwrap();
        assert_eq!(
            pipe.poll_read(&mut [0; 65]).map(|r| r.unwrap()),
            Poll::Ready(ReadResult::Disconnected)
        );
        assert!(pipe.close().complete());
    }
}
#[test]
fn changed_pending_write_and_short_or_excess_counts_do_not_issue_another_write() {
    reset(|m| m.write = Mode::Pending);
    let mut pipe = ControlMessagePipe::open(NAME).unwrap();
    assert!(pipe.poll_write(&[1; 32]).is_pending());
    assert!(matches!(pipe.poll_write(&[2; 32]), Poll::Ready(Err(_))));
    assert!(pipe.poll_write(&[1; 32]).is_pending());
    change(|m| m.completion = Some((0, 32)));
    assert!(matches!(pipe.poll_write(&[1; 32]), Poll::Ready(Ok(()))));
    MOCK.with(|m| assert_eq!(m.borrow().writes.len(), 1));
    assert!(pipe.close().complete());
    for count in [0, 31, 33] {
        reset(|m| m.immediate_count = Some(count));
        let mut pipe = ControlMessagePipe::open(NAME).unwrap();
        assert!(matches!(pipe.poll_write(&[1; 32]), Poll::Ready(Err(_))));
        assert!(pipe.close().complete());
    }
    reset(|m| m.immediate_count = Some(66));
    let mut pipe = ControlMessagePipe::open(NAME).unwrap();
    assert!(matches!(pipe.poll_read(&mut [0; 65]), Poll::Ready(Err(_))));
    assert!(pipe.close().complete());
}
#[test]
fn cancel_not_found_or_success_does_not_allow_reset_disconnect_or_free_before_signal() {
    for code in [None, Some(1168)] {
        reset(|m| {
            m.read = Mode::Pending;
            m.defer_cancel = true;
            m.cancel_code = code;
        });
        let mut pipe = server();
        assert!(matches!(pipe.poll_accept(), Poll::Ready(Ok(()))));
        assert!(pipe.poll_read(&mut [0; 65]).is_pending());
        assert!(pipe.poll_disconnect().is_pending());
        assert!(pipe.poll_disconnect().is_pending());
        MOCK.with(|m| {
            let m = m.borrow();
            assert!(!m.calls.contains(&"disconnect"));
            assert!(m.closes.is_empty());
            assert_eq!(m.calls.iter().filter(|c| **c == "cancel").count(), 1);
        });
        assert!(pipe.close().completion_pending);
        change(|m| m.completion = Some((995, 0)));
        // Completion is observed by the private event during cleanup. The mock
        // can finish the canceled operation without borrowing any caller slice.
        change(|m| {
            m.defer_cancel = false;
        });
        api::finish_pending();
        assert!(pipe.close().complete());
    }
}
#[test]
fn connect_pending_and_already_connected_ignore_undefined_connect_byte_count() {
    reset(|m| m.connect = Mode::Pending);
    let mut pipe = server();
    assert!(pipe.poll_accept().is_pending());
    change(|m| m.completion = Some((0, u32::MAX)));
    assert!(matches!(pipe.poll_accept(), Poll::Ready(Ok(()))));
    assert!(pipe.close().complete());
    reset(|m| m.connect = Mode::Error(535));
    let mut pipe = server();
    assert!(matches!(pipe.poll_accept(), Poll::Ready(Ok(()))));
    assert!(pipe.close().complete());
}
#[test]
fn factory_rejects_names_modes_inheritance_and_closes_each_owned_resource_on_failure() {
    for bad in [
        r"\\server\pipe\keyweave-control-0123456789abcdef0123456789abcdef",
        r"\\.\pipe\other",
        r"\\.\pipe\keyweave-control-0123456789ABCDEF0123456789ABCDEF",
    ] {
        reset(|_| {});
        assert!(ControlMessagePipe::open(bad).is_err());
        MOCK.with(|m| assert!(m.borrow().calls.is_empty()));
    }
    for fail in ["create-event", "set-mode", "info", "state", "inheritance"] {
        reset(|m| m.fail = vec![fail]);
        let failure = ControlMessagePipe::open(NAME).unwrap_err();
        assert!(failure.cleanup.complete());
    }
    for kind in 0..3 {
        reset(|m| match kind {
            0 => m.flags = Some(8),
            1 => m.mode = Some(0),
            _ => m.inheritance = 1,
        });
        let failure = ControlMessagePipe::open(NAME).unwrap_err();
        assert!(failure.cleanup.complete());
    }
}
#[test]
fn cleanup_preserves_independent_errors_and_unknown_drop_poison_blocks_new_factories() {
    reset(|m| m.fail = vec!["close-pipe", "close-event"]);
    let mut pipe = ControlMessagePipe::open(NAME).unwrap();
    let report = pipe.close();
    assert_eq!(report.pipe_close_error, Some(5));
    assert_eq!(report.event_close_error, Some(5));
    assert!(!report.resources_released);
    change(|m| m.fail.clear());
    let report = pipe.close();
    assert!(report.resources_released);
    assert!(!report.complete());
    drop(pipe);
    assert!(ControlMessagePipe::open(NAME).is_err());
    reset(|m| {
        m.read = Mode::Pending;
        m.defer_cancel = true;
    });
    let mut pipe = ControlMessagePipe::open(NAME).unwrap();
    assert!(pipe.poll_read(&mut [0; 65]).is_pending());
    drop(pipe);
    let before = MOCK.with(|m| m.borrow().calls.len());
    assert!(ControlMessagePipe::open(NAME).is_err());
    assert_eq!(before, MOCK.with(|m| m.borrow().calls.len()));
}
#[test]
fn ambiguous_completion_error_retains_flight_and_all_cleanup_errors_are_reported() {
    reset(|m| {
        m.read = Mode::Pending;
        m.signal_errors = false;
        m.defer_cancel = true;
    });
    let mut pipe = ControlMessagePipe::open(NAME).unwrap();
    assert!(pipe.poll_read(&mut [0; 65]).is_pending());
    change(|m| m.completion = Some((5, 0)));
    assert!(matches!(pipe.poll_read(&mut [0; 65]), Poll::Ready(Err(_))));
    assert!(pipe.flight.is_some());
    change(|m| m.cancel_code = Some(5));
    let report = pipe.close();
    assert_eq!(report.cancel_error, Some(5));
    assert!(report.completion_pending);
    change(|m| m.fail = vec!["wait"]);
    let report = pipe.close();
    assert_eq!(report.wait_error, Some(5));
    assert_eq!(report.cancel_error, Some(5));
    assert!(report.completion_pending);
}
#[test]
fn callback_io_factory_and_drop_never_call_native_apis() {
    reset(|m| m.read = Mode::Pending);
    let mut pipe = ControlMessagePipe::open(NAME).unwrap();
    assert!(pipe.poll_read(&mut [0; 65]).is_pending());
    let before = MOCK.with(|m| m.borrow().calls.len());
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(matches!(pipe.poll_read(&mut [0; 65]), Poll::Ready(Err(_))));
        assert!(ControlMessagePipe::open(NAME).is_err());
        drop(pipe);
    });
    assert_eq!(before, MOCK.with(|m| m.borrow().calls.len()));
    assert!(ControlMessagePipe::open(NAME).is_err());
    reset(|_| {});
    let mut closed = ControlMessagePipe::open(NAME).unwrap();
    assert!(closed.close().complete());
    let before = MOCK.with(|m| m.borrow().calls.len());
    super::super::hook_thread::with_callback_borrow(|| drop(closed));
    assert_eq!(before, MOCK.with(|m| m.borrow().calls.len()));
    assert!(!POISONED.get());
    assert!(ControlMessagePipe::open(NAME).unwrap().close().complete());
}

#[test]
fn actual_transaction_machine_runs_native_owner_path_with_synthetic_message_authentication() {
    use crate::session_control::{Server, ServerStep, ServerTransport};
    use keyweave_progress::session_control::{Command, Outcome, Phase, Reply, Request, Snapshot};
    use std::time::Duration;
    // Synthetic authentication is deliberate: no production trusted-transport
    // implementation is exported until actual message-token attestation exists.
    struct Fixture(ControlMessagePipe);
    impl ServerTransport for Fixture {
        fn accept(&mut self) -> Poll<io::Result<()>> {
            self.0.poll_accept()
        }
        fn authorize(&mut self) -> Poll<io::Result<bool>> {
            MOCK.with(|m| assert!(m.borrow().calls.contains(&"read")));
            Poll::Ready(Ok(true))
        }
        fn read(&mut self, out: &mut [u8; 65]) -> Poll<io::Result<ReadResult>> {
            self.0.poll_read(out)
        }
        fn write(&mut self, bytes: &[u8; 64]) -> Poll<io::Result<()>> {
            self.0.poll_write(bytes)
        }
        fn disconnect(&mut self) -> Poll<io::Result<()>> {
            self.0.poll_disconnect()
        }
    }
    let request = Request {
        id: 7,
        command: Command::Status,
    };
    reset(|m| m.payload = request.encode().unwrap().to_vec());
    let mut fixture = Fixture(server());
    let mut machine = Server::new();
    let mut handled = 0;
    let mut finished = None;
    for tick in 0..10 {
        let step = machine
            .poll(&mut fixture, Duration::from_millis(tick), |r| {
                handled += 1;
                assert_eq!(r, request);
                Reply {
                    id: r.id,
                    outcome: Outcome::Status,
                    snapshot: Snapshot {
                        session: 81,
                        phase: Phase::Starting,
                        progress_epoch: 0,
                        generation: None,
                        exit: None,
                        recovery_confirmed: false,
                        fault: false,
                    },
                }
            })
            .unwrap();
        if fixture.0.connected && machine.reply_written() {
            change(|m| m.read = Mode::Error(109));
        }
        if let ServerStep::Finished(end) = step {
            finished = Some(end);
            break;
        }
    }
    let end = finished.unwrap();
    assert_eq!(handled, 1);
    assert!(end.dispatched && end.reply_written);
    assert_eq!(end.reason, crate::session_control::EndReason::Completed);
    MOCK.with(|m| {
        let m = m.borrow();
        assert_eq!(m.writes.len(), 1);
        let value = Reply::decode(&m.writes[0]).unwrap();
        assert!(value.matches(request));
        assert_eq!(value.snapshot.progress_epoch, 0);
    });
    assert!(fixture.0.close().complete());
}
#[test]
fn factory_primary_error_and_both_close_failures_remain_independent() {
    reset(|m| m.fail = vec!["state", "close-pipe", "close-event"]);
    let failure = ControlMessagePipe::open(NAME).unwrap_err();
    assert_eq!(failure.operation.raw_os_error(), Some(5));
    assert_eq!(failure.cleanup.pipe_close_error, Some(5));
    assert_eq!(failure.cleanup.event_close_error, Some(5));
    assert!(!failure.cleanup.resources_released);
    assert!(POISONED.get());
}

#[test]
fn message_owner_ffi_layouts_match_windows_sdk_contract_for_pointer_width() {
    use std::mem::{align_of, offset_of, size_of};
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
