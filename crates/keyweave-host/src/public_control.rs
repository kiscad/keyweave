//! Shared public control state and bounded Windows endpoint metadata. No I/O.
pub use keyweave_progress::session_control::WindowsEndpoint;
use keyweave_progress::session_control::{
    Command, InvalidFrame, Outcome, Phase, Reply, Request, Snapshot,
};
/// Public session facts; neither a worker progress source nor an exit observer.
pub struct PublicSession {
    snapshot: Snapshot,
    stop: bool,
}
impl PublicSession {
    pub fn new(snapshot: Snapshot) -> Result<Self, InvalidFrame> {
        snapshot.validate()?;
        Ok(Self {
            snapshot,
            stop: false,
        })
    }
    pub fn observe(&mut self, snapshot: Snapshot) -> Result<(), InvalidFrame> {
        snapshot.validate()?;
        if snapshot.session != self.snapshot.session
            || snapshot.progress_epoch < self.snapshot.progress_epoch
            || self.snapshot.phase == Phase::Exited
                && (snapshot.phase != Phase::Exited || snapshot.exit != self.snapshot.exit)
        {
            return Err(InvalidFrame);
        }
        self.snapshot = snapshot;
        Ok(())
    }
    pub fn session(&self) -> u64 {
        self.snapshot.session
    }
    pub fn stop_requested(&self) -> bool {
        self.stop
    }
    /// Input is already decoded by the framing machine. Invalid direct calls
    /// never queue Stop and produce a reply the encoder rejects.
    pub fn handle(&mut self, request: Request) -> Reply {
        if request.encode().is_err() {
            return Reply {
                id: 0,
                outcome: Outcome::Status,
                snapshot: self.snapshot,
            };
        }
        let outcome = match request.command {
            Command::Status => Outcome::Status,
            Command::Stop { session } if session != self.snapshot.session => Outcome::StaleSession,
            Command::Stop { .. } => {
                self.stop = true;
                Outcome::StopAccepted
            }
        };
        let mut snapshot = self.snapshot;
        if self.stop && snapshot.phase != Phase::Exited {
            snapshot.phase = Phase::StopRequested;
        }
        Reply {
            id: request.id,
            outcome,
            snapshot,
        }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            session: 8,
            phase: Phase::Starting,
            progress_epoch: 0,
            generation: None,
            exit: None,
            recovery_confirmed: false,
            fault: false,
        }
    }
    #[test]
    fn endpoint_roundtrip_keeps_full_width_and_rejects_remote_injection_and_overflow() {
        let name = r"\\.\pipe\keyweave-control-0123456789abcdef0123456789abcdef";
        let endpoint = WindowsEndpoint::new(name, u32::MAX, u64::MAX).unwrap();
        assert_eq!(
            endpoint.to_string().parse::<WindowsEndpoint>().unwrap(),
            endpoint
        );
        for value in [
            format!("{name}|0|1"),
            format!("{name}|1|0"),
            format!("{name}|4294967296|1"),
            format!("{name}|1|18446744073709551616"),
            format!("{name}|+1|2"),
            format!("{name}|1|2|3"),
            format!("{name}|1|2\n"),
            "x".repeat(129),
        ] {
            assert!(value.parse::<WindowsEndpoint>().is_err());
        }
        assert!(
            WindowsEndpoint::new(
                r"\\remote\pipe\keyweave-control-0123456789abcdef0123456789abcdef",
                1,
                1
            )
            .is_err()
        );
    }
    #[test]
    fn stop_nonce_is_idempotent_and_does_not_create_exit_progress_generation_or_recovery() {
        let mut session = PublicSession::new(snapshot()).unwrap();
        let stale = session.handle(Request {
            id: 1,
            command: Command::Stop { session: 9 },
        });
        assert_eq!(stale.outcome, Outcome::StaleSession);
        assert!(!session.stop_requested());
        for id in 2..5 {
            let request = Request {
                id,
                command: Command::Stop { session: 8 },
            };
            let reply = session.handle(request);
            assert!(reply.matches(request));
            assert_eq!(reply.snapshot.phase, Phase::StopRequested);
            assert_eq!(reply.snapshot.progress_epoch, 0);
            assert!(
                reply.snapshot.generation.is_none()
                    && reply.snapshot.exit.is_none()
                    && !reply.snapshot.recovery_confirmed
            );
            reply.encode().unwrap();
        }
        assert!(session.stop_requested());
        let mut exited = snapshot();
        exited.phase = Phase::Exited;
        exited.exit = Some(keyweave_progress::session_control::Exit::Code(u32::MAX));
        session.observe(exited).unwrap();
        assert!(session.observe(snapshot()).is_err());
        assert_eq!(
            session
                .handle(Request {
                    id: 6,
                    command: Command::Status
                })
                .snapshot,
            exited
        );
    }
    #[test]
    fn invalid_snapshot_or_direct_request_cannot_replace_session_or_queue_stop() {
        let mut state = PublicSession::new(snapshot()).unwrap();
        let mut wrong = snapshot();
        wrong.session = 9;
        assert!(state.observe(wrong).is_err());
        wrong = snapshot();
        wrong.phase = Phase::Active;
        assert!(state.observe(wrong).is_err());
        assert!(
            state
                .handle(Request {
                    id: 0,
                    command: Command::Stop { session: 8 }
                })
                .encode()
                .is_err()
        );
        assert!(
            state
                .handle(Request {
                    id: 1,
                    command: Command::Stop { session: 0 }
                })
                .encode()
                .is_err()
        );
        assert!(!state.stop_requested());
        assert_eq!(
            state
                .handle(Request {
                    id: 1,
                    command: Command::Status
                })
                .snapshot,
            snapshot()
        );
    }
}
