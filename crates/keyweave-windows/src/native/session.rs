//! Explicit same-thread passive startup; no executable or automatic capture.
use super::{
    hook_thread::{HookSession, HookSessionError, LoopStep},
    input_probe::{check_input_access, sample_input_state},
};
use crate::{
    access::{AccessError, AccessLease},
    emergency::{EmergencyChord, EmergencyError},
    hook::{HookEvent, HookFault, HookHandler},
    keys::{IdentityPolicy, InjectionTag},
    pump::SendBackend,
    reactor::{SessionClock, WaitPlan},
    runtime::{ActorPhase, RuntimeError, SessionActor},
    startup::{InputSample, ProbeError},
};
use keyweave_engine::ConfigSnapshot;
use keyweave_types::{CaptureDecision, MonoTime};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
const STARTUP_NS: u64 = 2_000_000_000;
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Probe(#[from] ProbeError),
    #[error(transparent)]
    Hook(#[from] HookSessionError),
    #[error(transparent)]
    Actor(#[from] RuntimeError),
    #[error("passive startup deadline expired")]
    Timeout,
    #[error("session clock failed")]
    Clock,
    #[error(transparent)]
    EmergencyDetector(#[from] EmergencyError),
    #[error(transparent)]
    AccessLease(#[from] AccessError),
    #[error("session is closed")]
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStep {
    Waiting,
    Activated,
    Continue,
    Quit,
    Emergency,
}
#[derive(Debug)]
pub struct ShutdownReport {
    pub unhook: Result<(), HookSessionError>,
    pub recovery: Result<(), RuntimeError>,
}
struct SessionHandler<B: SendBackend, C: FnMut() -> Option<MonoTime>> {
    enabled: Rc<Cell<bool>>,
    actor: Rc<RefCell<SessionActor<B>>>,
    emergency: Rc<RefCell<EmergencyChord>>,
    access: Rc<RefCell<AccessLease>>,
    clock: C,
}
fn guard(
    chord: &RefCell<EmergencyChord>,
    now: MonoTime,
    event: Option<HookEvent>,
) -> Result<Option<MonoTime>, HookFault> {
    let mut chord = chord.try_borrow_mut().map_err(|_| HookFault::Reentrant)?;
    if chord.advance(now).map_err(HookFault::EmergencyDetector)? {
        return Err(HookFault::Emergency);
    }
    if let Some(event) = event {
        chord
            .observe(event.identity.key, event.edge, now)
            .map_err(HookFault::EmergencyDetector)?;
    }
    Ok(chord.next_deadline())
}
impl<B: SendBackend, C: FnMut() -> Option<MonoTime>> HookHandler for SessionHandler<B, C> {
    fn handle(&mut self, event: HookEvent) -> Result<CaptureDecision, HookFault> {
        if !self.enabled.get() {
            return Ok(CaptureDecision::Pass);
        }
        let mut actor = self
            .actor
            .try_borrow_mut()
            .map_err(|_| HookFault::Reentrant)?;
        let Some(now) = (self.clock)() else {
            actor.control_failed(RuntimeError::Clock);
            return Err(HookFault::Handler);
        };
        // Exactly one fresh delivery time, shared with the engine. Emergency
        // due wins before this up/down and before actor due timers/output.
        self.access
            .try_borrow()
            .map_err(|_| HookFault::Reentrant)?
            .check(now)
            .map_err(HookFault::AccessLease)?;
        guard(&self.emergency, now, Some(event))?;
        actor.key(event, now).map_err(|error| {
            actor.control_failed(error);
            HookFault::Handler
        })
    }
}
fn active_wait<B: SendBackend>(
    actor: &RefCell<SessionActor<B>>,
    emergency: &RefCell<EmergencyChord>,
    access: &RefCell<AccessLease>,
    now: Option<MonoTime>,
) -> Result<WaitPlan, HookFault> {
    let mut actor = actor.try_borrow_mut().map_err(|_| HookFault::Reentrant)?;
    let Some(now) = now else {
        actor.control_failed(RuntimeError::Clock);
        return Err(HookFault::Handler);
    };
    let refresh_at = {
        let access = access.try_borrow().map_err(|_| HookFault::Reentrant)?;
        access.check(now).map_err(HookFault::AccessLease)?;
        access.next_refresh()
    };
    let emergency_at = guard(emergency, now, None)?;
    if let Err(error) = actor.tick(now) {
        actor.control_failed(error);
        return Err(HookFault::Handler);
    }
    Ok(WaitPlan::for_deadline(
        now,
        [refresh_at, emergency_at, actor.next_wakeup()]
            .into_iter()
            .flatten()
            .min(),
    ))
}
fn monitored_wait<B: SendBackend>(
    actor: &RefCell<SessionActor<B>>,
    emergency: &RefCell<EmergencyChord>,
    access: &RefCell<AccessLease>,
    mut clock: impl FnMut() -> Option<MonoTime>,
    probe: impl FnOnce() -> Result<(), ProbeError>,
) -> Result<WaitPlan, HookFault> {
    let Some(mut now) = clock() else {
        return active_wait(actor, emergency, access, None);
    };
    let due = access
        .try_borrow()
        .map_err(|_| HookFault::Reentrant)?
        .refresh_due(now)
        .map_err(HookFault::AccessLease)?;
    guard(emergency, now, None)?;
    if due {
        // Metadata APIs may synchronously deliver sent callbacks. No actor or
        // lease borrow spans these calls, and an existing fault wins over work.
        let observed = probe();
        if let Some(fault) = super::hook_thread::current_fault() {
            return Err(fault);
        }
        observed.map_err(HookFault::InputAccess)?;
        let Some(after) = clock() else {
            return active_wait(actor, emergency, access, None);
        };
        if after < now {
            return Err(HookFault::AccessLease(AccessError::ClockBackwards));
        }
        access
            .try_borrow_mut()
            .map_err(|_| HookFault::Reentrant)?
            .renew_after_probe(after)
            .map_err(HookFault::AccessLease)?;
        now = after;
    }
    active_wait(actor, emergency, access, Some(now))
}
/// Construct only on the dedicated session thread. This component does not
/// supply repeat, independent progress/session supervision or a deployment-ready host.
/// Call shutdown explicitly: Drop disables the Hook without injecting releases.
pub struct CaptureSession<B: SendBackend + 'static> {
    hook: HookSession,
    actor: Rc<RefCell<SessionActor<B>>>,
    enabled: Rc<Cell<bool>>,
    clock: SessionClock,
    deadline: MonoTime,
    emergency: Rc<RefCell<EmergencyChord>>,
    access: Rc<RefCell<AccessLease>>,
    shutdown: Option<ShutdownReport>,
}
impl<B: SendBackend + 'static> CaptureSession<B> {
    pub fn install(
        backend: B,
        tag: InjectionTag,
        policy: IdentityPolicy,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, SessionError> {
        // Validate every output/condition before registering or touching input.
        let actor = Rc::new(RefCell::new(SessionActor::new(backend, tag, config)?));
        let clock = SessionClock::new();
        let now = clock.now().ok_or(SessionError::Clock)?;
        let deadline = MonoTime(now.0.checked_add(STARTUP_NS).ok_or(SessionError::Clock)?);
        sample_input_state()?;
        if clock.now().ok_or(SessionError::Clock)? >= deadline {
            return Err(SessionError::Timeout);
        }
        let enabled = Rc::new(Cell::new(false));
        let handler_clock = clock.clone();
        let emergency = Rc::new(RefCell::new(EmergencyChord::new()));
        let access = Rc::new(RefCell::new(AccessLease::new()));
        let handler = SessionHandler {
            enabled: enabled.clone(),
            actor: actor.clone(),
            emergency: emergency.clone(),
            access: access.clone(),
            clock: move || handler_clock.now(),
        };
        let hook =
            HookSession::install(Box::new(handler), tag, policy).map_err(|(error, _)| error)?;
        Ok(Self {
            hook,
            actor,
            enabled,
            clock,
            deadline,
            emergency,
            access,
            shutdown: None,
        })
    }
    pub fn phase(&self) -> ActorPhase {
        self.actor.borrow().phase()
    }
    pub fn shutdown_report(&self) -> Option<&ShutdownReport> {
        self.shutdown.as_ref()
    }
    /// Bounded message cycle. Errors immediately disable/unhook before recovery;
    /// inspect shutdown_report separately to retain both secondary results.
    pub fn poll(&mut self) -> Result<SessionStep, SessionError> {
        if self.shutdown.is_some() {
            return Err(SessionError::Closed);
        }
        let result = match self.poll_inner() {
            Err(SessionError::Hook(HookSessionError::Callback(HookFault::Emergency))) => {
                Ok(SessionStep::Emergency)
            }
            Err(SessionError::Hook(HookSessionError::Callback(HookFault::EmergencyDetector(
                error,
            )))) => Err(SessionError::EmergencyDetector(error)),
            Err(SessionError::Hook(HookSessionError::Callback(HookFault::AccessLease(error)))) => {
                Err(SessionError::AccessLease(error))
            }
            Err(SessionError::Hook(HookSessionError::Callback(HookFault::InputAccess(error)))) => {
                Err(SessionError::Probe(error))
            }
            result => result,
        };
        if result.is_err() || matches!(result, Ok(SessionStep::Quit | SessionStep::Emergency)) {
            self.shutdown();
        }
        result
    }
    fn poll_inner(&mut self) -> Result<SessionStep, SessionError> {
        if self.enabled.get() {
            let clock = self.clock.clone();
            let actor = self.actor.clone();
            let emergency = self.emergency.clone();
            let access = self.access.clone();
            return Ok(
                match self.hook.poll_with_control(&mut || {
                    monitored_wait(
                        &actor,
                        &emergency,
                        &access,
                        || clock.now(),
                        check_input_access,
                    )
                })? {
                    LoopStep::Continue => SessionStep::Continue,
                    LoopStep::Quit => SessionStep::Quit,
                },
            );
        }
        self.check_deadline(self.clock.now())?;
        let clock = self.clock.clone();
        let deadline = self.deadline;
        let mut control_error = None;
        let step = self.hook.poll_with_control(&mut || match clock.now() {
            Some(now) if now < deadline => Ok(WaitPlan::for_deadline(now, Some(deadline))),
            Some(_) => {
                control_error = Some(SessionError::Timeout);
                Err(HookFault::Handler)
            }
            None => {
                control_error = Some(SessionError::Clock);
                Err(HookFault::Handler)
            }
        });
        if let Some(error) = control_error {
            return Err(error);
        }
        if step? == LoopStep::Quit {
            return Ok(SessionStep::Quit);
        }
        self.try_activate(sample_input_state, |clock| clock.now())
    }
    fn check_deadline(&self, now: Option<MonoTime>) -> Result<(), SessionError> {
        if now.ok_or(SessionError::Clock)? >= self.deadline {
            Err(SessionError::Timeout)
        } else {
            Ok(())
        }
    }
    fn try_activate(
        &mut self,
        sample: impl FnOnce() -> Result<InputSample, ProbeError>,
        now: impl Fn(&SessionClock) -> Option<MonoTime>,
    ) -> Result<SessionStep, SessionError> {
        self.check_deadline(now(&self.clock))?;
        let before = self.hook.held_keys()?;
        // Win32 may deliver callbacks during queries: do not retain any borrow.
        let sample = sample()?;
        let after = self.hook.held_keys()?;
        let completed = now(&self.clock);
        self.check_deadline(completed)?;
        if sample == InputSample::Held || before != 0 || after != 0 {
            return Ok(SessionStep::Waiting);
        }
        // No native calls or message dispatch between the last check and switch.
        self.access
            .borrow_mut()
            .renew_after_probe(completed.ok_or(SessionError::Clock)?)?;
        self.actor.borrow_mut().start_after_neutral()?;
        self.enabled.set(true);
        Ok(SessionStep::Activated)
    }
    pub fn activate_config(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), SessionError> {
        if self.shutdown.is_some() {
            return Err(SessionError::Closed);
        }
        self.actor.borrow_mut().activate(config)?;
        Ok(())
    }
    /// Idempotent, bounded, Hook-close-first recovery. Unhook failure does not
    /// prevent recovery because close has already disabled callback dispatch.
    pub fn shutdown(&mut self) -> &ShutdownReport {
        self.enabled.set(false);
        self.shutdown.get_or_insert_with(|| {
            let (_, unhook) = self.hook.close();
            let recovery = self.actor.borrow_mut().stop_after_unhook();
            ShutdownReport { unhook, recovery }
        })
    }
}
impl<B: SendBackend + 'static> Drop for CaptureSession<B> {
    fn drop(&mut self) {
        self.enabled.set(false);
        let _ = self.hook.close();
    }
}
#[cfg(test)]
mod tests;
