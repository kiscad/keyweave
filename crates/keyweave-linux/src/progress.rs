//! Fixed, private-channel progress requests. No keys or user text on the wire.
use crate::{
    capture::CaptureSource,
    pump::FrameBackend,
    runtime::{Runtime, RuntimePhase},
};
use keyweave_types::MonoTime;
use std::io::{self, Read, Write};
pub const CHALLENGE_BYTES: usize = 24;
pub const RECEIPT_BYTES: usize = 40;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub run_id: u64,
    pub id: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub challenge: Challenge,
    pub baseline: u64,
    pub epoch: u64,
    pub phase: RuntimePhase,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProgressError {
    #[error("invalid progress frame")]
    Frame,
    #[error("invalid watchdog policy or identity")]
    Policy,
    #[error("progress clock went backwards")]
    Clock,
    #[error("progress deadline overflow")]
    TimeOverflow,
    #[error("progress arrived at or after lease expiry")]
    Expired,
    #[error("wrong run or stale challenge identity")]
    Identity,
    #[error("progress did not advance after the challenge")]
    Epoch,
    #[error("runtime phase moved backwards")]
    Phase,
    #[error("request identity space exhausted")]
    Exhausted,
}
fn number(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}
impl Challenge {
    pub fn encode(self) -> [u8; CHALLENGE_BYTES] {
        let mut bytes = [0; CHALLENGE_BYTES];
        bytes[..4].copy_from_slice(b"KWPC");
        bytes[4] = 1;
        bytes[8..16].copy_from_slice(&self.run_id.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.id.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProgressError> {
        if bytes.len() != CHALLENGE_BYTES
            || &bytes[..4] != b"KWPC"
            || bytes[4] != 1
            || bytes[5..8] != [0; 3]
        {
            return Err(ProgressError::Frame);
        }
        let value = Self {
            run_id: number(bytes, 8),
            id: number(bytes, 16),
        };
        if value.run_id == 0 || value.id == 0 {
            return Err(ProgressError::Identity);
        }
        Ok(value)
    }
}
impl Receipt {
    pub fn encode(self) -> [u8; RECEIPT_BYTES] {
        let mut bytes = [0; RECEIPT_BYTES];
        bytes[..4].copy_from_slice(b"KWPR");
        bytes[4] = 1;
        bytes[5] = match self.phase {
            RuntimePhase::Active => 1,
            RuntimePhase::Recovering => 2,
            RuntimePhase::Stopped => 3,
        };
        bytes[8..16].copy_from_slice(&self.challenge.run_id.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.challenge.id.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.baseline.to_le_bytes());
        bytes[32..40].copy_from_slice(&self.epoch.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProgressError> {
        if bytes.len() != RECEIPT_BYTES
            || &bytes[..4] != b"KWPR"
            || bytes[4] != 1
            || bytes[6..8] != [0; 2]
        {
            return Err(ProgressError::Frame);
        }
        let phase = match bytes[5] {
            1 => RuntimePhase::Active,
            2 => RuntimePhase::Recovering,
            3 => RuntimePhase::Stopped,
            _ => return Err(ProgressError::Phase),
        };
        let value = Self {
            challenge: Challenge {
                run_id: number(bytes, 8),
                id: number(bytes, 16),
            },
            baseline: number(bytes, 24),
            epoch: number(bytes, 32),
            phase,
        };
        if value.challenge.run_id == 0 || value.challenge.id == 0 {
            return Err(ProgressError::Identity);
        }
        if value.epoch <= value.baseline {
            return Err(ProgressError::Epoch);
        }
        Ok(value)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub startup_ns: u64,
    pub lease_ns: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            startup_ns: 10_000_000_000,
            lease_ns: 2_000_000_000,
        }
    }
}
pub struct ProgressWatchdog {
    policy: Policy,
    now: MonoTime,
    deadline: MonoTime,
    challenge: Challenge,
    epoch: u64,
    phase: Option<RuntimePhase>,
}
impl ProgressWatchdog {
    pub fn new(run_id: u64, policy: Policy, now: MonoTime) -> Result<Self, ProgressError> {
        if run_id == 0
            || policy.startup_ns == 0
            || policy.startup_ns > 120_000_000_000
            || policy.lease_ns == 0
            || policy.lease_ns > 60_000_000_000
        {
            return Err(ProgressError::Policy);
        }
        let deadline = now
            .checked_add(policy.startup_ns)
            .ok_or(ProgressError::TimeOverflow)?;
        Ok(Self {
            policy,
            now,
            deadline,
            challenge: Challenge { run_id, id: 1 },
            epoch: 0,
            phase: None,
        })
    }
    pub fn challenge(&self) -> Option<Challenge> {
        if self.phase == Some(RuntimePhase::Stopped) {
            None
        } else {
            Some(self.challenge)
        }
    }
    pub fn deadline(&self) -> MonoTime {
        self.deadline
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn phase(&self) -> Option<RuntimePhase> {
        self.phase
    }
    pub fn check(&mut self, now: MonoTime) -> Result<(), ProgressError> {
        if now < self.now {
            return Err(ProgressError::Clock);
        }
        self.now = now;
        if now >= self.deadline {
            return Err(ProgressError::Expired);
        }
        Ok(())
    }
    /// Atomic validation; old buffered responses cannot renew a new request.
    pub fn observe(&mut self, receipt: Receipt, now: MonoTime) -> Result<(), ProgressError> {
        if now < self.now {
            return Err(ProgressError::Clock);
        }
        self.now = now; // Record observed time even if a receipt is rejected.
        if now >= self.deadline {
            return Err(ProgressError::Expired);
        }
        if self.challenge() != Some(receipt.challenge) {
            return Err(ProgressError::Identity);
        }
        if receipt.epoch <= receipt.baseline
            || receipt.epoch <= self.epoch
            || receipt.baseline < self.epoch
        {
            return Err(ProgressError::Epoch);
        }
        if matches!(
            (self.phase, receipt.phase),
            (Some(RuntimePhase::Recovering), RuntimePhase::Active)
                | (Some(RuntimePhase::Stopped), _)
        ) {
            return Err(ProgressError::Phase);
        }
        let deadline = now
            .checked_add(self.policy.lease_ns)
            .ok_or(ProgressError::TimeOverflow)?;
        let next = if receipt.phase == RuntimePhase::Stopped {
            self.challenge.id
        } else {
            self.challenge
                .id
                .checked_add(1)
                .ok_or(ProgressError::Exhausted)?
        };
        self.now = now;
        self.deadline = deadline;
        self.epoch = receipt.epoch;
        self.phase = Some(receipt.phase);
        self.challenge.id = next;
        Ok(())
    }
}
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Protocol(#[from] ProgressError),
    #[error("progress channel failed: {0}")]
    Io(#[from] io::Error),
    #[error("progress channel closed")]
    Closed,
}
struct Pending {
    bytes: [u8; RECEIPT_BYTES],
    written: usize,
}
/// Own one nonblocking reader/writer pair. No Clone and no epoch counter of its
/// own. Only a Runtime poll after receive can make a publish eligible.
pub struct ProgressClient<R: Read, W: Write> {
    reader: R,
    writer: W,
    run_id: u64,
    last_challenge: u64,
    request: [u8; CHALLENGE_BYTES],
    read: usize,
    ticket: Option<(Challenge, u64)>,
    response: Option<Pending>,
}
impl<R: Read, W: Write> ProgressClient<R, W> {
    #[cfg(target_os = "linux")]
    pub(crate) fn wait_requests(&self) -> [crate::reactor::WaitRequest<'_>; 2]
    where
        R: std::os::fd::AsFd,
        W: std::os::fd::AsFd,
    {
        use crate::reactor::{Interest, WaitRequest};
        [
            WaitRequest {
                fd: self.reader.as_fd(),
                interest: if self.ticket.is_none() && self.response.is_none() {
                    Interest::Read
                } else {
                    Interest::Errors
                },
            },
            WaitRequest {
                fd: self.writer.as_fd(),
                interest: if self.response.is_some() {
                    Interest::Write
                } else {
                    Interest::Errors
                },
            },
        ]
    }
    pub fn new(reader: R, writer: W, run_id: u64) -> Result<Self, ClientError> {
        if run_id == 0 {
            return Err(ProgressError::Policy.into());
        }
        Ok(Self {
            reader,
            writer,
            run_id,
            last_challenge: 0,
            request: [0; CHALLENGE_BYTES],
            read: 0,
            ticket: None,
            response: None,
        })
    }
    pub fn receive<S: CaptureSource, B: FrameBackend>(
        &mut self,
        runtime: &Runtime<S, B>,
    ) -> Result<bool, ClientError> {
        if self.ticket.is_some() || self.response.is_some() {
            return Ok(false);
        }
        match self.reader.read(&mut self.request[self.read..]) {
            Ok(0) => return Err(ClientError::Closed),
            Ok(count) if count > CHALLENGE_BYTES - self.read => {
                return Err(ProgressError::Frame.into());
            }
            Ok(count) => self.read += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        if self.read != CHALLENGE_BYTES {
            return Ok(false);
        }
        let challenge = Challenge::decode(&self.request)?;
        if challenge.run_id != self.run_id
            || Some(challenge.id) != self.last_challenge.checked_add(1)
        {
            return Err(ProgressError::Identity.into());
        }
        self.last_challenge = challenge.id;
        self.read = 0;
        self.ticket = Some((challenge, runtime.progress_epoch()));
        Ok(true)
    }
    pub fn publish<S: CaptureSource, B: FrameBackend>(
        &mut self,
        runtime: &Runtime<S, B>,
    ) -> Result<bool, ClientError> {
        if self.response.is_none() {
            let Some((challenge, baseline)) = self.ticket else {
                return Ok(false);
            };
            if runtime.progress_epoch() <= baseline {
                return Ok(false);
            }
            self.response = Some(Pending {
                bytes: Receipt {
                    challenge,
                    baseline,
                    epoch: runtime.progress_epoch(),
                    phase: runtime.phase(),
                }
                .encode(),
                written: 0,
            });
        }
        self.flush()
    }
    /// Partial frames must finish unchanged; calling flush cannot mint new progress.
    pub fn flush(&mut self) -> Result<bool, ClientError> {
        let Some(pending) = self.response.as_mut() else {
            return Ok(false);
        };
        match self.writer.write(&pending.bytes[pending.written..]) {
            Ok(count) if count > RECEIPT_BYTES - pending.written => {
                return Err(ProgressError::Frame.into());
            }
            Ok(count) => pending.written += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        if pending.written == RECEIPT_BYTES {
            self.response = None;
            self.ticket = None;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn watchdog() -> ProgressWatchdog {
        ProgressWatchdog::new(
            7,
            Policy {
                startup_ns: 10,
                lease_ns: 5,
            },
            MonoTime(0),
        )
        .unwrap()
    }
    fn receipt(
        watch: &ProgressWatchdog,
        baseline: u64,
        epoch: u64,
        phase: RuntimePhase,
    ) -> Receipt {
        Receipt {
            challenge: watch.challenge().unwrap(),
            baseline,
            epoch,
            phase,
        }
    }
    #[test]
    fn wire_roundtrip_rejects_short_versions_reserved_and_zero_identity() {
        let challenge = Challenge { run_id: 7, id: 1 };
        assert_eq!(Challenge::decode(&challenge.encode()).unwrap(), challenge);
        let value = Receipt {
            challenge,
            baseline: 42,
            epoch: 43,
            phase: RuntimePhase::Recovering,
        };
        assert_eq!(Receipt::decode(&value.encode()).unwrap(), value);
        for bytes in [vec![], vec![0; 39], vec![0; 41]] {
            assert!(Receipt::decode(&bytes).is_err());
        }
        for offset in [0, 4, 6, 7] {
            let mut bytes = value.encode();
            bytes[offset] = 255;
            assert!(Receipt::decode(&bytes).is_err());
        }
        let mut bytes = challenge.encode();
        bytes[5] = 1;
        assert!(Challenge::decode(&bytes).is_err());
        let mut bytes = value.encode();
        bytes[8..16].fill(0);
        assert!(Receipt::decode(&bytes).is_err());
    }
    #[test]
    fn startup_and_lease_boundaries_do_not_accept_buffered_late_response() {
        let mut watch = watchdog();
        assert!(watch.check(MonoTime(9)).is_ok());
        let value = receipt(&watch, 0, 1, RuntimePhase::Active);
        assert_eq!(
            watch.observe(value, MonoTime(10)),
            Err(ProgressError::Expired)
        );
        assert_eq!(watch.epoch(), 0);
        let mut watch = watchdog();
        watch
            .observe(receipt(&watch, 0, 1, RuntimePhase::Active), MonoTime(2))
            .unwrap();
        assert_eq!(watch.deadline(), MonoTime(7));
        let value = receipt(&watch, 1, 2, RuntimePhase::Active);
        assert_eq!(
            watch.observe(value, MonoTime(7)),
            Err(ProgressError::Expired)
        );
        assert_eq!(watch.epoch(), 1);
    }
    #[test]
    fn old_challenges_frozen_epoch_and_wrong_run_do_not_extend_lease() {
        let mut watch = watchdog();
        let old = receipt(&watch, 0, 1, RuntimePhase::Active);
        watch.observe(old, MonoTime(1)).unwrap();
        let deadline = watch.deadline();
        assert_eq!(
            watch.observe(old, MonoTime(2)),
            Err(ProgressError::Identity)
        );
        let mut value = receipt(&watch, 1, 1, RuntimePhase::Active);
        assert_eq!(watch.observe(value, MonoTime(2)), Err(ProgressError::Epoch));
        value.epoch = 2;
        value.challenge.run_id = 8;
        assert_eq!(
            watch.observe(value, MonoTime(2)),
            Err(ProgressError::Identity)
        );
        assert_eq!(watch.deadline(), deadline);
        assert_eq!(watch.epoch(), 1);
    }
    #[test]
    fn stopped_receipt_does_not_prove_exit_or_keep_renewing() {
        let mut watch = watchdog();
        watch
            .observe(receipt(&watch, 0, 1, RuntimePhase::Recovering), MonoTime(1))
            .unwrap();
        assert_eq!(
            watch.observe(receipt(&watch, 1, 2, RuntimePhase::Active), MonoTime(2)),
            Err(ProgressError::Phase)
        );
        watch
            .observe(receipt(&watch, 1, 2, RuntimePhase::Stopped), MonoTime(2))
            .unwrap();
        assert!(watch.challenge().is_none());
        assert_eq!(watch.phase(), Some(RuntimePhase::Stopped));
        assert_eq!(watch.check(MonoTime(7)), Err(ProgressError::Expired));
    }
    #[test]
    fn clock_policy_overflow_and_retired_runtime_epoch_are_explicit() {
        assert!(ProgressWatchdog::new(0, Policy::default(), MonoTime(0)).is_err());
        assert!(
            ProgressWatchdog::new(
                1,
                Policy {
                    startup_ns: 0,
                    lease_ns: 5
                },
                MonoTime(0)
            )
            .is_err()
        );
        assert!(ProgressWatchdog::new(1, Policy::default(), MonoTime(u64::MAX)).is_err());
        let mut watch = watchdog();
        watch.check(MonoTime(2)).unwrap();
        assert_eq!(watch.check(MonoTime(1)), Err(ProgressError::Clock));
        watch
            .observe(receipt(&watch, 0, 2, RuntimePhase::Active), MonoTime(2))
            .unwrap();
        assert_eq!(
            watch.observe(receipt(&watch, 1, 3, RuntimePhase::Active), MonoTime(3)),
            Err(ProgressError::Epoch)
        );
    }
    #[test]
    fn rejected_late_receipt_cannot_rewind_time_and_resurrect_lease() {
        let mut watch = watchdog();
        let value = receipt(&watch, 0, 1, RuntimePhase::Active);
        assert_eq!(
            watch.observe(value, MonoTime(10)),
            Err(ProgressError::Expired)
        );
        assert_eq!(watch.observe(value, MonoTime(9)), Err(ProgressError::Clock));
        assert_eq!(
            watch.observe(value, MonoTime(11)),
            Err(ProgressError::Expired)
        );
        assert_eq!(watch.epoch(), 0);
    }
}
