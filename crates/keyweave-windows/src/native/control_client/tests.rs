#![allow(clippy::unwrap_used)]
use super::super::{control_identity, control_message_pipe};
use super::*;
use crate::session_control::{Client, ClientStep, EndReason};
use keyweave_progress::session_control::{Command, Outcome, Phase, Reply, Request, Snapshot};
use std::time::Duration;
const NAME: &str = r"\\.\pipe\keyweave-control-0123456789abcdef0123456789abcdef";
fn instance() -> ControlServerInstance {
    ControlServerInstance::from_parts(42, 123456).unwrap()
}
fn reset() {
    control_identity::reset_pipe_test_identity();
    control_message_pipe::reset_server_test_pipe(vec![]);
}
fn request() -> Request {
    Request {
        id: 9,
        command: Command::Status,
    }
}
fn reply() -> Reply {
    Reply {
        id: 9,
        outcome: Outcome::Status,
        snapshot: Snapshot {
            session: 73,
            phase: Phase::Starting,
            progress_epoch: 0,
            generation: None,
            exit: None,
            recovery_confirmed: false,
            fault: false,
        },
    }
}
#[test]
fn real_client_machine_adapter_owner_and_pinned_token_path_complete_without_input() {
    reset();
    control_message_pipe::change_client_test_pipe(|m| {
        m.payload = reply().encode().unwrap().to_vec()
    });
    let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
    assert!(control_message_pipe::server_test_writes().is_empty());
    assert!(matches!(transport.write(&[0; 32]), Poll::Ready(Err(_))));
    let mut machine = Client::new(request(), Duration::ZERO).unwrap();
    let mut finished = None;
    for tick in 0..10 {
        if let ClientStep::Finished { reason, reply } = machine
            .poll(&mut transport, Duration::from_millis(tick))
            .unwrap()
        {
            finished = Some((reason, reply));
            break;
        }
    }
    assert_eq!(finished, Some((EndReason::Completed, Some(reply()))));
    assert_eq!(
        control_message_pipe::server_test_writes(),
        vec![request().encode().unwrap().to_vec()]
    );
    assert!(transport.close_resources().complete());
}
#[test]
fn busy_connect_retries_once_per_poll_and_original_absolute_deadline_stops_it() {
    reset();
    control_message_pipe::change_client_test_pipe(|m| m.fail = vec!["open-client"]);
    // Fail with PIPE_BUSY, preserving no allocated native resources.
    control_message_pipe::change_client_test_pipe(|m| m.open_error = Some(231));
    let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
    let mut machine = Client::new(request(), Duration::ZERO).unwrap();
    for tick in [0, 200, 999] {
        assert_eq!(
            machine
                .poll(&mut transport, Duration::from_millis(tick))
                .unwrap(),
            ClientStep::Pending
        );
    }
    assert_eq!(control_message_pipe::server_test_open_count(), 3);
    machine
        .poll(&mut transport, Duration::from_millis(1000))
        .unwrap();
    assert_eq!(control_message_pipe::server_test_open_count(), 3);
    assert_eq!(
        machine
            .poll(&mut transport, Duration::from_millis(1001))
            .unwrap(),
        ClientStep::Finished {
            reason: EndReason::TimedOut,
            reply: None
        }
    );
}
#[test]
fn wrong_expected_birth_or_scope_and_wrong_actual_pipe_pid_never_write() {
    for kind in 0..5 {
        reset();
        control_identity::change_client_test_identity(|m| match kind {
            0 => m.process_created += 1,
            1 => m.peer_sid = true,
            2 => m.peer_logon = true,
            3 => m.peer_session = true,
            _ => (),
        });
        if kind < 4 {
            assert!(ControlClientTransport::new(NAME, instance()).is_err());
        } else {
            control_message_pipe::change_client_test_pipe(|m| m.server_pid = Some(43));
            let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
            assert!(matches!(transport.connect(), Poll::Ready(Err(_))));
            assert!(transport.close_resources().complete());
        }
        assert!(control_message_pipe::server_test_writes().is_empty());
    }
}
#[test]
fn pinned_server_exit_token_change_or_pid_change_after_connect_rejects_request() {
    for kind in 0..4 {
        reset();
        let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
        assert!(matches!(transport.connect(), Poll::Ready(Ok(()))));
        assert!(matches!(transport.authorize(), Poll::Ready(Ok(true))));
        if kind < 3 {
            control_identity::change_client_test_identity(|m| match kind {
                0 => m.process_wait = 0,
                1 => m.peer_logon = true,
                _ => m.process_created += 1,
            });
        } else {
            control_message_pipe::change_client_test_pipe(|m| m.server_pid = Some(43));
        }
        assert!(matches!(
            transport.write(&request().encode().unwrap()),
            Poll::Ready(Err(_))
        ));
        assert!(control_message_pipe::server_test_writes().is_empty());
        assert!(transport.close_resources().complete());
    }
}

