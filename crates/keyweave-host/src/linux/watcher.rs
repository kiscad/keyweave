//! Bounded source polling and ack-bound baseline. No input/metadata IO inline.
use super::file_source::{Compared, FileSource, MAX_SOURCE_BYTES, SourceError, SourceStep};
use super::{DriveStatus, NativeSession};
use keyweave_progress::reload::Outcome;
use keyweave_progress::session_apply::{self, Admission, BackendError, Completion, Refusal};
use keyweave_types::{ConfigGeneration, MonoTime};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
fn owner_id() -> Result<u64, WatchError> {
    NEXT_OWNER
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| WatchError::Identity)
}
/// Local correlation only; never an authentication token or serialized run ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplyTicket {
    owner: u64,
    sequence: u64,
}
struct External {
    ticket: ApplyTicket,
    result: Option<(Outcome, ConfigGeneration)>,
    abandoned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    Unchanged,
    Submitted,
    Accepted(ConfigGeneration),
    Rejected(ConfigGeneration),
    Cancelled(ConfigGeneration),
    ReadFailed(SourceError),
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchError {
    Source(SourceError),
    Interval,
    Baseline,
    Identity,
    Clock,
    Generation,
    Submit,
    Disabled,
}
impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "configuration watcher failed: {self:?}")
    }
}
impl std::error::Error for WatchError {}
trait Source {
    fn enabled(&self) -> bool {
        true
    }
    fn request(&mut self, seen: Arc<[u8]>, active: Arc<[u8]>) -> Result<(), SourceError>;
    fn take_ready(&mut self) -> Option<Result<Compared, SourceError>>;
    fn notice(&mut self) -> bool {
        false
    }
    fn close(&mut self);
    fn reap(&mut self) -> Result<bool, SourceError>;
}
impl Source for FileSource {
    fn request(&mut self, seen: Arc<[u8]>, active: Arc<[u8]>) -> Result<(), SourceError> {
        self.request_compare(seen, active)
    }
    fn take_ready(&mut self) -> Option<Result<Compared, SourceError>> {
        if self.poll() == SourceStep::Ready {
            self.take().map(|r| r.and_then(|s| s.into_comparison()))
        } else {
            None
        }
    }
    fn notice(&mut self) -> bool {
        self.poll_notice()
    }
    fn close(&mut self) {
        FileSource::close(self)
    }
    fn reap(&mut self) -> Result<bool, SourceError> {
        self.reap_closed()
    }
}
struct SourceMode(Option<FileSource>);
impl Source for SourceMode {
    fn enabled(&self) -> bool {
        self.0.is_some()
    }
    fn request(&mut self, seen: Arc<[u8]>, active: Arc<[u8]>) -> Result<(), SourceError> {
        self.0
            .as_mut()
            .ok_or(SourceError::Closed)?
            .request_compare(seen, active)
    }
    fn take_ready(&mut self) -> Option<Result<Compared, SourceError>> {
        self.0.as_mut().and_then(Source::take_ready)
    }
    fn notice(&mut self) -> bool {
        self.0.as_mut().is_some_and(Source::notice)
    }
    fn close(&mut self) {
        if let Some(source) = &mut self.0 {
            source.close();
        }
    }
    fn reap(&mut self) -> Result<bool, SourceError> {
        self.0.as_mut().map_or(Ok(true), FileSource::reap_closed)
    }
}
impl SourceMode {
    #[cfg(test)]
    fn job_pending(&self) -> bool {
        self.0.as_ref().is_some_and(FileSource::job_pending)
    }
}
trait Peer {
    fn claim(&mut self, owner: u64, allow_new: bool) -> bool;
    fn drive(&mut self) -> DriveStatus;
    fn at(&self) -> MonoTime;
    fn run(&self) -> Option<u64>;
    fn ready(&self) -> bool;
    fn submit(&mut self, owner: u64, body: Box<[u8]>) -> io::Result<()>;
    fn outcome(&mut self, owner: u64) -> Option<Outcome>;
    fn generation(&self) -> Option<ConfigGeneration>;
    fn fail(&mut self, error: io::Error);
}
impl Peer for NativeSession {
    fn claim(&mut self, owner: u64, allow_new: bool) -> bool {
        self.watcher_claim(owner, allow_new)
    }
    fn drive(&mut self) -> DriveStatus {
        self.poll_once()
    }
    fn at(&self) -> MonoTime {
        self.watcher_time()
    }
    fn run(&self) -> Option<u64> {
        self.watcher_run()
    }
    fn ready(&self) -> bool {
        self.watcher_ready()
    }
    fn submit(&mut self, owner: u64, body: Box<[u8]>) -> io::Result<()> {
        self.watcher_submit(owner, body)
    }
    fn outcome(&mut self, owner: u64) -> Option<Outcome> {
        self.watcher_outcome(owner)
    }
    fn generation(&self) -> Option<ConfigGeneration> {
        self.observed_config_generation()
    }
    fn fail(&mut self, error: io::Error) {
        self.watcher_fail(error)
    }
}
type Candidate = (Box<[u8]>, Arc<[u8]>);
struct Scheduler<S: Source> {
    source: S,
    active: Arc<[u8]>,
    seen: Arc<[u8]>,
    candidate: Option<Candidate>,
    inflight: Option<Arc<[u8]>>,
    base: Option<ConfigGeneration>,
    run: Option<u64>,
    last: Option<MonoTime>,
    next: Option<MonoTime>,
    interval: u64,
    dirty_since: Option<MonoTime>,
    quiet_until: Option<MonoTime>,
    reading: bool,
    manual: bool,
    event: Option<WatchEvent>,
    closed: bool,
    owner: u64,
    sequence: u64,
    external: Option<External>,
    dispatching: bool,
    claimed: bool,
}
impl<S: Source> Scheduler<S> {
    fn submit_external<P: Peer>(
        &mut self,
        peer: &mut P,
        body: Box<[u8]>,
        base: u64,
        target: u64,
    ) -> Result<Admission<ApplyTicket>, BackendError> {
        if self.closed
            || self.run.is_none()
            || self.run != peer.run()
            || !peer.claim(self.owner, false)
        {
            return Ok(Admission::NotSubmitted(Refusal::Unavailable));
        }
        if base == 0
            || base.checked_add(1) != Some(target)
            || peer.generation() != Some(ConfigGeneration(base))
        {
            return Ok(Admission::NotSubmitted(Refusal::Unavailable));
        }
        if body.is_empty() || body.len() > MAX_SOURCE_BYTES {
            return Ok(Admission::NotSubmitted(Refusal::Rejected));
        }
        if self.external.is_some()
            || self.reading
            || self.candidate.is_some()
            || self.inflight.is_some()
            || !peer.ready()
        {
            return Ok(Admission::NotSubmitted(Refusal::Busy));
        }
        if self.last.is_some_and(|last| peer.at() < last) {
            self.close();
            peer.fail(io::Error::other(WatchError::Clock));
            return Err(BackendError);
        }
        let Some(sequence) = self.sequence.checked_add(1) else {
            return Ok(Admission::NotSubmitted(Refusal::Unavailable));
        };
        self.sequence = sequence;
        let ticket = ApplyTicket {
            owner: self.owner,
            sequence,
        };
        self.inflight = Some(Arc::from(body.as_ref()));
        self.base = Some(ConfigGeneration(base));
        self.external = Some(External {
            ticket,
            result: None,
            abandoned: false,
        });
        // A panicking callback must never permit this body to be replayed or
        // an unrelated result to be consumed as its completion.
        self.dispatching = true;
        if peer.submit(self.owner, body).is_err() {
            self.dispatching = false;
            self.close();
            peer.fail(io::Error::other(WatchError::Submit));
            return Err(BackendError);
        }
        self.dispatching = false;
        self.event = Some(WatchEvent::Submitted);
        Ok(Admission::Queued(ticket))
    }
    fn external_completion(
        &mut self,
        ticket: &ApplyTicket,
    ) -> Result<Option<Completion<ApplyTicket>>, BackendError> {
        let external = self
            .external
            .as_ref()
            .filter(|external| external.ticket == *ticket && !external.abandoned)
            .ok_or(BackendError)?;
        if let Some((outcome, generation)) = external.result {
            self.external = None;
            return Ok(Some(Completion {
                ticket: *ticket,
                outcome,
                generation: generation.0,
            }));
        }
        if self.closed || self.dispatching {
            return Err(BackendError);
        }
        Ok(None)
    }
    fn abandon_external(&mut self, ticket: &ApplyTicket) -> Result<(), BackendError> {
        let external = self
            .external
            .as_mut()
            .filter(|external| external.ticket == *ticket)
            .ok_or(BackendError)?;
        if external.result.is_some() || self.closed {
            self.external = None;
        } else {
            external.abandoned = true;
        }
        Ok(())
    }
    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.source.close();
        self.candidate = None;
        self.inflight = None;
        self.event = Some(WatchEvent::Closed);
    }
    fn poll<P: Peer>(&mut self, peer: &mut P) -> Result<DriveStatus, WatchError> {
        if self.run.is_some_and(|old| peer.run() != Some(old)) {
            self.close();
            return Err(WatchError::Identity);
        }
        if !peer.claim(self.owner, !self.claimed) {
            self.close();
            return Err(WatchError::Identity);
        }
        self.claimed = true;
        if self.dispatching {
            self.dispatching = false;
            self.close();
            peer.fail(io::Error::other(WatchError::Submit));
            return Err(WatchError::Submit);
        }
        let status = peer.drive();
        if !matches!(
            status,
            DriveStatus::Pending(keyweave_linux::supervisor::SupervisorStatus::Running)
        ) {
            self.close();
            return Ok(status);
        }
        if self.closed {
            return Ok(status);
        }
        let result = self.advance(peer);
        if let Err(error) = result {
            self.close();
            if error != WatchError::Identity {
                peer.fail(io::Error::other(error));
            }
            return Err(error);
        }
        Ok(status)
    }
    fn advance<P: Peer>(&mut self, peer: &mut P) -> Result<(), WatchError> {
        let run = peer.run().ok_or(WatchError::Identity)?;
        if self.run.is_some_and(|old| old != run) {
            return Err(WatchError::Identity);
        }
        if self.run.is_none() && peer.generation() != Some(ConfigGeneration(1)) {
            return Err(WatchError::Baseline);
        }
        self.run = Some(run);
        let now = peer.at();
        if self.last.is_some_and(|last| now < last) {
            return Err(WatchError::Clock);
        }
        self.last = Some(now);
        if self.source.notice() {
            if self.dirty_since.is_none() {
                self.dirty_since = Some(now);
            }
            self.quiet_until = Some(now.checked_add(200_000_000).ok_or(WatchError::Clock)?);
        }
        // Confirmation processing never waits behind a user status notification.
        if self.inflight.is_some()
            && let Some(outcome) = peer.outcome(self.owner)
        {
            let generation = peer.generation().ok_or(WatchError::Generation)?;
            let base = self.base.ok_or(WatchError::Generation)?;
            let expected = if outcome == Outcome::Accepted {
                ConfigGeneration(base.0.checked_add(1).ok_or(WatchError::Generation)?)
            } else {
                base
            };
            if generation != expected {
                return Err(WatchError::Generation);
            }
            if let Some(external) = self
                .external
                .as_mut()
                .filter(|external| external.result.is_none())
            {
                if external.abandoned {
                    self.external = None;
                } else {
                    external.result = Some((outcome, generation));
                }
            }
            if let Some(bytes) = self.inflight.take()
                && outcome == Outcome::Accepted
            {
                self.active = bytes;
            }
            self.base = None;
            self.event = Some(match outcome {
                Outcome::Accepted => WatchEvent::Accepted(generation),
                Outcome::Rejected => WatchEvent::Rejected(generation),
                Outcome::Cancelled => WatchEvent::Cancelled(generation),
            });
            self.next = Some(now.checked_add(self.interval).ok_or(WatchError::Clock)?);
        }
        if self.event.is_some() || self.inflight.is_some() {
            return Ok(());
        }
        if self.reading
            && let Some(result) = self.source.take_ready()
        {
            self.reading = false;
            match result {
                Ok(Compared::Seen) => self.event = Some(WatchEvent::Unchanged),
                Ok(Compared::Active) => {
                    self.seen = self.active.clone();
                    self.event = Some(WatchEvent::Unchanged)
                }
                Ok(Compared::Changed { body, retained }) => self.candidate = Some((body, retained)),
                Err(error) => {
                    if matches!(
                        error,
                        SourceError::Panicked
                            | SourceError::Spawn
                            | SourceError::Comparison
                            | SourceError::Closed
                            | SourceError::Busy
                    ) {
                        return Err(WatchError::Source(error));
                    }
                    self.event = Some(WatchEvent::ReadFailed(error));
                }
            }
            self.next = Some(now.checked_add(self.interval).ok_or(WatchError::Clock)?);
        }
        if self.event.is_some() {
            return Ok(());
        }
        if self.candidate.is_some() && peer.ready() {
            let base = peer.generation().ok_or(WatchError::Generation)?;
            if base.0 == 0 || base.0 == u64::MAX {
                return Err(WatchError::Generation);
            }
            let (body, retained) = self.candidate.take().ok_or(WatchError::Submit)?;
            peer.submit(self.owner, body)
                .map_err(|_| WatchError::Submit)?;
            self.seen = retained.clone();
            self.inflight = Some(retained);
            self.base = Some(base);
            self.event = Some(WatchEvent::Submitted);
            return Ok(());
        }
        if self.source.enabled()
            && !self.reading
            && self.candidate.is_none()
            && peer.ready()
            && (self.manual
                || match (self.dirty_since, self.quiet_until) {
                    (Some(first), Some(quiet)) => {
                        now >= quiet
                            || now >= first.checked_add(1_000_000_000).ok_or(WatchError::Clock)?
                    }
                    _ => self.next.is_none_or(|next| now >= next),
                })
        {
            self.source
                .request(self.seen.clone(), self.active.clone())
                .map_err(WatchError::Source)?;
            self.reading = true;
            self.manual = false;
            self.dirty_since = None;
            self.quiet_until = None;
        }
        Ok(())
    }
}
/// Exclusively owns this session's reload source/results after first poll.
/// Construct cold with exact bytes used in the initial sealed generation1.
/// Consume notifications to permit further reads; use no other reload owner.
/// poll drives the bound NativeSession first, even after watcher close.
/// A different session is rejected before driving it.
/// close stops source work; it does not retract an already submitted reload.
pub struct ConfigWatcher {
    inner: Scheduler<SourceMode>,
}
impl ConfigWatcher {
    pub fn new(path: PathBuf, initial: Arc<[u8]>, interval_ns: u64) -> Result<Self, WatchError> {
        if !(100_000_000..=60_000_000_000).contains(&interval_ns) {
            return Err(WatchError::Interval);
        }
        if initial.is_empty() || initial.len() > MAX_SOURCE_BYTES {
            return Err(WatchError::Baseline);
        }
        let source = FileSource::with_notifications(path).map_err(WatchError::Source)?;
        Self::build(Some(source), initial, interval_ns)
    }
    /// Cold external-only mode: no file reader or directory notification owner.
    pub fn apply_only(initial: Arc<[u8]>) -> Result<Self, WatchError> {
        Self::build(None, initial, 100_000_000)
    }
    fn build(
        source: Option<FileSource>,
        initial: Arc<[u8]>,
        interval_ns: u64,
    ) -> Result<Self, WatchError> {
        if initial.is_empty() || initial.len() > MAX_SOURCE_BYTES {
            return Err(WatchError::Baseline);
        }
        Ok(Self {
            inner: Scheduler {
                source: SourceMode(source),
                active: initial.clone(),
                seen: initial,
                candidate: None,
                inflight: None,
                base: None,
                run: None,
                last: None,
                next: None,
                interval: interval_ns,
                dirty_since: None,
                quiet_until: None,
                reading: false,
                manual: false,
                event: None,
                closed: false,
                owner: owner_id()?,
                sequence: 0,
                external: None,
                dispatching: false,
                claimed: false,
            },
        })
    }
    /// Borrow the sole ACK owner; neither method drives the session. The host
    /// must poll this coordinator first on every iteration, then step Apply.
    pub fn apply_backend<'a>(&'a mut self, session: &'a mut NativeSession) -> ApplyBackend<'a> {
        ApplyBackend {
            coordinator: self,
            session,
            queued_ticket: None,
        }
    }
    /// Abandon reply ownership, not the actual reload. Continue polling this
    /// coordinator so any later real ACK updates its baseline exactly once.
    pub fn abandon_apply(&mut self, ticket: &ApplyTicket) -> Result<(), BackendError> {
        self.inner.abandon_external(ticket)
    }
    pub fn poll(&mut self, session: &mut NativeSession) -> Result<DriveStatus, WatchError> {
        self.inner.poll(session)
    }
    /// Coalesce manual requests into one flag; never extend an in-flight deadline.
    pub fn request_read(&mut self) -> Result<(), WatchError> {
        if self.inner.closed {
            return Err(WatchError::Source(SourceError::Closed));
        }
        if !self.inner.source.enabled() {
            return Err(WatchError::Disabled);
        }
        self.inner.manual = true;
        Ok(())
    }
    /// Cached notification failure; periodic source reading still continues.
    pub fn notification_error(&self) -> Option<SourceError> {
        self.inner
            .source
            .0
            .as_ref()
            .and_then(FileSource::notification_error)
    }
    pub fn take_event(&mut self) -> Option<WatchEvent> {
        self.inner.event.take()
    }
    pub fn close(&mut self) {
        self.inner.close()
    }
    pub fn reap_closed(&mut self) -> Result<bool, WatchError> {
        if !self.inner.closed {
            return Err(WatchError::Source(SourceError::Busy));
        }
        self.inner.source.reap().map_err(WatchError::Source)
    }
}

