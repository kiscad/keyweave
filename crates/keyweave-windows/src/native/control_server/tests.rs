#![allow(clippy::unwrap_used)]
use super::super::{control_identity, control_message_pipe};
use super::*;
use crate::session_control::{EndReason, Server, ServerStep};
use keyweave_progress::session_control::{Command, Outcome, Phase, Reply, Request, Snapshot};
use std::time::Duration;
const NAME: &str = r"\\.\pipe\keyweave-control-0123456789abcdef0123456789abcdef";
#[test]
fn real_machine_adapter_owner_and_token_api_path_authorize_before_host_handler() {
    for denied in [false, true] {
        control_identity::reset_pipe_test_identity();
        control_identity::set_server_test_peer_mismatch(denied);
        let request = Request {
            id: 1,
            command: Command::Stop { session: 67 },
        };
        control_message_pipe::reset_server_test_pipe(request.encode().unwrap().to_vec());
        let mut transport = ControlServerTransport::listen(NAME).unwrap();
        let mut machine = Server::new();
        let mut handled = 0;
        let mut end = None;
        assert!(matches!(transport.write(&[0; 64]), Poll::Ready(Err(_))));
        for tick in 0..12 {
            let step = machine
                .poll(&mut transport, Duration::from_millis(tick), |r| {
                    handled += 1;
                    assert!(!denied);
                    assert!(control_identity::server_test_restored());
                    assert_eq!(r, request);
                    Reply {
                        id: r.id,
                        outcome: Outcome::StopAccepted,
                        snapshot: Snapshot {
                            session: 67,
                            phase: Phase::StopRequested,
                            progress_epoch: 0,
                            generation: None,
                            exit: None,
                            recovery_confirmed: false,
                            fault: false,
                        },
                    }
                })
                .unwrap();
            if machine.reply_written() {
                control_message_pipe::server_test_peer_closed();
            }
            if let ServerStep::Finished(value) = step {
                end = Some(value);
                break;
            }
        }
        let end = end.unwrap();
        assert_eq!(handled, usize::from(!denied));
        assert_eq!(end.dispatched, !denied);
        assert_eq!(
            end.reason,
            if denied {
                EndReason::Unauthorized
            } else {
                EndReason::Completed
            }
        );
        let writes = control_message_pipe::server_test_writes();
        assert_eq!(writes.len(), usize::from(!denied));
        if !denied {
            let reply = Reply::decode(&writes[0]).unwrap();
            assert!(reply.matches(request));
            assert_eq!(reply.snapshot.progress_epoch, 0);
            assert!(reply.snapshot.exit.is_none());
        }
        assert!(transport.close().complete());
    }
}

#[test]
fn adapter_rejects_early_truncated_and_repeated_authorization_without_dispatching() {
    for length in [0, 31, 33, 65, 200] {
        control_identity::reset_pipe_test_identity();
        control_message_pipe::reset_server_test_pipe(vec![0; length]);
        let mut transport = ControlServerTransport::listen(NAME).unwrap();
        assert!(matches!(transport.accept(), Poll::Ready(Ok(()))));
        assert!(matches!(transport.read(&mut [0; 65]), Poll::Ready(Ok(_))));
        assert!(matches!(transport.authorize(), Poll::Ready(Err(_))));
        assert!(!control_identity::server_test_restored());
        assert!(matches!(transport.write(&[0; 64]), Poll::Ready(Err(_))));
        assert!(transport.close().complete());
    }
    control_identity::reset_pipe_test_identity();
    control_message_pipe::reset_server_test_pipe(vec![0; 32]);
    let mut transport = ControlServerTransport::listen(NAME).unwrap();
    assert!(matches!(transport.accept(), Poll::Ready(Ok(()))));
    assert!(matches!(transport.authorize(), Poll::Ready(Err(_))));
    assert!(!control_identity::server_test_restored());
    assert!(transport.close().complete());
    control_identity::reset_pipe_test_identity();
    control_message_pipe::reset_server_test_pipe(vec![0; 32]);
    let mut transport = ControlServerTransport::listen(NAME).unwrap();
    assert!(matches!(transport.accept(), Poll::Ready(Ok(()))));
    assert!(matches!(transport.read(&mut [0; 65]), Poll::Ready(Ok(_))));
    assert!(matches!(transport.authorize(), Poll::Ready(Ok(true))));
    assert!(matches!(transport.authorize(), Poll::Ready(Err(_))));
    assert!(control_identity::server_test_restored());
    assert!(matches!(transport.write(&[0; 64]), Poll::Ready(Err(_))));
    assert!(transport.close().complete());
}