#[test]
fn malformed_reply_and_exit_after_a_submitted_request_never_report_success() {
    for exited in [false, true] {
        reset();
        let mut value = reply();
        value.id = 10;
        control_message_pipe::change_client_test_pipe(|m| {
            m.payload = value.encode().unwrap().to_vec()
        });
        let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
        let mut machine = Client::new(request(), Duration::ZERO).unwrap();
        for tick in 0..3 {
            assert_eq!(
                machine
                    .poll(&mut transport, Duration::from_millis(tick))
                    .unwrap(),
                ClientStep::Pending
            );
        }
        assert_eq!(control_message_pipe::server_test_writes().len(), 1);
        if exited {
            control_identity::change_client_test_identity(|m| m.process_wait = 0);
            assert!(
                machine
                    .poll(&mut transport, Duration::from_millis(3))
                    .is_err()
            );
        } else {
            machine
                .poll(&mut transport, Duration::from_millis(3))
                .unwrap();
            assert_eq!(
                machine
                    .poll(&mut transport, Duration::from_millis(4))
                    .unwrap(),
                ClientStep::Finished {
                    reason: EndReason::Malformed,
                    reply: None
                }
            );
        }
        assert!(transport.close_resources().complete());
    }
}
#[test]
fn pending_pipe_close_keeps_process_pin_until_completion_and_reports_all_close_errors() {
    reset();
    let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
    assert!(matches!(transport.connect(), Poll::Ready(Ok(()))));
    assert!(matches!(transport.authorize(), Poll::Ready(Ok(true))));
    control_message_pipe::client_test_pending_read();
    assert!(transport.read(&mut [0; 65]).is_pending());
    let report = transport.close_resources();
    assert!(report.pipe.unwrap().completion_pending);
    assert!(!report.peer_released);
    control_identity::change_client_test_identity(|m| {
        assert!(m.handles.values().any(|peer| *peer))
    });
    control_message_pipe::client_test_finish_pending();
    assert!(transport.close_resources().complete());
    reset();
    let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
    assert!(matches!(transport.connect(), Poll::Ready(Ok(()))));
    control_message_pipe::change_client_test_pipe(|m| m.fail = vec!["close-pipe", "close-event"]);
    let report = transport.close_resources();
    assert_eq!(report.pipe.unwrap().pipe_close_error, Some(5));
    assert_eq!(report.pipe.unwrap().event_close_error, Some(5));
    assert!(!report.peer_released);
    control_message_pipe::change_client_test_pipe(|m| m.fail.clear());
    control_identity::change_client_test_identity(|m| {
        m.fail_close = true;
        m.peer_only_failure = true;
    });
    let report = transport.close_resources();
    assert!(report.pipe.unwrap().resources_released);
    assert!(report.peer_close.is_some());
    assert!(!report.peer_released);
    control_identity::change_client_test_identity(|m| m.fail_close = false);
    let report = transport.close_resources();
    assert!(report.peer_released);
    assert!(!report.complete());
}
#[test]
fn callback_prepare_poll_and_drop_do_not_issue_native_calls_or_release_pending_storage() {
    reset();
    let mut transport = ControlClientTransport::new(NAME, instance()).unwrap();
    assert!(matches!(transport.connect(), Poll::Ready(Ok(()))));
    assert!(matches!(transport.authorize(), Poll::Ready(Ok(true))));
    control_message_pipe::client_test_pending_read();
    assert!(transport.read(&mut [0; 65]).is_pending());
    let mut identity_calls = 0;
    let mut pipe_calls = 0;
    control_identity::change_client_test_identity(|m| identity_calls = m.calls);
    control_message_pipe::change_client_test_pipe(|m| pipe_calls = m.calls.len());
    super::super::hook_thread::with_callback_borrow(|| {
        assert!(ControlClientTransport::new(NAME, instance()).is_err());
        assert!(matches!(transport.read(&mut [0; 65]), Poll::Ready(Err(_))));
        drop(transport);
    });
    control_identity::change_client_test_identity(|m| assert_eq!(m.calls, identity_calls));
    control_message_pipe::change_client_test_pipe(|m| assert_eq!(m.calls.len(), pipe_calls));
}
