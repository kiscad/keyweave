#![allow(clippy::unwrap_used)]
use super::*;
use keyweave_progress::session_control::{Command, Outcome, Phase, Snapshot};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
fn now(ms: u64) -> Duration {
    Duration::from_millis(ms)
}
fn request() -> Request {
    Request {
        id: 9,
        command: Command::Stop { session: 71 },
    }
}
fn reply(request: Request) -> Reply {
    Reply {
        id: request.id,
        outcome: match request.command {
            Command::Status => Outcome::Status,
            Command::Stop { session: 71 } => Outcome::StopAccepted,
            Command::Stop { .. } => Outcome::StaleSession,
        },
        snapshot: Snapshot {
            session: 71,
            phase: Phase::StopRequested,
            progress_epoch: 0,
            generation: None,
            exit: None,
            recovery_confirmed: false,
            fault: false,
        },
    }
}
#[derive(Default)]
struct Wire {
    connected: bool,
    client_closed: bool,
    requests: VecDeque<Vec<u8>>,
    replies: VecDeque<Vec<u8>>,
    server_calls: usize,
    client_calls: usize,
    server_authorized: bool,
    message_context: bool,
    client_authorized: bool,
    pending: bool,
    fail: bool,
    writes: Vec<Vec<u8>>,
    disconnects: usize,
}
#[derive(Clone)]
struct Transport(Rc<RefCell<Wire>>);
impl Transport {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(Wire {
            server_authorized: true,
            client_authorized: true,
            ..Wire::default()
        })))
    }
    fn operation<T>(
        &self,
        server: bool,
        f: impl FnOnce(&mut Wire) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        let mut wire = self.0.borrow_mut();
        if server {
            wire.server_calls += 1;
        } else {
            wire.client_calls += 1;
        }
        if wire.fail {
            return Poll::Ready(Err(io::Error::other("transport failure")));
        }
        if wire.pending {
            return Poll::Pending;
        }
        f(&mut wire)
    }
}
fn read(
    message: Option<Vec<u8>>,
    closed: bool,
    buffer: &mut [u8; MESSAGE_CAPACITY],
) -> Poll<io::Result<ReadResult>> {
    if let Some(message) = message {
        let len = message.len().min(buffer.len());
        buffer[..len].copy_from_slice(&message[..len]);
        Poll::Ready(Ok(ReadResult::Message {
            length: len,
            more: message.len() > len,
        }))
    } else if closed {
        Poll::Ready(Ok(ReadResult::Disconnected))
    } else {
        Poll::Pending
    }
}
impl ServerTransport for Transport {
    fn accept(&mut self) -> Poll<io::Result<()>> {
        self.operation(true, |w| {
            if w.connected {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
    }
    fn authorize(&mut self) -> Poll<io::Result<bool>> {
        self.operation(true, |w| {
            assert!(
                w.message_context,
                "authorization requires last-read message context"
            );
            Poll::Ready(Ok(w.server_authorized))
        })
    }
    fn read(&mut self, b: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>> {
        self.operation(true, |w| {
            let result = read(w.requests.pop_front(), w.client_closed, b);
            if matches!(result, Poll::Ready(Ok(ReadResult::Message { .. }))) {
                w.message_context = true;
            }
            result
        })
    }
    fn write(&mut self, b: &[u8; REPLY_BYTES]) -> Poll<io::Result<()>> {
        self.operation(true, |w| {
            w.writes.push(b.to_vec());
            w.replies.push_back(b.to_vec());
            Poll::Ready(Ok(()))
        })
    }
    fn disconnect(&mut self) -> Poll<io::Result<()>> {
        self.operation(true, |w| {
            w.connected = false;
            w.message_context = false;
            w.client_closed = false;
            w.requests.clear();
            w.replies.clear();
            w.disconnects += 1;
            Poll::Ready(Ok(()))
        })
    }
}
impl ClientTransport for Transport {
    fn connect(&mut self) -> Poll<io::Result<()>> {
        self.operation(false, |w| {
            w.connected = true;
            Poll::Ready(Ok(()))
        })
    }
    fn authorize(&mut self) -> Poll<io::Result<bool>> {
        self.operation(false, |w| Poll::Ready(Ok(w.client_authorized)))
    }
    fn read(&mut self, b: &mut [u8; MESSAGE_CAPACITY]) -> Poll<io::Result<ReadResult>> {
        self.operation(false, |w| read(w.replies.pop_front(), false, b))
    }
    fn write(&mut self, b: &[u8; 32]) -> Poll<io::Result<()>> {
        self.operation(false, |w| {
            w.requests.push_back(b.to_vec());
            Poll::Ready(Ok(()))
        })
    }
    fn close(&mut self) -> Poll<io::Result<()>> {
        self.operation(false, |w| {
            w.client_closed = true;
            Poll::Ready(Ok(()))
        })
    }
}
#[test]
fn paired_machines_use_real_dtos_once_with_pending_io_and_rearm_only_after_close() {
    let mut server = Server::new();
    let mut transport = Transport::new();
    let mut calls = 0;
    for (round, req) in [
        request(),
        Request {
            id: 10,
            command: Command::Status,
        },
        Request {
            id: 11,
            command: Command::Stop { session: 72 },
        },
    ]
    .into_iter()
    .enumerate()
    {
        let start = round as u64 * 100;
        let mut client = Client::new(req, now(start)).unwrap();
        let mut client_done = false;
        let mut server_done = false;
        for step in 0..40 {
            transport.0.borrow_mut().pending = step % 3 == 0;
            let time = now(start + step);
            if !client_done {
                let before = transport.0.borrow().client_calls;
                if let ClientStep::Finished {
                    reason,
                    reply: result,
                } = client.poll(&mut transport, time).unwrap()
                {
                    assert_eq!(reason, EndReason::Completed);
                    assert_eq!(result, Some(reply(req)));
                    client_done = true;
                }
                assert!(transport.0.borrow().client_calls - before <= 1);
            }
            let before = transport.0.borrow().server_calls;
            match server
                .poll(&mut transport, time, |r| {
                    calls += 1;
                    reply(r)
                })
                .unwrap()
            {
                ServerStep::Finished(end) => {
                    assert!(client_done);
                    assert_eq!(
                        end,
                        End {
                            reason: EndReason::Completed,
                            dispatched: true,
                            reply_written: true
                        }
                    );
                    server_done = true;
                }
                ServerStep::Dispatched => assert!(!server.reply_written()),
                ServerStep::Pending => (),
            }
            assert!(transport.0.borrow().server_calls - before <= 1);
            if server_done {
                break;
            }
        }
        assert!(client_done && server_done);
        assert_eq!(calls, round + 1);
        assert!(server.next_deadline().is_none());
        assert!(!server.request_dispatched());
    }
    assert_eq!(transport.0.borrow().writes.len(), 3);
    assert_eq!(transport.0.borrow().disconnects, 3);
}
#[test]
fn malformed_request_messages_and_unauthorized_connections_never_dispatch() {
    let good = request().encode().unwrap().to_vec();
    let mut bad_magic = good.clone();
    bad_magic[0] = 0;
    let mut reserved = good.clone();
    reserved[31] = 1;
    for bytes in [
        vec![],
        good[..31].to_vec(),
        [good.clone(), vec![0]].concat(),
        vec![1; 200],
        bad_magic,
        reserved,
    ] {
        let mut t = Transport::new();
        {
            let mut w = t.0.borrow_mut();
            w.connected = true;
            w.requests.push_back(bytes);
        }
        let mut server = Server::new();
        let mut end = None;
        for time in 0..10 {
            if let ServerStep::Finished(value) = server
                .poll(&mut t, now(time), |_| panic!("must not dispatch"))
                .unwrap()
            {
                end = Some(value);
                break;
            }
        }
        assert_eq!(
            end,
            Some(End {
                reason: EndReason::Malformed,
                dispatched: false,
                reply_written: false
            })
        );
        assert!(t.0.borrow().writes.is_empty());
    }
    let mut t = Transport::new();
    {
        let mut w = t.0.borrow_mut();
        w.connected = true;
        w.server_authorized = false;
        w.requests.push_back(good);
    }
    let mut server = Server::new();
    for time in 0..4 {
        let result = server
            .poll(&mut t, now(time), |_| panic!("must not dispatch"))
            .unwrap();
        if time == 3 {
            assert_eq!(
                result,
                ServerStep::Finished(End {
                    reason: EndReason::Unauthorized,
                    dispatched: false,
                    reply_written: false
                })
            );
        }
    }
}
#[test]
fn server_timeout_covers_authorize_read_dispatch_write_and_peer_close_without_renewal() {
    // Advance to each connected state, then expire exactly at the original
    // absolute deadline. Dispatch-at-deadline must not run a Stop.
    for stage in 1..=6 {
        let mut t = Transport::new();
        {
            let mut w = t.0.borrow_mut();
            w.connected = true;
            w.requests.push_back(request().encode().unwrap().to_vec());
        }
        let mut server = Server::new();
        let mut dispatched = 0;
        for time in 0..stage {
            server
                .poll(&mut t, now(time), |r| {
                    dispatched += 1;
                    reply(r)
                })
                .unwrap();
        }
        assert_eq!(server.next_deadline(), Some(now(1000)));
        let before = t.0.borrow().server_calls;
        assert_eq!(
            server
                .poll(&mut t, now(1000), |_| panic!("expired dispatch"))
                .unwrap(),
            ServerStep::Pending
        );
        assert_eq!(t.0.borrow().server_calls, before);
        assert_eq!(server.next_deadline(), Some(now(1100)));
        let end = server
            .poll(&mut t, now(1001), |_| panic!("closing"))
            .unwrap();
        assert_eq!(
            end,
            ServerStep::Finished(End {
                reason: EndReason::TimedOut,
                dispatched: dispatched > 0,
                reply_written: stage >= 5
            })
        );
    }
}
#[test]
fn second_request_after_reply_is_rejected_without_a_second_host_effect() {
    let mut t = Transport::new();
    {
        let mut w = t.0.borrow_mut();
        w.connected = true;
        w.requests.push_back(request().encode().unwrap().to_vec());
        w.requests.push_back(request().encode().unwrap().to_vec());
    }
    let mut server = Server::new();
    let mut calls = 0;
    let mut end = None;
    for time in 0..10 {
        if let ServerStep::Finished(value) = server
            .poll(&mut t, now(time), |r| {
                calls += 1;
                reply(r)
            })
            .unwrap()
        {
            end = Some(value);
            break;
        }
    }
    assert_eq!(calls, 1);
    assert_eq!(
        end,
        Some(End {
            reason: EndReason::Malformed,
            dispatched: true,
            reply_written: true
        })
    );
}
#[test]
fn invalid_or_panicking_handler_cannot_replay_a_stop_and_retains_dispatch_evidence() {
    for panic_handler in [false, true] {
        let mut t = Transport::new();
        {
            let mut w = t.0.borrow_mut();
            w.connected = true;
            w.requests.push_back(request().encode().unwrap().to_vec());
        }
        let mut server = Server::new();
        for time in 0..3 {
            server.poll(&mut t, now(time), reply).unwrap();
        }
        if panic_handler {
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.poll(
                    &mut t,
                    now(3),
                    |_| panic!("handler panic")
                )))
                .is_err()
            );
        } else {
            assert!(matches!(
                server.poll(&mut t, now(3), |r| {
                    let mut value = reply(r);
                    value.id += 1;
                    value
                }),
                Err(Error::InvalidReply)
            ));
        }
        assert!(server.request_dispatched());
        assert!(!server.reply_written());
        assert!(server.next_deadline().is_none());
        assert!(matches!(
            server.poll(&mut t, now(4), |_| panic!("must not replay")),
            Err(Error::Terminal)
        ));
        assert!(t.0.borrow().writes.is_empty());
    }
}
#[test]
fn client_requires_matching_complete_reply_and_confirmed_close() {
    let good = reply(request()).encode().unwrap().to_vec();
    let mut wrong_id = good.clone();
    wrong_id[8] += 1;
    let mut wrong_outcome = good.clone();
    wrong_outcome[5] = 1;
    let mut wrong_session = good.clone();
    wrong_session[16] += 1;
    let mut reserved = good.clone();
    reserved[63] = 1;
    for bytes in [
        vec![],
        good[..63].to_vec(),
        [good.clone(), vec![0]].concat(),
        vec![0; 200],
        wrong_id,
        wrong_outcome,
        wrong_session,
        reserved,
    ] {
        let mut t = Transport::new();
        t.0.borrow_mut().replies.push_back(bytes);
        let mut client = Client::new(request(), now(0)).unwrap();
        for time in 0..4 {
            assert_eq!(client.poll(&mut t, now(time)).unwrap(), ClientStep::Pending);
        }
        t.0.borrow_mut().pending = true;
        assert_eq!(client.poll(&mut t, now(4)).unwrap(), ClientStep::Pending);
        assert!(!t.0.borrow().client_closed);
        t.0.borrow_mut().pending = false;
        assert_eq!(
            client.poll(&mut t, now(5)).unwrap(),
            ClientStep::Finished {
                reason: EndReason::Malformed,
                reply: None
            }
        );
        assert!(t.0.borrow().client_closed);
        assert!(matches!(client.poll(&mut t, now(6)), Err(Error::Terminal)));
    }
}
#[test]
fn client_auth_failure_timeout_and_pending_close_never_extend_transaction_deadline() {
    let mut t = Transport::new();
    t.0.borrow_mut().client_authorized = false;
    let mut client = Client::new(request(), now(0)).unwrap();
    client.poll(&mut t, now(0)).unwrap();
    client.poll(&mut t, now(1)).unwrap();
    assert_eq!(
        client.poll(&mut t, now(2)).unwrap(),
        ClientStep::Finished {
            reason: EndReason::Unauthorized,
            reply: None
        }
    );
    assert!(t.0.borrow().requests.is_empty());
    for stage in 0..4 {
        let mut t = Transport::new();
        let mut client = Client::new(request(), now(0)).unwrap();
        for time in 0..stage {
            client.poll(&mut t, now(time)).unwrap();
        }
        t.0.borrow_mut().pending = true;
        client.poll(&mut t, now(999)).unwrap();
        assert_eq!(client.next_deadline(), Some(now(1000)));
        let before = t.0.borrow().client_calls;
        client.poll(&mut t, now(1000)).unwrap();
        assert_eq!(t.0.borrow().client_calls, before);
        client.poll(&mut t, now(1099)).unwrap();
        assert!(matches!(
            client.poll(&mut t, now(1100)),
            Err(Error::CloseTimeout)
        ));
        let before = t.0.borrow().client_calls;
        assert!(matches!(
            client.poll(&mut t, now(1101)),
            Err(Error::Terminal)
        ));
        assert_eq!(t.0.borrow().client_calls, before);
    }
}
#[test]
fn clock_overflow_regression_transport_error_and_unconfirmed_disconnect_are_terminal() {
    assert!(matches!(
        Client::new(request(), Duration::MAX),
        Err(Error::DeadlineOverflow)
    ));
    assert!(matches!(
        Client::new(Request { id: 0, ..request() }, now(0)),
        Err(Error::InvalidRequest)
    ));
    let mut t = Transport::new();
    let mut server = Server::new();
    server.poll(&mut t, now(2), reply).unwrap();
    assert!(matches!(
        server.poll(&mut t, now(1), reply),
        Err(Error::ClockRegression)
    ));
    assert!(matches!(
        server.poll(&mut t, now(3), reply),
        Err(Error::Terminal)
    ));
    let mut server = Server::new();
    t.0.borrow_mut().connected = true;
    assert!(matches!(
        server.poll(&mut t, Duration::MAX, reply),
        Err(Error::DeadlineOverflow)
    ));
    let mut server = Server::new();
    t.0.borrow_mut().fail = true;
    assert!(matches!(
        server.poll(&mut t, now(0), reply),
        Err(Error::Transport(_))
    ));
    t.0.borrow_mut().fail = false;
    assert!(matches!(
        server.poll(&mut t, now(1), reply),
        Err(Error::Terminal)
    ));
    let mut server = Server::new();
    server.poll(&mut t, now(0), reply).unwrap();
    server.poll(&mut t, now(1000), reply).unwrap();
    t.0.borrow_mut().pending = true;
    server.poll(&mut t, now(1099), reply).unwrap();
    let before = t.0.borrow().server_calls;
    assert!(matches!(
        server.poll(&mut t, now(1100), reply),
        Err(Error::CloseTimeout)
    ));
    assert_eq!(t.0.borrow().server_calls, before);
}

#[test]
fn transport_failure_at_every_stage_is_terminal_and_does_not_replay_host_effects() {
    for stage in [0, 1, 2, 4, 5, 6] {
        let mut t = Transport::new();
        {
            let mut w = t.0.borrow_mut();
            w.connected = true;
            w.client_closed = true;
            w.requests.push_back(request().encode().unwrap().to_vec());
        }
        let mut server = Server::new();
        let mut effects = 0;
        for time in 0..stage {
            server
                .poll(&mut t, now(time), |r| {
                    effects += 1;
                    reply(r)
                })
                .unwrap();
        }
        t.0.borrow_mut().fail = true;
        assert!(matches!(
            server.poll(&mut t, now(stage), |_| panic!(
                "IO failure must not dispatch"
            )),
            Err(Error::Transport(_))
        ));
        let before = t.0.borrow().server_calls;
        t.0.borrow_mut().fail = false;
        assert!(matches!(
            server.poll(&mut t, now(stage + 1), |_| panic!("no replay")),
            Err(Error::Terminal)
        ));
        assert_eq!(before, t.0.borrow().server_calls);
        assert_eq!(effects, usize::from(stage >= 4));
        assert_eq!(server.request_dispatched(), stage >= 4);
    }
    for stage in 0..5 {
        let mut t = Transport::new();
        t.0.borrow_mut()
            .replies
            .push_back(reply(request()).encode().unwrap().to_vec());
        let mut client = Client::new(request(), now(0)).unwrap();
        for time in 0..stage {
            client.poll(&mut t, now(time)).unwrap();
        }
        t.0.borrow_mut().fail = true;
        assert!(matches!(
            client.poll(&mut t, now(stage)),
            Err(Error::Transport(_))
        ));
        let before = t.0.borrow().client_calls;
        t.0.borrow_mut().fail = false;
        assert!(matches!(
            client.poll(&mut t, now(stage + 1)),
            Err(Error::Terminal)
        ));
        assert_eq!(before, t.0.borrow().client_calls);
    }
}
#[test]
fn disconnect_before_request_is_not_a_dispatch_and_invalid_snapshot_is_not_sent() {
    let mut t = Transport::new();
    {
        let mut w = t.0.borrow_mut();
        w.connected = true;
        w.client_closed = true;
    }
    let mut server = Server::new();
    for time in 0..3 {
        let result = server
            .poll(&mut t, now(time), |_| panic!("no request"))
            .unwrap();
        if time == 2 {
            assert_eq!(
                result,
                ServerStep::Finished(End {
                    reason: EndReason::PeerClosed,
                    dispatched: false,
                    reply_written: false
                })
            );
        }
    }
    let mut t = Transport::new();
    {
        let mut w = t.0.borrow_mut();
        w.connected = true;
        w.requests.push_back(request().encode().unwrap().to_vec());
    }
    let mut server = Server::new();
    for time in 0..3 {
        server.poll(&mut t, now(time), reply).unwrap();
    }
    assert!(matches!(
        server.poll(&mut t, now(3), |r| {
            let mut value = reply(r);
            value.snapshot.recovery_confirmed = true;
            value
        }),
        Err(Error::InvalidReply)
    ));
    assert!(server.request_dispatched());
    assert!(t.0.borrow().writes.is_empty());
}
