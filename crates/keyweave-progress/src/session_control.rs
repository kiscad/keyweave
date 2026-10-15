//! Public local session DTOs. Distinct from private worker progress/reload IDs.
use crate::number;
pub const REQUEST_BYTES: usize = 32;
pub const REPLY_BYTES: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Stop { session: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub command: Command,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Active,
    StopRequested,
    Exited,
    Unobserved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Code(u32),
    Signal(u32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub session: u64,
    pub phase: Phase,
    pub progress_epoch: u64,
    pub generation: Option<u64>,
    pub exit: Option<Exit>,
    pub recovery_confirmed: bool,
    pub fault: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Status,
    StopAccepted,
    StaleSession,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reply {
    pub id: u64,
    pub outcome: Outcome,
    pub snapshot: Snapshot,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid public session control frame or facts")]
pub struct InvalidFrame;
impl Request {
    pub fn encode(self) -> Result<[u8; REQUEST_BYTES], InvalidFrame> {
        if self.id == 0 || matches!(self.command, Command::Stop { session: 0 }) {
            return Err(InvalidFrame);
        }
        let mut bytes = [0; REQUEST_BYTES];
        bytes[..4].copy_from_slice(b"KWCC");
        bytes[4] = 1;
        let (command, session) = match self.command {
            Command::Status => (1, 0),
            Command::Stop { session } => (2, session),
        };
        bytes[5] = command;
        bytes[8..16].copy_from_slice(&self.id.to_le_bytes());
        bytes[16..24].copy_from_slice(&session.to_le_bytes());
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, InvalidFrame> {
        if bytes.len() != REQUEST_BYTES
            || &bytes[..4] != b"KWCC"
            || bytes[4] != 1
            || bytes[6..8] != [0; 2]
            || bytes[24..] != [0; 8]
        {
            return Err(InvalidFrame);
        }
        let session = number(bytes, 16);
        let command = match (bytes[5], session) {
            (1, 0) => Command::Status,
            (2, 1..) => Command::Stop { session },
            _ => return Err(InvalidFrame),
        };
        let request = Self {
            id: number(bytes, 8),
            command,
        };
        request.encode()?;
        Ok(request)
    }
}
impl Snapshot {
    pub fn validate(self) -> Result<(), InvalidFrame> {
        if self.session == 0
            || self.generation == Some(0)
            || (self.phase == Phase::Exited) != self.exit.is_some()
            || self.recovery_confirmed && self.phase != Phase::Exited
            || matches!(self.exit, Some(Exit::Signal(0 | 65..)))
            || self.phase == Phase::Starting
                && (self.generation.is_some() || self.progress_epoch != 0)
            || self.phase == Phase::Active
                && (self.generation.is_none() || self.progress_epoch == 0)
        {
            Err(InvalidFrame)
        } else {
            Ok(())
        }
    }
}
impl Reply {
    pub fn encode(self) -> Result<[u8; REPLY_BYTES], InvalidFrame> {
        self.snapshot.validate()?;
        if self.id == 0 {
            return Err(InvalidFrame);
        }
        let mut bytes = [0; REPLY_BYTES];
        bytes[..4].copy_from_slice(b"KWSC");
        bytes[4] = 1;
        bytes[5] = match self.outcome {
            Outcome::Status => 1,
            Outcome::StopAccepted => 2,
            Outcome::StaleSession => 3,
        };
        bytes[6] = match self.snapshot.phase {
            Phase::Starting => 1,
            Phase::Active => 2,
            Phase::StopRequested => 3,
            Phase::Exited => 4,
            Phase::Unobserved => 5,
        };
        let (kind, value) = match self.snapshot.exit {
            None => (0, 0),
            Some(Exit::Code(code)) => (1, code),
            Some(Exit::Signal(signal)) => (2, signal),
        };
        bytes[7] = kind;
        bytes[8..16].copy_from_slice(&self.id.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.snapshot.session.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.snapshot.progress_epoch.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.snapshot.generation.unwrap_or(0).to_le_bytes());
        bytes[40..44].copy_from_slice(&value.to_le_bytes());
        bytes[44] = u8::from(self.snapshot.recovery_confirmed);
        bytes[45] = u8::from(self.snapshot.fault);
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, InvalidFrame> {
        if bytes.len() != REPLY_BYTES
            || &bytes[..4] != b"KWSC"
            || bytes[4] != 1
            || bytes[44] > 1
            || bytes[45] > 1
            || bytes[46..] != [0; 18]
        {
            return Err(InvalidFrame);
        }
        let outcome = match bytes[5] {
            1 => Outcome::Status,
            2 => Outcome::StopAccepted,
            3 => Outcome::StaleSession,
            _ => return Err(InvalidFrame),
        };
        let phase = match bytes[6] {
            1 => Phase::Starting,
            2 => Phase::Active,
            3 => Phase::StopRequested,
            4 => Phase::Exited,
            5 => Phase::Unobserved,
            _ => return Err(InvalidFrame),
        };
        let value = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        let exit = match (bytes[7], value) {
            (0, 0) => None,
            (1, code) => Some(Exit::Code(code)),
            (2, signal) => Some(Exit::Signal(signal)),
            _ => return Err(InvalidFrame),
        };
        let generation = number(bytes, 32);
        let reply = Self {
            id: number(bytes, 8),
            outcome,
            snapshot: Snapshot {
                session: number(bytes, 16),
                phase,
                progress_epoch: number(bytes, 24),
                generation: if generation == 0 {
                    None
                } else {
                    Some(generation)
                },
                exit,
                recovery_confirmed: bytes[44] != 0,
                fault: bytes[45] != 0,
            },
        };
        reply.encode()?;
        Ok(reply)
    }
    pub fn matches(self, request: Request) -> bool {
        self.id == request.id
            && match (request.command, self.outcome) {
                (Command::Status, Outcome::Status) => true,
                (Command::Stop { session }, Outcome::StopAccepted) => {
                    session == self.snapshot.session
                }
                (Command::Stop { session }, Outcome::StaleSession) => {
                    session != self.snapshot.session
                }
                _ => false,
            }
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            session: 123,
            phase: Phase::Active,
            progress_epoch: 7,
            generation: Some(1),
            exit: None,
            recovery_confirmed: false,
            fault: false,
        }
    }
    #[test]
    fn roundtrip_requires_distinct_commands_and_correlated_public_session() {
        for command in [Command::Status, Command::Stop { session: 123 }] {
            let request = Request { id: 1, command };
            assert_eq!(
                Request::decode(&request.encode().unwrap()).unwrap(),
                request
            );
        }
        let status = Request {
            id: 4,
            command: Command::Status,
        };
        let reply = Reply {
            id: 4,
            outcome: Outcome::Status,
            snapshot: snapshot(),
        };
        assert_eq!(Reply::decode(&reply.encode().unwrap()).unwrap(), reply);
        assert!(reply.matches(status));
        let stop = Request {
            id: 4,
            command: Command::Stop { session: 122 },
        };
        assert!(
            !Reply {
                outcome: Outcome::StopAccepted,
                ..reply
            }
            .matches(stop)
        );
        assert!(
            Reply {
                outcome: Outcome::StaleSession,
                ..reply
            }
            .matches(stop)
        );
    }
    #[test]
    fn reserved_fields_invalid_codes_and_invented_exit_or_recovery_facts_reject() {
        let request = Request {
            id: 1,
            command: Command::Status,
        }
        .encode()
        .unwrap();
        for i in [4, 5, 6, 7, 24, 31] {
            let mut b = request;
            b[i] = 255;
            assert!(Request::decode(&b).is_err());
        }
        let reply = Reply {
            id: 1,
            outcome: Outcome::Status,
            snapshot: snapshot(),
        }
        .encode()
        .unwrap();
        for i in [4, 5, 6, 7, 44, 45, 46, 63] {
            let mut b = reply;
            b[i] = 255;
            assert!(Reply::decode(&b).is_err());
        }
        for s in [
            Snapshot {
                phase: Phase::Exited,
                ..snapshot()
            },
            Snapshot {
                recovery_confirmed: true,
                ..snapshot()
            },
            Snapshot {
                generation: None,
                ..snapshot()
            },
            Snapshot {
                phase: Phase::Starting,
                ..snapshot()
            },
            Snapshot {
                phase: Phase::Exited,
                exit: Some(Exit::Signal(0)),
                ..snapshot()
            },
        ] {
            assert!(s.validate().is_err());
        }
        let s = Snapshot {
            phase: Phase::Exited,
            exit: Some(Exit::Code(0)),
            recovery_confirmed: false,
            ..snapshot()
        };
        assert!(s.validate().is_ok());
        assert!(!s.recovery_confirmed);
    }
}