/// Actual Linux reload adapter; private NativeSession ACK validation remains
/// authoritative. Do not also consume NativeSession.take_reload_outcome.
pub struct ApplyBackend<'a> {
    coordinator: &'a mut ConfigWatcher,
    session: &'a mut NativeSession,
    queued_ticket: Option<ApplyTicket>,
}
impl ApplyBackend<'_> {
    /// Retain this after the initial Transaction step, even if its post-call
    /// clock check fails. Use it to abandon reply ownership on timeout/Drop.
    pub fn queued_ticket(&self) -> Option<ApplyTicket> {
        self.queued_ticket
    }
}
impl session_apply::Backend for ApplyBackend<'_> {
    type Ticket = ApplyTicket;
    fn submit(
        &mut self,
        body: Box<[u8]>,
        base: u64,
        target: u64,
    ) -> Result<Admission<ApplyTicket>, BackendError> {
        let result = self
            .coordinator
            .inner
            .submit_external(self.session, body, base, target);
        if let Ok(Admission::Queued(ticket)) = &result {
            self.queued_ticket = Some(*ticket);
        }
        result
    }
    fn completion(
        &mut self,
        ticket: &ApplyTicket,
    ) -> Result<Option<Completion<ApplyTicket>>, BackendError> {
        if self.coordinator.inner.run != self.session.watcher_run()
            || !self.session.watcher_owned(self.coordinator.inner.owner)
        {
            return Err(BackendError);
        }
        self.coordinator.inner.external_completion(ticket)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use keyweave_linux::supervisor::SupervisorStatus;
    type Baselines = (Arc<[u8]>, Arc<[u8]>);
    #[derive(Default)]
    struct FakeSource {
        requests: Vec<Baselines>,
        result: Option<Result<Compared, SourceError>>,
        closed: bool,
        reaped: bool,
        hint: bool,
    }
    impl Source for FakeSource {
        fn request(&mut self, seen: Arc<[u8]>, active: Arc<[u8]>) -> Result<(), SourceError> {
            assert!(!self.closed);
            self.requests.push((seen, active));
            Ok(())
        }
        fn take_ready(&mut self) -> Option<Result<Compared, SourceError>> {
            self.result.take()
        }
        fn notice(&mut self) -> bool {
            std::mem::take(&mut self.hint)
        }
        fn close(&mut self) {
            self.closed = true;
        }
        fn reap(&mut self) -> Result<bool, SourceError> {
            Ok(self.reaped)
        }
    }
    struct FakePeer {
        status: DriveStatus,
        run: u64,
        now: MonoTime,
        ready: bool,
        generation: ConfigGeneration,
        drives: usize,
        submissions: Vec<Box<[u8]>>,
        outcome: Option<Outcome>,
        faults: usize,
        submit_calls: usize,
        submit_fails: bool,
        submit_panics: bool,
        owner: Option<u64>,
    }
    impl Peer for FakePeer {
        fn claim(&mut self, owner: u64, allow_new: bool) -> bool {
            if self.owner.is_none() && allow_new {
                self.owner = Some(owner);
            }
            self.owner == Some(owner)
        }
        fn drive(&mut self) -> DriveStatus {
            self.drives += 1;
            self.status
        }
        fn at(&self) -> MonoTime {
            self.now
        }
        fn run(&self) -> Option<u64> {
            Some(self.run)
        }
        fn ready(&self) -> bool {
            self.ready
        }
        fn submit(&mut self, owner: u64, body: Box<[u8]>) -> io::Result<()> {
            assert!(self.ready);
            assert_eq!(self.owner, Some(owner));
            self.submit_calls += 1;
            assert!(!self.submit_panics, "synthetic external submit panic");
            if self.submit_fails {
                return Err(io::Error::other("synthetic submit failure"));
            }
            self.submissions.push(body);
            self.ready = false;
            Ok(())
        }
        fn outcome(&mut self, owner: u64) -> Option<Outcome> {
            assert_eq!(self.owner, Some(owner));
            self.outcome.take()
        }
        fn generation(&self) -> Option<ConfigGeneration> {
            Some(self.generation)
        }
        fn fail(&mut self, _: io::Error) {
            self.faults += 1;
        }
    }
    fn setup() -> (Scheduler<FakeSource>, FakePeer) {
        let initial: Arc<[u8]> = Arc::from(b"initial".as_slice());
        (
            Scheduler {
                source: FakeSource::default(),
                active: initial.clone(),
                seen: initial,
                candidate: None,
                inflight: None,
                base: None,
                run: None,
                last: None,
                next: None,
                interval: 100_000_000,
                dirty_since: None,
                quiet_until: None,
                reading: false,
                manual: false,
                event: None,
                closed: false,
                owner: owner_id().unwrap(),
                sequence: 0,
                external: None,
                dispatching: false,
                claimed: false,
            },
            FakePeer {
                status: DriveStatus::Pending(SupervisorStatus::Running),
                run: 57,
                now: MonoTime(0),
                ready: true,
                generation: ConfigGeneration(1),
                drives: 0,
                submissions: Vec::new(),
                outcome: None,
                faults: 0,
                submit_calls: 0,
                submit_fails: false,
                submit_panics: false,
                owner: None,
            },
        )
    }
    fn changed(s: &mut Scheduler<FakeSource>, bytes: &[u8]) {
        s.source.result = Some(Ok(Compared::Changed {
            body: bytes.into(),
            retained: Arc::from(bytes),
        }));
    }
    #[test]
    fn interval_notifications_and_active_gate_bound_reads_without_blocking_drive() {
        let (mut s, mut p) = setup();
        p.ready = false;
        s.poll(&mut p).unwrap();
        assert!(s.source.requests.is_empty());
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        assert_eq!(s.event, Some(WatchEvent::Unchanged));
        p.now = MonoTime(99_999_999);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        s.event.take();
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        p.now = MonoTime(100_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        assert_eq!(p.drives, 6);
    }
    #[test]
    fn only_matching_accepted_confirmation_advances_active_baseline() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"candidate");
        s.poll(&mut p).unwrap();
        assert_eq!(s.event, Some(WatchEvent::Submitted));
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(p.submissions[0].as_ref(), b"candidate");
        s.manual = true;
        p.now = MonoTime(500_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        assert!(s.manual);
        // Accepted is processed even if Submitted was never consumed.
        p.outcome = Some(Outcome::Accepted);
        p.generation = ConfigGeneration(2);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.active.as_ref(), b"candidate");
        assert_eq!(
            s.event.take(),
            Some(WatchEvent::Accepted(ConfigGeneration(2)))
        );
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        assert!(!s.manual);
        assert_eq!(s.source.requests[1].1.as_ref(), b"candidate");
    }
    #[test]
    fn rejected_bytes_are_suppressed_and_revert_to_active_needs_no_transaction() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"invalid");
        s.poll(&mut p).unwrap();
        s.event.take();
        p.outcome = Some(Outcome::Rejected);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(s.seen.as_ref(), b"invalid");
        assert_eq!(
            s.event.take(),
            Some(WatchEvent::Rejected(ConfigGeneration(1)))
        );
        s.manual = true;
        s.poll(&mut p).unwrap();
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        assert_eq!(s.event.take(), Some(WatchEvent::Unchanged));
        s.manual = true;
        s.poll(&mut p).unwrap();
        s.source.result = Some(Ok(Compared::Active));
        s.poll(&mut p).unwrap();
        assert_eq!(s.seen.as_ref(), b"initial");
        assert_eq!(p.submissions.len(), 1);
        assert_eq!(p.generation, ConfigGeneration(1));
    }
    #[test]
    fn ordinary_read_failure_preserves_baseline_and_recovers_on_next_interval() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        let error = SourceError::Io {
            kind: io::ErrorKind::NotFound,
            errno: Some(2),
        };
        s.source.result = Some(Err(error));
        s.poll(&mut p).unwrap();
        assert_eq!(s.event.take(), Some(WatchEvent::ReadFailed(error)));
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(p.faults, 0);
        p.now = MonoTime(100_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        changed(&mut s, b"fixed");
        s.poll(&mut p).unwrap();
        assert_eq!(p.submissions[0].as_ref(), b"fixed");
    }
    #[test]
    fn stop_first_closes_source_and_discards_late_candidate_without_submission() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"late");
        p.status = DriveStatus::Pending(SupervisorStatus::ShutdownRequested {
            deadline: MonoTime(2_000_000_000),
            request_sent: false,
        });
        s.poll(&mut p).unwrap();
        assert!(s.closed && s.source.closed);
        assert!(p.submissions.is_empty());
        assert!(!s.source.reap().unwrap());
        s.source.reaped = true;
        assert!(s.source.reap().unwrap());
        s.event.take();
        s.poll(&mut p).unwrap();
        assert!(s.event.is_none());
        assert_eq!(p.drives, 3);
    }
    #[test]
    fn wrong_session_is_never_driven_or_stopped_and_bad_initial_baseline_fails() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        p.run = 58;
        assert_eq!(s.poll(&mut p), Err(WatchError::Identity));
        assert_eq!(p.drives, 1);
        assert_eq!(p.faults, 0);
        assert!(s.closed);
        let (mut s, mut p) = setup();
        p.generation = ConfigGeneration(2);
        assert_eq!(s.poll(&mut p), Err(WatchError::Baseline));
        assert_eq!(p.faults, 1);
        assert!(s.source.requests.is_empty());
    }
    #[test]
    fn backward_time_and_wrong_ack_generation_permanently_close_without_retry() {
        let (mut s, mut p) = setup();
        p.now = MonoTime(1);
        s.poll(&mut p).unwrap();
        p.now = MonoTime(0);
        assert_eq!(s.poll(&mut p), Err(WatchError::Clock));
        s.poll(&mut p).unwrap();
        assert_eq!(p.faults, 1);
        assert!(s.source.closed);
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"unknown");
        s.poll(&mut p).unwrap();
        p.outcome = Some(Outcome::Accepted);
        assert_eq!(s.poll(&mut p), Err(WatchError::Generation));
        assert_eq!(s.active.as_ref(), b"initial");
        assert!(s.closed);
        p.ready = true;
        p.generation = ConfigGeneration(2);
        s.poll(&mut p).unwrap();
        assert_eq!(p.submissions.len(), 1);
        assert_eq!(p.faults, 1);
    }
    #[test]
    fn fatal_source_failure_stops_same_owner_and_constructor_checks_are_cold() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        s.source.result = Some(Err(SourceError::Panicked));
        assert_eq!(
            s.poll(&mut p),
            Err(WatchError::Source(SourceError::Panicked))
        );
        assert_eq!(p.faults, 1);
        assert!(s.closed);
        assert!(matches!(
            ConfigWatcher::new(PathBuf::from("/missing"), Arc::from(b"a".as_slice()), 1),
            Err(WatchError::Interval)
        ));
        assert!(matches!(
            ConfigWatcher::new(
                PathBuf::from("/missing"),
                Arc::from(b"".as_slice()),
                100_000_000
            ),
            Err(WatchError::Baseline)
        ));
        let mut watcher = ConfigWatcher::new(
            PathBuf::from("/missing"),
            Arc::from(b"a".as_slice()),
            100_000_000,
        )
        .unwrap();
        assert!(!watcher.inner.source.job_pending());
        watcher.request_read().unwrap();
        watcher.request_read().unwrap();
        assert!(watcher.inner.manual);
        watcher.close();
        assert_eq!(
            watcher.request_read(),
            Err(WatchError::Source(SourceError::Closed))
        );
        assert!(watcher.reap_closed().unwrap());
    }
    #[test]
    fn cancelled_confirmation_is_distinct_and_does_not_advance_baseline() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        changed(&mut s, b"cancelled");
        s.poll(&mut p).unwrap();
        p.outcome = Some(Outcome::Cancelled);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(
            s.event.take(),
            Some(WatchEvent::Cancelled(ConfigGeneration(1)))
        );
        assert_eq!(s.active.as_ref(), b"initial");
        assert_eq!(s.seen.as_ref(), b"cancelled");
        assert!(s.inflight.is_none());
        assert_eq!(p.submissions.len(), 1);
    }
    #[test]
    fn directory_hints_wait_for_quiet_and_cannot_postpone_checks_forever() {
        let (mut s, mut p) = setup();
        s.interval = 60_000_000_000;
        s.poll(&mut p).unwrap();
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        s.event.take();
        p.now = MonoTime(1);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        p.now = MonoTime(200_000_000);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        p.now = MonoTime(200_000_001);
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        s.event.take();
        p.now = MonoTime(300_000_000);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        for at in [
            400_000_000,
            600_000_000,
            800_000_000,
            1_000_000_000,
            1_200_000_000,
        ] {
            p.now = MonoTime(at);
            s.source.hint = true;
            s.poll(&mut p).unwrap();
            assert_eq!(s.source.requests.len(), 2);
        }
        p.now = MonoTime(1_300_000_000);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 3);
        assert!(s.dirty_since.is_none());
    }
    #[test]
    fn hints_during_busy_ack_and_notifications_coalesce_without_renewing_transaction() {
        let (mut s, mut p) = setup();
        s.interval = 60_000_000_000;
        s.poll(&mut p).unwrap();
        changed(&mut s, b"candidate");
        s.poll(&mut p).unwrap();
        p.now = MonoTime(1);
        s.source.hint = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        assert_eq!(s.base, Some(ConfigGeneration(1)));
        p.now = MonoTime(200_000_001);
        p.generation = ConfigGeneration(2);
        p.outcome = Some(Outcome::Accepted);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 1);
        s.event.take();
        s.poll(&mut p).unwrap();
        assert_eq!(s.source.requests.len(), 2);
    }
    fn idle() -> (Scheduler<FakeSource>, FakePeer) {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        s.event.take();
        (s, p)
    }
    fn external(s: &mut Scheduler<FakeSource>, p: &mut FakePeer, base: u64) -> ApplyTicket {
        match s
            .submit_external(p, Box::new(*b"external"), base, base + 1)
            .unwrap()
        {
            Admission::Queued(ticket) => ticket,
            _ => panic!("external candidate was not admitted"),
        }
    }
    #[test]
    fn external_ack_is_separate_from_file_seen_and_survives_later_file_ack() {
        let (mut s, mut p) = idle();
        let ticket = external(&mut s, &mut p, 1);
        assert!(s.external_completion(&ticket).unwrap().is_none());
        let wrong = ApplyTicket {
            owner: ticket.owner + 1,
            ..ticket
        };
        assert!(s.external_completion(&wrong).is_err());
        p.outcome = Some(Outcome::Accepted);
        p.generation = ConfigGeneration(2);
        p.ready = true;
        s.poll(&mut p).unwrap();
        s.event.take();
        assert_eq!(&*s.active, b"external");
        assert_eq!(&*s.seen, b"initial");
        s.manual = true;
        s.poll(&mut p).unwrap();
        assert_eq!(&*s.source.requests.last().unwrap().0, b"initial");
        assert_eq!(&*s.source.requests.last().unwrap().1, b"external");
        s.source.result = Some(Ok(Compared::Seen));
        s.poll(&mut p).unwrap();
        s.event.take();
        assert_eq!(p.submit_calls, 1); // unchanged old file does not revert Apply.
        s.manual = true;
        s.poll(&mut p).unwrap();
        changed(&mut s, b"changed file");
        s.poll(&mut p).unwrap();
        p.outcome = Some(Outcome::Accepted);
        p.generation = ConfigGeneration(3);
        p.ready = true;
        s.poll(&mut p).unwrap();
        s.event.take();
        assert_eq!(&*s.active, b"changed file");
        assert_eq!(&*s.seen, b"changed file");
        let reply = s.external_completion(&ticket).unwrap().unwrap();
        assert_eq!(reply.ticket, ticket);
        assert_eq!(reply.outcome, Outcome::Accepted);
        assert_eq!(reply.generation, 2);
        assert!(s.external_completion(&ticket).is_err());
        assert_eq!(p.submit_calls, 2);
    }
    #[test]
    fn external_never_preempts_read_candidate_pending_or_unclaimed_result() {
        let (mut s, mut p) = setup();
        s.poll(&mut p).unwrap();
        assert!(matches!(
            s.submit_external(&mut p, Box::new(*b"x"), 1, 2).unwrap(),
            Admission::NotSubmitted(Refusal::Busy)
        ));
        changed(&mut s, b"file");
        p.ready = false;
        s.poll(&mut p).unwrap();
        assert!(s.candidate.is_some());
        p.ready = true;
        assert!(matches!(
            s.submit_external(&mut p, Box::new(*b"x"), 1, 2).unwrap(),
            Admission::NotSubmitted(Refusal::Busy)
        ));
        s.poll(&mut p).unwrap();
        assert!(matches!(
            s.submit_external(&mut p, Box::new(*b"x"), 1, 2).unwrap(),
            Admission::NotSubmitted(Refusal::Busy)
        ));
        let (mut s, mut p) = idle();
        let ticket = external(&mut s, &mut p, 1);
        assert!(matches!(
            s.submit_external(&mut p, Box::new(*b"x"), 1, 2).unwrap(),
            Admission::NotSubmitted(Refusal::Busy)
        ));
        p.outcome = Some(Outcome::Accepted);
        p.generation = ConfigGeneration(2);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert!(matches!(
            s.submit_external(&mut p, Box::new(*b"x"), 2, 3).unwrap(),
            Admission::NotSubmitted(Refusal::Busy)
        ));
        s.external_completion(&ticket).unwrap().unwrap();
        assert_ne!(external(&mut s, &mut p, 2), ticket);
    }
    #[test]
    fn external_rejection_cancellation_abandonment_and_close_preserve_facts() {
        for outcome in [Outcome::Rejected, Outcome::Cancelled] {
            let (mut s, mut p) = idle();
            let ticket = external(&mut s, &mut p, 1);
            p.outcome = Some(outcome);
            p.ready = true;
            s.poll(&mut p).unwrap();
            assert_eq!(&*s.active, b"initial");
            assert_eq!(&*s.seen, b"initial");
            s.close(); // An already confirmed reply remains a historical fact.
            let reply = s.external_completion(&ticket).unwrap().unwrap();
            assert_eq!(reply.outcome, outcome);
            assert_eq!(reply.generation, 1);
        }
        let (mut s, mut p) = idle();
        let ticket = external(&mut s, &mut p, 1);
        s.abandon_external(&ticket).unwrap();
        assert!(s.external_completion(&ticket).is_err());
        p.outcome = Some(Outcome::Accepted);
        p.generation = ConfigGeneration(2);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert_eq!(&*s.active, b"external");
        assert!(s.external.is_none());
        let next = external(&mut s, &mut p, 2);
        s.close();
        assert!(s.external_completion(&next).is_err()); // no invented Cancelled
        s.abandon_external(&next).unwrap();
        assert!(s.external.is_none());
    }
    #[test]
    fn external_fences_refuse_wrong_owner_generation_length_and_exhaustion() {
        let (mut s, mut p) = idle();
        let drives = p.drives;
        p.run += 1;
        assert!(matches!(
            s.submit_external(&mut p, Box::new(*b"x"), 1, 2).unwrap(),
            Admission::NotSubmitted(Refusal::Unavailable)
        ));
        assert_eq!(p.drives, drives);
        assert_eq!(p.faults, 0);
        assert_eq!(p.submit_calls, 0);
        p.run -= 1;
        for (base, target) in [(0, 1), (2, 3), (1, 3), (u64::MAX, 0)] {
            assert!(matches!(
                s.submit_external(&mut p, Box::new(*b"x"), base, target)
                    .unwrap(),
                Admission::NotSubmitted(Refusal::Unavailable)
            ));
        }
        assert!(matches!(
            s.submit_external(&mut p, Box::new([]), 1, 2).unwrap(),
            Admission::NotSubmitted(Refusal::Rejected)
        ));
        assert!(matches!(
            s.submit_external(
                &mut p,
                vec![0; MAX_SOURCE_BYTES + 1].into_boxed_slice(),
                1,
                2
            )
            .unwrap(),
            Admission::NotSubmitted(Refusal::Rejected)
        ));
        s.sequence = u64::MAX;
        assert!(matches!(
            s.submit_external(&mut p, Box::new(*b"x"), 1, 2).unwrap(),
            Admission::NotSubmitted(Refusal::Unavailable)
        ));
        assert_eq!(p.submit_calls, 0);
    }
    #[test]
    fn external_submit_fault_or_panic_latches_and_never_replays() {
        for panic in [false, true] {
            let (mut s, mut p) = idle();
            p.submit_panics = panic;
            p.submit_fails = !panic;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                s.submit_external(&mut p, Box::new(*b"x"), 1, 2)
            }));
            if panic {
                assert!(result.is_err());
                assert_eq!(s.poll(&mut p), Err(WatchError::Submit));
            } else {
                assert!(result.unwrap().is_err());
            }
            assert!(s.closed);
            assert_eq!(p.faults, 1);
            assert_eq!(p.submit_calls, 1);
            assert!(matches!(
                s.submit_external(&mut p, Box::new(*b"x"), 1, 2).unwrap(),
                Admission::NotSubmitted(Refusal::Unavailable)
            ));
            s.poll(&mut p).unwrap();
            assert_eq!(p.submit_calls, 1);
            assert_eq!(p.faults, 1);
        }
    }
    #[test]
    fn apply_only_has_no_reader_and_foreign_tickets_do_not_consume_confirmations() {
        let (_, mut p) = setup();
        let mut coordinator = ConfigWatcher::apply_only(Arc::from(b"initial".as_slice())).unwrap();
        coordinator.inner.poll(&mut p).unwrap();
        assert!(!coordinator.inner.source.enabled());
        assert!(!coordinator.inner.reading);
        assert!(coordinator.notification_error().is_none());
        assert_eq!(coordinator.request_read(), Err(WatchError::Disabled));
        let Admission::Queued(ticket) = coordinator
            .inner
            .submit_external(&mut p, Box::new(*b"x"), 1, 2)
            .unwrap()
        else {
            panic!("not queued")
        };
        let (mut other, mut other_peer) = idle();
        let foreign = external(&mut other, &mut other_peer, 1);
        assert!(coordinator.inner.external_completion(&foreign).is_err());
        p.outcome = Some(Outcome::Accepted);
        p.generation = ConfigGeneration(2);
        p.ready = true;
        coordinator.inner.poll(&mut p).unwrap();
        assert!(
            coordinator
                .inner
                .external_completion(&ticket)
                .unwrap()
                .is_some()
        );
        coordinator.close();
        assert!(coordinator.reap_closed().unwrap());
    }
    #[test]
    fn external_generation_without_ack_is_not_completion_and_source_claim_is_sticky() {
        let (mut s, mut p) = idle();
        let ticket = external(&mut s, &mut p, 1);
        p.generation = ConfigGeneration(2);
        p.ready = true;
        s.poll(&mut p).unwrap();
        assert!(s.external_completion(&ticket).unwrap().is_none());
        assert_eq!(&*s.active, b"initial");
        let (_, mut other_peer) = setup();
        assert_eq!(other_peer.run, p.run);
        assert_eq!(s.poll(&mut other_peer), Err(WatchError::Identity));
        assert_eq!(other_peer.drives, 0);
        assert_eq!(other_peer.faults, 0);
        assert!(other_peer.owner.is_none());
        // Use a separate healthy transaction for the malformed-ACK checks.
        let (mut s, mut p) = idle();
        let ticket = external(&mut s, &mut p, 1);
        let (mut foreign, _) = setup();
        let drives = p.drives;
        assert_eq!(foreign.poll(&mut p), Err(WatchError::Identity));
        assert_eq!(p.drives, drives);
        assert_eq!(p.faults, 0);
        p.generation = ConfigGeneration(3);
        p.outcome = Some(Outcome::Accepted);
        assert_eq!(s.poll(&mut p), Err(WatchError::Generation));
        assert!(s.external_completion(&ticket).is_err());
        assert_eq!(p.faults, 1);
        let (mut s, mut p) = idle();
        s.last = Some(MonoTime(10));
        p.now = MonoTime(9);
        assert!(s.submit_external(&mut p, Box::new(*b"x"), 1, 2).is_err());
        assert!(s.closed);
        assert_eq!(p.faults, 1);
        assert_eq!(p.submit_calls, 0);
    }
}
