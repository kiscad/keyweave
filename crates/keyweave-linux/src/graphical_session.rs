//! Read-only session metadata; never a capture or output-consumer grant.
use keyweave_types::MonoTime;
use std::io;
#[cfg(target_os = "linux")]
mod native;
#[cfg(target_os = "linux")]
pub use native::NativeSessionProbe;
pub const SESSION_LIFETIME_NS: u64 = 250_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionSubject {
    pub parent: u32,
    pub worker: u32,
    pub uid: u32,
}
impl SessionSubject {
    fn valid(self) -> bool {
        self.parent > 0
            && self.worker > 0
            && self.parent != self.worker
            && self.parent <= i32::MAX as u32
            && self.worker <= i32::MAX as u32
            && self.uid > 0
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub parent_session: String,
    pub worker_session: String,
    pub uid: u32,
    pub active: bool,
    pub remote: bool,
    pub class: String,
    pub kind: String,
    pub seat: String,
    pub display: Option<String>,
    pub state: String,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}
fn local_display(value: &str) -> bool {
    if value.len() > 64 {
        return false;
    }
    let Some(value) = value.strip_prefix(':') else {
        return false;
    };
    let mut parts = value.split('.');
    let number = parts.next().unwrap_or("");
    let screen = parts.next();
    !number.is_empty()
        && number.bytes().all(|c| c.is_ascii_digit())
        && screen.is_none_or(|value| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()))
        && parts.next().is_none()
}
impl SessionSnapshot {
    fn eligible(&self, subject: SessionSubject) -> bool {
        subject.valid()
            && identifier(&self.parent_session)
            && self.parent_session == self.worker_session
            && self.uid == subject.uid
            && self.active
            && !self.remote
            && self.class == "user"
            && self.state == "active"
            && identifier(&self.seat)
            && match self.kind.as_str() {
                "x11" => self.display.as_deref().is_some_and(local_display),
                "wayland" => self.display.is_none(),
                _ => false,
            }
    }
}
pub trait SessionQuery {
    fn query(&mut self, subject: SessionSubject) -> io::Result<SessionSnapshot>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Anchor {
    session: String,
    uid: u32,
    seat: String,
    kind: String,
    display: Option<String>,
}
impl From<&SessionSnapshot> for Anchor {
    fn from(value: &SessionSnapshot) -> Self {
        Self {
            session: value.parent_session.clone(),
            uid: value.uid,
            seat: value.seat.clone(),
            kind: value.kind.clone(),
            display: value.display.clone(),
        }
    }
}
#[derive(Debug)]
pub struct SessionReady {
    subject: SessionSubject,
    snapshot: SessionSnapshot,
    observed: MonoTime,
    until: MonoTime,
}
impl SessionReady {
    pub fn fresh_for(&self, subject: SessionSubject, now: MonoTime) -> bool {
        self.subject == subject && now >= self.observed && now < self.until
    }
    pub fn snapshot(&self) -> &SessionSnapshot {
        &self.snapshot
    }
    pub fn observed_at(&self) -> MonoTime {
        self.observed
    }
    pub fn valid_until(&self) -> MonoTime {
        self.until
    }
}
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("invalid graphical session subject")]
    Subject,
    #[error("graphical session observer clock invalid")]
    Clock,
    #[error("graphical session identity changed")]
    Identity,
    #[error("graphical session query failed: {0}")]
    Query(#[source] io::Error),
}
pub struct SessionObserver<P> {
    probe: P,
    subject: SessionSubject,
    anchor: Option<Anchor>,
    ready: Option<SessionReady>,
    last: Option<MonoTime>,
}
impl<P: SessionQuery> SessionObserver<P> {
    pub fn new(probe: P, subject: SessionSubject) -> Result<Self, SessionError> {
        if !subject.valid() {
            return Err(SessionError::Subject);
        }
        Ok(Self {
            probe,
            subject,
            anchor: None,
            ready: None,
            last: None,
        })
    }
    pub fn current(&self, now: MonoTime) -> Option<&SessionReady> {
        self.ready
            .as_ref()
            .filter(|ready| ready.fresh_for(self.subject, now))
    }
    /// Control-plane only. Evidence starts before either query, never at completion.
    pub fn observe(
        &mut self,
        clock: &mut impl FnMut() -> io::Result<MonoTime>,
    ) -> Result<Option<&SessionReady>, SessionError> {
        self.ready = None;
        let start = clock().map_err(SessionError::Query)?;
        if self.last.is_some_and(|last| start < last) {
            return Err(SessionError::Clock);
        }
        self.last = Some(start);
        let until = start
            .checked_add(SESSION_LIFETIME_NS)
            .ok_or(SessionError::Clock)?;
        let first = self
            .probe
            .query(self.subject)
            .map_err(SessionError::Query)?;
        let last = self
            .probe
            .query(self.subject)
            .map_err(SessionError::Query)?;
        let finish = clock().map_err(SessionError::Query)?;
        if finish < start {
            return Err(SessionError::Clock);
        }
        self.last = Some(finish);
        if finish >= until || first != last || !first.eligible(self.subject) {
            return Ok(None);
        }
        let anchor = Anchor::from(&first);
        if self
            .anchor
            .as_ref()
            .is_some_and(|existing| *existing != anchor)
        {
            return Err(SessionError::Identity);
        }
        if self.anchor.is_none() {
            self.anchor = Some(anchor);
        }
        self.ready = Some(SessionReady {
            subject: self.subject,
            snapshot: first,
            observed: start,
            until,
        });
        Ok(self.ready.as_ref())
    }
}
