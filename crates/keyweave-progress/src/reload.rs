//! Independent reload control protocol. Never multiplex into progress/Stop IO.
//! No configuration parsing, OS calls, progress receipts or actor activation.
pub mod transport;
use crate::number;
use keyweave_types::{ConfigGeneration, MonoTime};
pub const FRAME_BYTES: usize = 48;
pub const MAX_UPDATE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_TIMEOUT_NS: u64 = 30_000_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReloadError {
    #[error("invalid reload frame")]
    Frame,
    #[error("reload run/request identity mismatch")]
    Identity,
    #[error("reload generation is invalid or out of sync")]
    Generation,
    #[error("reload payload size is invalid")]
    Length,
    #[error("another reload is pending")]
    Busy,
    #[error("reload has no pending request")]
    NoPending,
    #[error("reload lane is closed")]
    Closed,
    #[error("reload clock moved backwards")]
    Clock,
    #[error("reload deadline overflow or invalid timeout")]
    Deadline,
    #[error("reload deadline expired")]
    Expired,
    #[error("reload identity space exhausted")]
    Exhausted,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub run_id: u64,
    pub id: u64,
    pub base: ConfigGeneration,
    pub target: ConfigGeneration,
    pub byte_length: u64,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReloadRequest")
            .field("id", &self.id)
            .field("base", &self.base)
            .field("target", &self.target)
            .field("byte_length", &self.byte_length)
            .finish_non_exhaustive()
    }
}
fn generations(base: ConfigGeneration, target: ConfigGeneration) -> Result<(), ReloadError> {
    if base.0 == 0 || base.0.checked_add(1) != Some(target.0) {
        Err(ReloadError::Generation)
    } else {
        Ok(())
    }
}
fn encode(magic: &[u8; 4], kind: u8, fields: [u64; 5]) -> [u8; FRAME_BYTES] {
    let mut bytes = [0; FRAME_BYTES];
    bytes[..4].copy_from_slice(magic);
    bytes[4] = 1;
    bytes[5] = kind;
    for (index, value) in fields.into_iter().enumerate() {
        let start = 8 + 8 * index;
        bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
fn frame(bytes: &[u8], magic: &[u8; 4]) -> Result<(), ReloadError> {
    if bytes.len() != FRAME_BYTES || &bytes[..4] != magic || bytes[4] != 1 || bytes[6..8] != [0; 2]
    {
        Err(ReloadError::Frame)
    } else {
        Ok(())
    }
}
impl Request {
    pub fn validate(self) -> Result<(), ReloadError> {
        if self.run_id == 0 || self.id == 0 {
            return Err(ReloadError::Identity);
        }
        generations(self.base, self.target)?;
        if !(1..=MAX_UPDATE_BYTES).contains(&self.byte_length) {
            return Err(ReloadError::Length);
        }
        Ok(())
    }
    pub fn encode(self) -> Result<[u8; FRAME_BYTES], ReloadError> {
        self.validate()?;
        Ok(encode(
            b"KWRQ",
            0,
            [
                self.run_id,
                self.id,
                self.base.0,
                self.target.0,
                self.byte_length,
            ],
        ))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ReloadError> {
        frame(bytes, b"KWRQ")?;
        if bytes[5] != 0 {
            return Err(ReloadError::Frame);
        }
        let r = Self {
            run_id: number(bytes, 8),
            id: number(bytes, 16),
            base: ConfigGeneration(number(bytes, 24)),
            target: ConfigGeneration(number(bytes, 32)),
            byte_length: number(bytes, 40),
        };
        r.validate()?;
        Ok(r)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Rejected,
    Cancelled,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub run_id: u64,
    pub id: u64,
    pub base: ConfigGeneration,
    pub target: ConfigGeneration,
    pub active: ConfigGeneration,
    pub outcome: Outcome,
}
impl std::fmt::Debug for Ack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReloadAck")
            .field("id", &self.id)
            .field("base", &self.base)
            .field("target", &self.target)
            .field("active", &self.active)
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}
impl Ack {
    pub fn validate(self) -> Result<(), ReloadError> {
        if self.run_id == 0 || self.id == 0 {
            return Err(ReloadError::Identity);
        }
        generations(self.base, self.target)?;
        let expected = if self.outcome == Outcome::Accepted {
            self.target
        } else {
            self.base
        };
        if self.active != expected {
            return Err(ReloadError::Generation);
        }
        Ok(())
    }
    pub fn encode(self) -> Result<[u8; FRAME_BYTES], ReloadError> {
        self.validate()?;
        let kind = match self.outcome {
            Outcome::Accepted => 1,
            Outcome::Rejected => 2,
            Outcome::Cancelled => 3,
        };
        Ok(encode(
            b"KWRA",
            kind,
            [
                self.run_id,
                self.id,
                self.base.0,
                self.target.0,
                self.active.0,
            ],
        ))
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ReloadError> {
        frame(bytes, b"KWRA")?;
        let outcome = match bytes[5] {
            1 => Outcome::Accepted,
            2 => Outcome::Rejected,
            3 => Outcome::Cancelled,
            _ => return Err(ReloadError::Frame),
        };
        let ack = Self {
            run_id: number(bytes, 8),
            id: number(bytes, 16),
            base: ConfigGeneration(number(bytes, 24)),
            target: ConfigGeneration(number(bytes, 32)),
            active: ConfigGeneration(number(bytes, 40)),
            outcome,
        };
        ack.validate()?;
        Ok(ack)
    }
}
/// Unique parent-side lane. One in-flight transaction; no retry/deadline renewal.
/// Ack acceptance is not progress or proof that old held bindings were released.
pub struct Client {
    run_id: u64,
    active: ConfigGeneration,
    next: Option<u64>,
    pending: Option<(Request, MonoTime)>,
    last: MonoTime,
    timeout_ns: u64,
    closed: bool,
}
impl Client {
    pub fn new(
        run_id: u64,
        active: ConfigGeneration,
        now: MonoTime,
        timeout_ns: u64,
    ) -> Result<Self, ReloadError> {
        if run_id == 0 {
            return Err(ReloadError::Identity);
        }
        if active.0 == 0 {
            return Err(ReloadError::Generation);
        }
        if timeout_ns == 0 || timeout_ns > MAX_TIMEOUT_NS {
            return Err(ReloadError::Deadline);
        }
        Ok(Self {
            run_id,
            active,
            next: Some(1),
            pending: None,
            last: now,
            timeout_ns,
            closed: false,
        })
    }
    fn fail<T>(&mut self, error: ReloadError) -> Result<T, ReloadError> {
        self.closed = true;
        Err(error)
    }
    fn check(&mut self, now: MonoTime) -> Result<(), ReloadError> {
        if self.closed {
            return Err(ReloadError::Closed);
        }
        if now < self.last {
            return self.fail(ReloadError::Clock);
        }
        self.last = now;
        if self.pending.is_some_and(|(_, deadline)| now >= deadline) {
            return self.fail(ReloadError::Expired);
        }
        Ok(())
    }
    pub fn begin(&mut self, byte_length: u64, now: MonoTime) -> Result<Request, ReloadError> {
        self.check(now)?;
        if self.pending.is_some() {
            return Err(ReloadError::Busy);
        }
        if !(1..=MAX_UPDATE_BYTES).contains(&byte_length) {
            return Err(ReloadError::Length);
        }
        let Some(id) = self.next else {
            return self.fail(ReloadError::Exhausted);
        };
        let Some(target) = self.active.0.checked_add(1).map(ConfigGeneration) else {
            return self.fail(ReloadError::Exhausted);
        };
        let Some(deadline) = now.checked_add(self.timeout_ns) else {
            return self.fail(ReloadError::Deadline);
        };
        let request = Request {
            run_id: self.run_id,
            id,
            base: self.active,
            target,
            byte_length,
        };
        self.next = id.checked_add(1);
        self.pending = Some((request, deadline));
        Ok(request)
    }
    pub fn accept(&mut self, ack: Ack, now: MonoTime) -> Result<Outcome, ReloadError> {
        self.check(now)?;
        if let Err(e) = ack.validate() {
            return self.fail(e);
        }
        let Some((request, _)) = self.pending else {
            return self.fail(ReloadError::NoPending);
        };
        if ack.run_id != request.run_id || ack.id != request.id {
            return self.fail(ReloadError::Identity);
        }
        if ack.base != request.base || ack.target != request.target {
            return self.fail(ReloadError::Generation);
        }
        if ack.outcome == Outcome::Accepted {
            self.active = ack.active;
        }
        self.pending = None;
        Ok(ack.outcome)
    }
    /// Called from independent supervision; no expiry reset or lease renewal.
    pub fn poll(&mut self, now: MonoTime) -> Result<(), ReloadError> {
        self.check(now)
    }
    /// Stop/session loss/IO fault permanently closes this lane. Do not reopen
    /// under the same run or infer rollback if worker may already have committed.
    pub fn close(&mut self) {
        self.closed = true;
    }
    pub fn active(&self) -> ConfigGeneration {
        self.active
    }
    pub fn pending(&self) -> Option<Request> {
        self.pending.map(|(r, _)| r)
    }
    pub fn deadline(&self) -> Option<MonoTime> {
        self.pending.map(|(_, d)| d)
    }
}
/// Worker-side order and generation state only. Caller must invoke finish only
/// after the real actor activation result, never after parsing/preparation alone.
/// Commit can precede ack delivery: ack IO failure requires session stop, not retry.
pub struct Worker {
    run_id: u64,
    active: ConfigGeneration,
    last_id: u64,
    pending: Option<Request>,
    closed: bool,
}
impl Worker {
    pub fn new(run_id: u64, active: ConfigGeneration) -> Result<Self, ReloadError> {
        if run_id == 0 {
            return Err(ReloadError::Identity);
        }
        if active.0 == 0 {
            return Err(ReloadError::Generation);
        }
        Ok(Self {
            run_id,
            active,
            last_id: 0,
            pending: None,
            closed: false,
        })
    }
    fn fail<T>(&mut self, error: ReloadError) -> Result<T, ReloadError> {
        self.closed = true;
        Err(error)
    }
    pub fn begin(&mut self, request: Request) -> Result<(), ReloadError> {
        if self.closed {
            return Err(ReloadError::Closed);
        }
        if let Err(e) = request.validate() {
            return self.fail(e);
        }
        if self.pending.is_some() {
            return self.fail(ReloadError::Busy);
        }
        if request.run_id != self.run_id || self.last_id.checked_add(1) != Some(request.id) {
            return self.fail(ReloadError::Identity);
        }
        if request.base != self.active {
            return self.fail(ReloadError::Generation);
        }
        self.last_id = request.id;
        self.pending = Some(request);
        Ok(())
    }
    pub fn finish(
        &mut self,
        outcome: Outcome,
        actor_active: ConfigGeneration,
    ) -> Result<Ack, ReloadError> {
        if self.closed {
            return Err(ReloadError::Closed);
        }
        let Some(request) = self.pending else {
            return self.fail(ReloadError::NoPending);
        };
        let ack = Ack {
            run_id: request.run_id,
            id: request.id,
            base: request.base,
            target: request.target,
            active: actor_active,
            outcome,
        };
        if let Err(e) = ack.validate() {
            return self.fail(e);
        }
        self.active = actor_active;
        self.pending = None;
        Ok(ack)
    }
    pub fn close(&mut self) {
        self.closed = true;
    }
    pub fn active(&self) -> ConfigGeneration {
        self.active
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn client() -> Client {
        Client::new(7, ConfigGeneration(1), MonoTime(0), 10).unwrap()
    }
    #[test]
    fn fixed_wire_roundtrip_layout_and_reserved_or_length_rejection() {
        let mut c = client();
        let r = c.begin(123, MonoTime(0)).unwrap();
        let bytes = r.encode().unwrap();
        assert_eq!(Request::decode(&bytes).unwrap(), r);
        assert_eq!(&bytes[40..48], &123u64.to_le_bytes());
        for index in [0, 4, 5, 6, 7] {
            let mut bad = bytes;
            bad[index] = 255;
            assert!(Request::decode(&bad).is_err());
        }
        assert!(Request::decode(&bytes[..47]).is_err());
        assert!(Request::decode(&[0; 49]).is_err());
        let mut w = Worker::new(7, ConfigGeneration(1)).unwrap();
        w.begin(r).unwrap();
        let ack = w.finish(Outcome::Accepted, ConfigGeneration(2)).unwrap();
        let bytes = ack.encode().unwrap();
        assert_eq!(Ack::decode(&bytes).unwrap(), ack);
        for index in [0, 4, 5, 6, 7] {
            let mut bad = bytes;
            bad[index] = 255;
            assert!(Ack::decode(&bad).is_err());
        }
        assert!(Request::decode(&bytes).is_err());
        assert!(Ack::decode(&r.encode().unwrap()).is_err());
    }
    #[test]
    fn only_actual_acceptance_advances_parent_and_rejected_retry_has_new_id() {
        let mut c = client();
        let mut w = Worker::new(7, ConfigGeneration(1)).unwrap();
        let r = c.begin(1, MonoTime(0)).unwrap();
        assert_eq!(c.active(), ConfigGeneration(1));
        assert_eq!(c.begin(1, MonoTime(1)), Err(ReloadError::Busy));
        assert_eq!(c.deadline(), Some(MonoTime(10)));
        w.begin(r).unwrap();
        let rejected = w.finish(Outcome::Rejected, ConfigGeneration(1)).unwrap();
        assert_eq!(c.accept(rejected, MonoTime(2)).unwrap(), Outcome::Rejected);
        assert_eq!(c.active(), ConfigGeneration(1));
        let r = c.begin(1, MonoTime(3)).unwrap();
        assert_eq!(r.id, 2);
        assert_eq!(r.target, ConfigGeneration(2));
        w.begin(r).unwrap();
        let ack = w.finish(Outcome::Accepted, ConfigGeneration(2)).unwrap();
        assert_eq!(w.active(), ConfigGeneration(2));
        assert_eq!(c.active(), ConfigGeneration(1));
        c.accept(ack, MonoTime(4)).unwrap();
        assert_eq!(c.active(), ConfigGeneration(2));
        assert_eq!(c.accept(ack, MonoTime(5)), Err(ReloadError::NoPending));
        assert_eq!(c.begin(1, MonoTime(6)), Err(ReloadError::Closed));
    }
    #[test]
    fn stale_wrong_run_request_or_generation_ack_never_advances_parent() {
        for change in 0..3 {
            let mut c = client();
            let r = c.begin(1, MonoTime(0)).unwrap();
            let mut ack = Ack {
                run_id: r.run_id,
                id: r.id,
                base: r.base,
                target: r.target,
                active: r.target,
                outcome: Outcome::Accepted,
            };
            match change {
                0 => ack.run_id += 1,
                1 => ack.id += 1,
                _ => {
                    ack.base = ConfigGeneration(2);
                    ack.target = ConfigGeneration(3);
                    ack.active = ack.target;
                }
            }
            assert!(c.accept(ack, MonoTime(1)).is_err());
            assert_eq!(c.active(), ConfigGeneration(1));
            assert!(c.poll(MonoTime(2)).is_err());
        }
    }
    #[test]
    fn worker_cannot_ack_prepared_target_as_rejected_or_old_generation_as_accepted() {
        for (outcome, active) in [
            (Outcome::Accepted, ConfigGeneration(1)),
            (Outcome::Rejected, ConfigGeneration(2)),
            (Outcome::Cancelled, ConfigGeneration(2)),
        ] {
            let mut c = client();
            let r = c.begin(1, MonoTime(0)).unwrap();
            let mut w = Worker::new(7, ConfigGeneration(1)).unwrap();
            w.begin(r).unwrap();
            assert_eq!(w.finish(outcome, active), Err(ReloadError::Generation));
            assert_eq!(w.active(), ConfigGeneration(1));
            assert_eq!(
                w.finish(Outcome::Accepted, ConfigGeneration(2)),
                Err(ReloadError::Closed)
            );
        }
    }
    #[test]
    fn timeout_backwards_clock_and_stop_close_without_rollback_claim() {
        let mut c = client();
        c.begin(1, MonoTime(0)).unwrap();
        c.poll(MonoTime(9)).unwrap();
        assert_eq!(c.poll(MonoTime(10)), Err(ReloadError::Expired));
        assert_eq!(c.active(), ConfigGeneration(1));
        let mut c = client();
        c.begin(1, MonoTime(2)).unwrap();
        assert_eq!(c.poll(MonoTime(1)), Err(ReloadError::Clock));
        let mut c = client();
        let r = c.begin(1, MonoTime(0)).unwrap();
        let mut w = Worker::new(7, ConfigGeneration(1)).unwrap();
        w.begin(r).unwrap();
        let ack = w.finish(Outcome::Accepted, ConfigGeneration(2)).unwrap();
        c.close();
        assert_eq!(c.accept(ack, MonoTime(1)), Err(ReloadError::Closed));
        assert_eq!(c.active(), ConfigGeneration(1));
        assert_eq!(w.active(), ConfigGeneration(2)); // commit may precede lost ack
    }
    #[test]
    fn worker_rejects_duplicate_skipped_or_wrong_base_requests() {
        for change in 0..3 {
            let mut c = client();
            let mut r = c.begin(1, MonoTime(0)).unwrap();
            match change {
                0 => r.run_id += 1,
                1 => r.id = 2,
                _ => {
                    r.base = ConfigGeneration(2);
                    r.target = ConfigGeneration(3);
                }
            }
            let mut w = Worker::new(7, ConfigGeneration(1)).unwrap();
            assert!(w.begin(r).is_err());
            assert_eq!(w.active(), ConfigGeneration(1));
        }
        let mut c = client();
        let r = c.begin(1, MonoTime(0)).unwrap();
        let mut w = Worker::new(7, ConfigGeneration(1)).unwrap();
        w.begin(r).unwrap();
        w.finish(Outcome::Rejected, ConfigGeneration(1)).unwrap();
        assert_eq!(w.begin(r), Err(ReloadError::Identity));
    }
    #[test]
    fn size_identity_and_deadline_exhaustion_do_not_wrap() {
        let mut c = client();
        assert_eq!(c.begin(0, MonoTime(0)), Err(ReloadError::Length));
        assert_eq!(
            c.begin(MAX_UPDATE_BYTES + 1, MonoTime(0)),
            Err(ReloadError::Length)
        );
        assert_eq!(c.begin(MAX_UPDATE_BYTES, MonoTime(0)).unwrap().id, 1);
        let mut c = Client::new(7, ConfigGeneration(u64::MAX), MonoTime(0), 10).unwrap();
        assert_eq!(c.begin(1, MonoTime(0)), Err(ReloadError::Exhausted));
        let mut c = client();
        c.next = Some(u64::MAX);
        let r = c.begin(1, MonoTime(0)).unwrap();
        assert_eq!(r.id, u64::MAX);
        c.pending = None;
        assert_eq!(c.begin(1, MonoTime(1)), Err(ReloadError::Exhausted));
        let mut c = Client::new(7, ConfigGeneration(1), MonoTime(u64::MAX), 10).unwrap();
        assert_eq!(c.begin(1, MonoTime(u64::MAX)), Err(ReloadError::Deadline));
        assert!(Client::new(0, ConfigGeneration(1), MonoTime(0), 1).is_err());
        assert!(Client::new(1, ConfigGeneration(0), MonoTime(0), 1).is_err());
        assert!(Client::new(1, ConfigGeneration(1), MonoTime(0), 0).is_err());
    }
    #[test]
    fn cancelled_ack_keeps_generation_and_debug_omits_run_identity() {
        let mut c = Client::new(u64::MAX, ConfigGeneration(1), MonoTime(0), 10).unwrap();
        let r = c.begin(1, MonoTime(0)).unwrap();
        let mut w = Worker::new(u64::MAX, ConfigGeneration(1)).unwrap();
        w.begin(r).unwrap();
        let ack = w.finish(Outcome::Cancelled, ConfigGeneration(1)).unwrap();
        assert_eq!(c.accept(ack, MonoTime(1)).unwrap(), Outcome::Cancelled);
        assert_eq!(c.active(), ConfigGeneration(1));
        assert!(!format!("{r:?}{ack:?}").contains(&u64::MAX.to_string()));
    }
}
