//! Explicit same-thread passive startup; no executable or automatic capture.
use super::{
    hook_thread::{HookSession, HookSessionError, LoopStep},
    input_probe::{check_input_access_in_session, sample_input_state},
    session_info::{SessionIdentity, own_session},
    session_notifications::{NotificationCleanup, NotificationError, SessionNotifications},
};
use crate::{
    access::{AccessError, AccessLease},
    emergency::{EmergencyChord, EmergencyError},
    hook::{HookEvent, HookFault, HookHandler},
    keys::{IdentityPolicy, InjectionTag, KeyboardPacket},
    pump::{Insertion, SendBackend, SubmitError},
    reactor::{SessionClock, WaitPlan},
    runtime::{ActorPhase, RuntimeError, SessionActor},
    startup::{InputSample, ProbeError},
};
use keyweave_engine::ConfigSnapshot;
use keyweave_types::{CaptureDecision, ContextSnapshot, DeviceInventoryScope, MonoTime};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
mod mixed;
pub use mixed::{MixedCaptureSession, MixedShutdownReport};

const STARTUP_NS: u64 = 2_000_000_000;
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(
        "mouse Hook installation failed: {hook}; keyboard removal: {keyboard_unhook:?}; notification cleanup: {notifications:?}"
    )]
    MouseInstall {
        hook: HookSessionError,
        keyboard_unhook: Result<(), HookSessionError>,
        notifications: NotificationCleanup,
    },
    #[error(transparent)]
    Probe(#[from] ProbeError),
    #[error(transparent)]
    Notifications(#[from] NotificationError),
    #[error("Hook installation failed: {hook}; notification cleanup: {notifications:?}")]
    HookInstall {
        hook: HookSessionError,
        notifications: NotificationCleanup,
    },
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
    #[error("runtime progress identity exhausted")]
    ProgressExhausted,
    #[error("session actor is already borrowed")]
    ActorBorrow,
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
    pub application_provider: Option<ProbeError>,
    pub access_lost: bool,
    pub session_changed: bool,
    pub notifications: NotificationCleanup,
    pub untracked_input: bool,
    pub unhook: Result<(), HookSessionError>,
    pub recovery: Result<(), RuntimeError>,
}
#[derive(Default)]
struct ApplicationRefresh {
    next: Option<MonoTime>,
    revision: u64,
    failure: Option<ProbeError>,
}
const APPLICATION_REFRESH_NS: u64 = 100_000_000;
const APPLICATION_LIFETIME_NS: u64 = 500_000_000;
trait SessionControl {
    fn control_failed(&mut self, error: RuntimeError);
    fn context_changed(
        &mut self,
        context: ContextSnapshot,
        now: MonoTime,
    ) -> Result<(), RuntimeError>;
    fn tick(&mut self, now: MonoTime) -> Result<(), RuntimeError>;
    fn next_wakeup(&self) -> Option<MonoTime>;
    fn key(&mut self, event: HookEvent, now: MonoTime) -> Result<CaptureDecision, RuntimeError>;
}
impl<B: SendBackend> SessionControl for SessionActor<B> {
    fn control_failed(&mut self, e: RuntimeError) {
        self.control_failed(e)
    }
    fn context_changed(&mut self, c: ContextSnapshot, n: MonoTime) -> Result<(), RuntimeError> {
        self.context_changed(c, n)
    }
    fn tick(&mut self, n: MonoTime) -> Result<(), RuntimeError> {
        self.tick(n)
    }
    fn next_wakeup(&self) -> Option<MonoTime> {
        self.next_wakeup()
    }
    fn key(&mut self, e: HookEvent, n: MonoTime) -> Result<CaptureDecision, RuntimeError> {
        self.key(e, n)
    }
}
impl<B: crate::mixed::SendMixedBackend> SessionControl
    for crate::mixed_actor::MixedSessionActor<B>
{
    fn control_failed(&mut self, e: RuntimeError) {
        self.control_failed(e)
    }
    fn context_changed(&mut self, c: ContextSnapshot, n: MonoTime) -> Result<(), RuntimeError> {
        self.context_changed(c, n)
    }
    fn tick(&mut self, n: MonoTime) -> Result<(), RuntimeError> {
        self.tick(n)
    }
    fn next_wakeup(&self) -> Option<MonoTime> {
        self.next_wakeup()
    }
    fn key(&mut self, e: HookEvent, n: MonoTime) -> Result<CaptureDecision, RuntimeError> {
        self.key(e, n)
    }
}
fn refresh_application<A: SessionControl>(
    state: &mut ApplicationRefresh,
    actor: &RefCell<A>,
    emergency: &RefCell<EmergencyChord>,
    access: &RefCell<AccessLease>,
    mut clock: impl FnMut() -> Option<MonoTime>,
    sample: impl FnOnce() -> Result<Option<keyweave_types::ApplicationContext>, ProbeError>,
) -> Result<WaitPlan, HookFault> {
    let Some(before) = clock() else {
        return active_wait(actor, emergency, access, None);
    };
    if state.next.is_some_and(|next| before < next) {
        return active_wait(actor, emergency, access, Some(before));
    }
    access
        .try_borrow()
        .map_err(|_| HookFault::Reentrant)?
        .check(before)
        .map_err(HookFault::AccessLease)?;
    guard(emergency, before, None)?;
    // No actor/lease borrow spans potentially reentrant metadata calls.
    let sampled = sample();
    if let Err(error) = sampled {
        state.failure.get_or_insert(error);
    }
    if let Some(fault) = super::hook_thread::current_fault() {
        return Err(fault);
    }
    let application = sampled.map_err(|error| {
        if let Ok(mut actor) = actor.try_borrow_mut() {
            actor.control_failed(RuntimeError::ApplicationProvider(error));
        }
        HookFault::Handler
    })?;
    let Some(after) = clock() else {
        return active_wait(actor, emergency, access, None);
    };
    access
        .try_borrow()
        .map_err(|_| HookFault::Reentrant)?
        .check(after)
        .map_err(HookFault::AccessLease)?;
    guard(emergency, after, None)?;
    let result = (|| {
        if after < before {
            return Err(RuntimeError::Time);
        }
        let valid_until = MonoTime(
            before
                .0
                .checked_add(APPLICATION_LIFETIME_NS)
                .ok_or(RuntimeError::Time)?,
        );
        let next = MonoTime(
            after
                .0
                .checked_add(APPLICATION_REFRESH_NS)
                .ok_or(RuntimeError::Time)?,
        );
        let revision = state
            .revision
            .checked_add(1)
            .ok_or(RuntimeError::Sequence)?;
        let context = ContextSnapshot {
            revision,
            provider: "windows-foreground-image-v1".into(),
            observed_at_ns: before,
            valid_until_ns: valid_until,
            application: if after < valid_until {
                application
            } else {
                None
            },
            devices: None,
            device_inventory_scope: DeviceInventoryScope::Partial,
            input_source: None,
        };
        actor
            .try_borrow_mut()
            .map_err(|_| RuntimeError::NotActive)?
            .context_changed(context, after)?;
        state.revision = revision;
        state.next = Some(next);
        Ok(())
    })();
    if let Err(error) = result {
        if let Ok(mut actor) = actor.try_borrow_mut() {
            actor.control_failed(error);
        }
        return Err(HookFault::Handler);
    }
    active_wait(actor, emergency, access, Some(after))
}
struct GuardedBackend<B> {
    backend: B,
    enabled: Rc<Cell<bool>>,
}
impl<B: SendBackend> SendBackend for GuardedBackend<B> {
    fn submit(&mut self, packets: &[KeyboardPacket]) -> Result<Insertion, SubmitError> {
        // An earlier native submission may have delivered a reentrant fault.
        // Reject only before calling this backend; its actual prior receipt was
        // acknowledged by the pump. Shutdown disables the generic fault gate;
        // revoked access independently forbids recovery native submissions.
        if !delivery_allowed(self.enabled.get()) {
            return Err(SubmitError::Rejected);
        }
        self.backend.submit(packets)
    }
}
fn delivery_allowed(enabled: bool) -> bool {
    !super::hook_thread::access_lost()
        && !super::session_notifications::access_lost()
        && !(enabled && super::hook_thread::current_fault().is_some())
}
impl<B: crate::mixed::SendMixedBackend> crate::mixed::SendMixedBackend for GuardedBackend<B> {
    fn submit_mixed(
        &mut self,
        packets: &[crate::mixed::NativePacket],
    ) -> Result<Insertion, SubmitError> {
        if !delivery_allowed(self.enabled.get()) {
            return Err(SubmitError::Rejected);
        }
        self.backend.submit_mixed(packets)
    }
}
struct SessionHandler<A: SessionControl, C: FnMut() -> Option<MonoTime>> {
    enabled: Rc<Cell<bool>>,
    actor: Rc<RefCell<A>>,
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
impl<A: SessionControl, C: FnMut() -> Option<MonoTime>> HookHandler for SessionHandler<A, C> {
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
fn active_wait<A: SessionControl>(
    actor: &RefCell<A>,
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
fn monitored_wait<A: SessionControl>(
    actor: &RefCell<A>,
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
        if observed.is_err() {
            // Keep independent access evidence even if a sent callback already
            // selected the primary fault. No actor/dispatcher borrow is needed.
            super::hook_thread::revoke_access();
        }
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
/// supply independent progress/session supervision or a deployment-ready host.
/// Call shutdown explicitly: Drop disables the Hook without injecting releases.
pub struct CaptureSession<B: SendBackend + 'static> {
    application: ApplicationRefresh,
    session: SessionIdentity,
    progress_epoch: u64,
    notifications: SessionNotifications,
    hook: HookSession,
    actor: Rc<RefCell<SessionActor<GuardedBackend<B>>>>,
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
        Self::install_with_mode(
            backend,
            tag,
            policy,
            config,
            crate::runtime::ExecutionMode::Legacy,
        )
    }
    pub fn install_with_mode(
        backend: B,
        tag: InjectionTag,
        policy: IdentityPolicy,
        config: Arc<ConfigSnapshot>,
        mode: crate::runtime::ExecutionMode,
    ) -> Result<Self, SessionError> {
        // Validate every output/condition before registering or touching input.
        let enabled = Rc::new(Cell::new(false));
        let actor = Rc::new(RefCell::new(SessionActor::new_with_mode(
            GuardedBackend {
                backend,
                enabled: enabled.clone(),
            },
            tag,
            config,
            mode,
        )?));
        let clock = SessionClock::new();
        let now = clock.now().ok_or(SessionError::Clock)?;
        let deadline = MonoTime(now.0.checked_add(STARTUP_NS).ok_or(SessionError::Clock)?);
        let session = own_session()?;
        sample_input_state()?;
        if clock.now().ok_or(SessionError::Clock)? >= deadline {
            return Err(SessionError::Timeout);
        }
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
        let mut notifications = SessionNotifications::install_for(session)?;
        let hook = match HookSession::install(Box::new(handler), tag, policy) {
            Ok(hook) => hook,
            Err((hook, _)) => {
                return Err(SessionError::HookInstall {
                    hook,
                    notifications: notifications.close(),
                });
            }
        };
        Ok(Self {
            application: ApplicationRefresh::default(),
            session,
            progress_epoch: 0,
            notifications,
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
    pub fn progress_epoch(&self) -> u64 {
        self.progress_epoch
    }
    pub fn progress_phase(&self) -> Option<keyweave_progress::ProgressPhase> {
        use keyweave_progress::ProgressPhase;
        match self.phase() {
            ActorPhase::Standby => None,
            ActorPhase::Active => Some(ProgressPhase::Active),
            ActorPhase::StopRequired => Some(ProgressPhase::Recovering),
            ActorPhase::Stopped | ActorPhase::Unresolved => Some(ProgressPhase::Stopped),
        }
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
        let Some(next_epoch) = self.progress_epoch.checked_add(1) else {
            self.shutdown();
            return Err(SessionError::ProgressExhausted);
        };
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
        if matches!(
            &result,
            Err(SessionError::Probe(_) | SessionError::AccessLease(_))
        ) {
            super::hook_thread::revoke_access();
        }
        if result.is_err() || matches!(result, Ok(SessionStep::Quit | SessionStep::Emergency)) {
            self.shutdown();
        }
        self.progress_epoch = next_epoch;
        result
    }
    fn check_notifications(&self) -> Result<(), SessionError> {
        if self.notifications.access_lost() {
            super::hook_thread::session_changed();
            return Err(HookSessionError::Callback(
                super::hook_thread::current_fault().unwrap_or(HookFault::SessionChanged),
            )
            .into());
        }
        Ok(())
    }
    fn poll_inner(&mut self) -> Result<SessionStep, SessionError> {
        self.check_notifications()?;
        if self.enabled.get() {
            let session = self.session;
            let clock = self.clock.clone();
            let actor = self.actor.clone();
            let emergency = self.emergency.clone();
            let access = self.access.clone();
            let application = &mut self.application;
            return Ok(
                match self.hook.poll_with_control(&mut || {
                    monitored_wait(
                        &actor,
                        &emergency,
                        &access,
                        || clock.now(),
                        || check_input_access_in_session(session),
                    )?;
                    refresh_application(
                        application,
                        &actor,
                        &emergency,
                        &access,
                        || clock.now(),
                        super::application::sample,
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
        check_input_access_in_session(self.session)?;
        self.check_notifications()?;
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
        self.hook.guard_untracked_input()?;
        self.enabled.set(true);
        Ok(SessionStep::Activated)
    }
    /// Outside callback only; observed generation is not an input lease/epoch.
    pub fn config_generation(&self) -> Result<keyweave_types::ConfigGeneration, SessionError> {
        if self.shutdown.is_some() {
            return Err(SessionError::Closed);
        }
        crate::startup::ensure_preparation_context()?;
        Ok(self
            .actor
            .try_borrow()
            .map_err(|_| SessionError::ActorBorrow)?
            .config_generation())
    }
    pub fn activate_config(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), SessionError> {
        if self.shutdown.is_some() {
            return Err(SessionError::Closed);
        }
        crate::startup::ensure_preparation_context()?;
        self.actor
            .try_borrow_mut()
            .map_err(|_| SessionError::ActorBorrow)?
            .activate(config)?;
        Ok(())
    }
    /// Idempotent, bounded, Hook-close-first recovery. Unhook failure does not
    /// prevent recovery because close has already disabled callback dispatch.
    pub fn shutdown(&mut self) -> &ShutdownReport {
        self.enabled.set(false);
        self.shutdown.get_or_insert_with(|| {
            let untracked_input = self.hook.untracked_input();
            let (_, unhook) = self.hook.close();
            let access_lost = super::hook_thread::access_lost() || self.notifications.access_lost();
            let recovery = if access_lost {
                self.actor.borrow_mut().stop_for_access_loss_after_unhook()
            } else if untracked_input {
                self.actor
                    .borrow_mut()
                    .stop_for_untracked_input_after_unhook()
            } else {
                self.actor.borrow_mut().stop_after_unhook()
            };
            let notifications = self.notifications.close();
            ShutdownReport {
                application_provider: self.application.failure,
                access_lost: access_lost
                    || super::hook_thread::access_lost()
                    || self.notifications.access_lost(),
                session_changed: self.notifications.access_lost(),
                notifications,
                untracked_input,
                unhook,
                recovery,
            }
        })
    }
}
impl<B: SendBackend + 'static> Drop for CaptureSession<B> {
    fn drop(&mut self) {
        self.enabled.set(false);
        let _ = self.hook.close();
        self.notifications.close();
    }
}
#[cfg(test)]
mod tests;
#[cfg(test)]
use super::input_probe::check_input_access;
