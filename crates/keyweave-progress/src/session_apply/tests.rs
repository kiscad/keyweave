#![allow(clippy::unwrap_used)]
use super::*;
fn request() -> Request {
    Request {
        id: 7,
        session: 19,
        base: 3,
        target: 4,
        byte_length: 3,
    }
}
fn snapshot() -> Snapshot {
    Snapshot {
        session: 19,
        phase: Phase::Active,
        progress_epoch: 12,
        generation: Some(3),
        exit: None,
        recovery_confirmed: false,
        fault: false,
    }
}
fn transaction() -> Transaction<u64> {
    Transaction::new(request(), Box::new(*b"abc"), MonoTime(10), 100).unwrap()
}
#[derive(Default)]
struct Peer {
    calls: usize,
    polls: usize,
    admission: Option<Result<Admission<u64>, BackendError>>,
    completion: Option<Result<Option<Completion<u64>>, BackendError>>,
    panic_submit: bool,
    panic_poll: bool,
}
impl Backend for Peer {
    type Ticket = u64;
    fn submit(
        &mut self,
        body: Box<[u8]>,
        base: u64,
        target: u64,
    ) -> Result<Admission<u64>, BackendError> {
        self.calls += 1;
        assert_eq!(&*body, b"abc");
        assert_eq!((base, target), (3, 4));
        assert!(!self.panic_submit, "synthetic admission panic");
        self.admission.take().unwrap_or(Ok(Admission::Queued(81)))
    }
    fn completion(&mut self, ticket: &u64) -> Result<Option<Completion<u64>>, BackendError> {
        self.polls += 1;
        assert_eq!(*ticket, 81);
        assert!(!self.panic_poll, "synthetic completion panic");
        self.completion.take().unwrap_or(Ok(None))
    }
}
#[test]
fn public_wire_roundtrip_reserved_magic_and_request_fences() {
    let request = request();
    let bytes = request.encode().unwrap();
    assert_eq!(&bytes[..8], b"KWAQ\x01\0\0\0");
    assert_eq!(&bytes[8..16], &7u64.to_le_bytes());
    assert_eq!(Request::decode(&bytes).unwrap(), request);
    for index in [0, 3, 4, 5, 6, 7] {
        let mut bad = bytes;
        bad[index] ^= 128;
        assert!(Request::decode(&bad).is_err());
    }
    assert!(Request::decode(&bytes[..47]).is_err());
    let mut extra = bytes.to_vec();
    extra.push(0);
    assert!(Request::decode(&extra).is_err());
    for outcome in [
        Outcome::Committed,
        Outcome::Rejected,
        Outcome::Cancelled,
        Outcome::Busy,
        Outcome::Unavailable,
        Outcome::StaleSession,
        Outcome::StaleGeneration,
    ] {
        let reply = Reply {
            id: 7,
            session: if outcome == Outcome::StaleSession {
                20
            } else {
                19
            },
            base: 3,
            target: 4,
            observed: Some(if outcome == Outcome::Committed {
                4
            } else if outcome == Outcome::StaleGeneration {
                5
            } else {
                3
            }),
            outcome,
        };
        let wire = reply.encode().unwrap();
        assert_eq!(&wire[..4], b"KWAR");
        assert_eq!(Reply::decode(&wire).unwrap(), reply);
        assert!(reply.matches(request));
        assert!(!reply.matches(Request { id: 8, ..request }));
        assert!(!reply.matches(Request {
            base: 4,
            target: 5,
            ..request
        }));
        let mut bad = wire;
        bad[6] = 1;
        assert!(Reply::decode(&bad).is_err());
    }
}
#[test]
fn invalid_bounds_and_false_success_frames_are_rejected() {
    let request = request();
    for bad in [
        Request { id: 0, ..request },
        Request {
            session: 0,
            ..request
        },
        Request {
            base: 0,
            target: 1,
            ..request
        },
        Request {
            base: u64::MAX,
            target: 0,
            ..request
        },
        Request {
            target: 5,
            ..request
        },
        Request {
            byte_length: 0,
            ..request
        },
        Request {
            byte_length: MAX_BYTES + 1,
            ..request
        },
    ] {
        assert!(bad.encode().is_err());
    }
    assert!(
        Request {
            byte_length: MAX_BYTES,
            ..request
        }
        .encode()
        .is_ok()
    );
    assert!(Transaction::<u64>::new(request, Box::new([]), MonoTime(0), 10).is_err());
    for timeout in [0, MAX_TIMEOUT_NS + 1] {
        assert!(Transaction::<u64>::new(request, Box::new(*b"abc"), MonoTime(0), timeout).is_err());
    }
    assert!(Transaction::<u64>::new(request, Box::new(*b"abc"), MonoTime(u64::MAX), 1).is_err());
    for (outcome, observed) in [
        (Outcome::Committed, Some(3)),
        (Outcome::Rejected, Some(4)),
        (Outcome::Cancelled, None),
        (Outcome::StaleGeneration, Some(3)),
    ] {
        assert!(
            Reply {
                id: 7,
                session: 19,
                base: 3,
                target: 4,
                observed,
                outcome
            }
            .encode()
            .is_err()
        );
    }
}
#[test]
fn changed_session_generation_fault_and_inactive_facts_never_dispatch() {
    for (facts, outcome) in [
        (
            Snapshot {
                session: 20,
                ..snapshot()
            },
            Outcome::StaleSession,
        ),
        (
            Snapshot {
                generation: Some(5),
                ..snapshot()
            },
            Outcome::StaleGeneration,
        ),
        (
            Snapshot {
                fault: true,
                ..snapshot()
            },
            Outcome::Unavailable,
        ),
        (
            Snapshot {
                phase: Phase::StopRequested,
                ..snapshot()
            },
            Outcome::Unavailable,
        ),
        (
            Snapshot {
                phase: Phase::Starting,
                generation: None,
                progress_epoch: 0,
                ..snapshot()
            },
            Outcome::StaleGeneration,
        ),
    ] {
        let mut tx = transaction();
        let mut peer = Peer::default();
        assert_eq!(
            tx.step(facts, &mut peer, || MonoTime(11))
                .unwrap()
                .unwrap()
                .outcome,
            outcome
        );
        assert_eq!(peer.calls, 0);
        assert_eq!(peer.polls, 0);
        assert!(!tx.may_have_applied());
    }
    let mut tx = transaction();
    let mut peer = Peer::default();
    assert_eq!(
        tx.step(
            Snapshot {
                session: 0,
                ..snapshot()
            },
            &mut peer,
            || MonoTime(11)
        )
        .unwrap_err()
        .error,
        Error::Session
    );
    assert_eq!(peer.calls, 0);
}
#[test]
fn pending_admission_and_completion_are_distinct_and_finished_reply_is_frozen() {
    let mut tx = transaction();
    let mut peer = Peer::default();
    assert!(
        tx.step(snapshot(), &mut peer, || MonoTime(11))
            .unwrap()
            .is_none()
    );
    assert!(tx.may_have_applied());
    assert_eq!((peer.calls, peer.polls), (1, 0));
    assert!(
        tx.step(snapshot(), &mut peer, || MonoTime(12))
            .unwrap()
            .is_none()
    );
    peer.completion = Some(Ok(Some(Completion {
        ticket: 81,
        outcome: reload::Outcome::Accepted,
        generation: 4,
    })));
    let reply = tx
        .step(snapshot(), &mut peer, || MonoTime(13))
        .unwrap()
        .unwrap();
    assert_eq!(reply.outcome, Outcome::Committed);
    assert_eq!(reply.observed, Some(4));
    assert_eq!((peer.calls, peer.polls), (1, 2));
    assert_eq!(
        tx.step(snapshot(), &mut peer, || panic!(
            "finished facts need no new clock"
        ))
        .unwrap(),
        Some(reply)
    );
    assert_eq!((peer.calls, peer.polls), (1, 2));
}
#[test]
fn definitive_admission_refusals_are_not_worker_commits() {
    for (refusal, outcome) in [
        (Refusal::Busy, Outcome::Busy),
        (Refusal::Unavailable, Outcome::Unavailable),
        (Refusal::Rejected, Outcome::Rejected),
    ] {
        let mut tx = transaction();
        let mut peer = Peer {
            admission: Some(Ok(Admission::NotSubmitted(refusal))),
            ..Default::default()
        };
        let reply = tx
            .step(snapshot(), &mut peer, || MonoTime(11))
            .unwrap()
            .unwrap();
        assert_eq!(reply.outcome, outcome);
        assert_eq!(reply.observed, Some(3));
        assert!(!tx.may_have_applied());
        assert_eq!((peer.calls, peer.polls), (1, 0));
    }
}
#[test]
fn completion_ticket_and_generation_must_both_match() {
    for outcome in [
        reload::Outcome::Accepted,
        reload::Outcome::Rejected,
        reload::Outcome::Cancelled,
    ] {
        let expected = if outcome == reload::Outcome::Accepted {
            4
        } else {
            3
        };
        for (ticket, generation, ok) in [
            (81, expected, true),
            (82, expected, false),
            (81, expected + 1, false),
        ] {
            let mut tx = transaction();
            let mut peer = Peer::default();
            tx.step(snapshot(), &mut peer, || MonoTime(11)).unwrap();
            peer.completion = Some(Ok(Some(Completion {
                ticket,
                outcome,
                generation,
            })));
            let result = tx.step(snapshot(), &mut peer, || MonoTime(12));
            if ok {
                assert!(result.unwrap().unwrap().matches(request()));
                assert_eq!(tx.may_have_applied(), outcome == reload::Outcome::Accepted);
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    Failure {
                        error: Error::Completion,
                        may_have_applied: true
                    }
                );
                assert!(tx.step(snapshot(), &mut peer, || MonoTime(13)).is_err());
            }
            assert_eq!((peer.calls, peer.polls), (1, 1));
        }
    }
}
#[test]
fn absolute_clock_checks_cover_before_and_after_each_backend_call() {
    let mut tx = transaction();
    let mut peer = Peer::default();
    assert_eq!(
        tx.step(snapshot(), &mut peer, || MonoTime(110))
            .unwrap_err(),
        Failure {
            error: Error::Expired,
            may_have_applied: false
        }
    );
    assert_eq!(peer.calls, 0);
    for times in [[11, 110], [11, 10]] {
        let mut tx = transaction();
        let mut peer = Peer::default();
        let mut times = times.into_iter();
        let error = tx
            .step(snapshot(), &mut peer, || MonoTime(times.next().unwrap()))
            .unwrap_err();
        assert!(error.may_have_applied);
        assert_eq!(peer.calls, 1);
    }
    let mut tx = transaction();
    let mut peer = Peer::default();
    tx.step(snapshot(), &mut peer, || MonoTime(11)).unwrap();
    for at in 12..110 {
        assert!(
            tx.step(snapshot(), &mut peer, || MonoTime(at))
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        tx.step(snapshot(), &mut peer, || MonoTime(110))
            .unwrap_err()
            .error,
        Error::Expired
    );
    assert_eq!(peer.calls, 1);
    let mut tx = transaction();
    let mut peer = Peer::default();
    tx.step(snapshot(), &mut peer, || MonoTime(11)).unwrap();
    peer.completion = Some(Ok(Some(Completion {
        ticket: 81,
        outcome: reload::Outcome::Accepted,
        generation: 4,
    })));
    let mut times = [109, 110].into_iter();
    assert_eq!(
        tx.step(snapshot(), &mut peer, || MonoTime(times.next().unwrap()))
            .unwrap_err(),
        Failure {
            error: Error::Expired,
            may_have_applied: true
        }
    );
}
#[test]
fn callback_panic_backend_failure_or_session_change_cannot_replay_submission() {
    for panic_poll in [false, true] {
        let mut tx = transaction();
        let mut peer = Peer {
            panic_submit: !panic_poll,
            panic_poll,
            ..Default::default()
        };
        if panic_poll {
            tx.step(snapshot(), &mut peer, || MonoTime(11)).unwrap();
        }
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tx.step(
                snapshot(),
                &mut peer,
                || MonoTime(12)
            )))
            .is_err()
        );
        assert_eq!(
            tx.step(snapshot(), &mut peer, || MonoTime(13)).unwrap_err(),
            Failure {
                error: Error::Closed,
                may_have_applied: true
            }
        );
        assert_eq!(peer.calls, 1);
        assert_eq!(peer.polls, usize::from(panic_poll));
    }
    let mut tx = transaction();
    let mut peer = Peer {
        admission: Some(Err(BackendError)),
        ..Default::default()
    };
    assert_eq!(
        tx.step(snapshot(), &mut peer, || MonoTime(11)).unwrap_err(),
        Failure {
            error: Error::Backend,
            may_have_applied: true
        }
    );
    let mut tx = transaction();
    let mut peer = Peer::default();
    tx.step(snapshot(), &mut peer, || MonoTime(11)).unwrap();
    assert_eq!(
        tx.step(
            Snapshot {
                session: 20,
                ..snapshot()
            },
            &mut peer,
            || MonoTime(12)
        )
        .unwrap_err(),
        Failure {
            error: Error::Session,
            may_have_applied: true
        }
    );
    assert_eq!((peer.calls, peer.polls), (1, 0));
}
