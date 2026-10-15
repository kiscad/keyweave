//! Opt-in mixed native session; shared lifecycle gates live in parent module.
use super::*;
use crate::{
    mixed::SendMixedBackend,
    mixed_actor::MixedSessionActor,
    mouse_hook::{MouseFault, MouseHandler, MouseHookEvent},
};
#[derive(Debug)]
pub struct MixedShutdownReport {
    pub application_provider: Option<ProbeError>,
    pub access_lost: bool,
    pub session_changed: bool,
    pub notifications: NotificationCleanup,
    pub untracked_input: bool,
    pub unhook: Result<(), HookSessionError>,
    pub mouse_unhook: Result<(), HookSessionError>,
    pub recovery: Result<(), RuntimeError>,
}
struct MixedMouseHandler<B: SendMixedBackend, C: FnMut() -> Option<MonoTime>> {
    enabled: Rc<Cell<bool>>,
    actor: Rc<RefCell<MixedSessionActor<B>>>,
    emergency: Rc<RefCell<EmergencyChord>>,
    access: Rc<RefCell<AccessLease>>,
    clock: C,
}
impl<B: SendMixedBackend, C: FnMut() -> Option<MonoTime>> MouseHandler for MixedMouseHandler<B, C> {
    fn handle_mouse(&mut self, event: MouseHookEvent) -> Result<CaptureDecision, MouseFault> {
        if !self.enabled.get() {
            return Ok(CaptureDecision::Pass);
        }
        let Some(now) = (self.clock)() else {
            if let Ok(mut actor) = self.actor.try_borrow_mut() {
                actor.control_failed(RuntimeError::Clock);
            }
            return Err(MouseFault::Handler);
        };
        let gate = (|| {
            self.access
                .try_borrow()
                .map_err(|_| HookFault::Reentrant)?
                .check(now)
                .map_err(HookFault::AccessLease)?;
            guard(&self.emergency, now, None)?;
            Ok::<_, HookFault>(())
        })();
        if let Err(fault) = gate {
            super::super::hook_thread::stop_from_control(fault);
            return Err(MouseFault::Handler);
        }
        let mut actor = self
            .actor
            .try_borrow_mut()
            .map_err(|_| MouseFault::Handler)?;
        actor.mouse(event, now).map_err(|error| {
            actor.control_failed(error);
            MouseFault::Handler
        })
    }
}
pub struct MixedCaptureSession<B: SendMixedBackend + 'static> {
    application: ApplicationRefresh,
    session: SessionIdentity,
    progress_epoch: u64,
    notifications: SessionNotifications,
    hook: HookSession,
    actor: Rc<RefCell<MixedSessionActor<GuardedBackend<B>>>>,
    enabled: Rc<Cell<bool>>,
    clock: SessionClock,
    deadline: MonoTime,
    emergency: Rc<RefCell<EmergencyChord>>,
    access: Rc<RefCell<AccessLease>>,
    shutdown: Option<MixedShutdownReport>,
}
impl<B: SendMixedBackend + 'static> MixedCaptureSession<B> {
    pub fn install(
        backend: B,
        tag: InjectionTag,
        policy: IdentityPolicy,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, SessionError> {
        // Validate every output/condition before registering or touching input.
        let enabled = Rc::new(Cell::new(false));
        let actor = Rc::new(RefCell::new(MixedSessionActor::new(
            GuardedBackend {
                backend,
                enabled: enabled.clone(),
            },
            tag,
            config,
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
        let mut hook = match HookSession::install(Box::new(handler), tag, policy) {
            Ok(hook) => hook,
            Err((hook, _)) => {
                return Err(SessionError::HookInstall {
                    hook,
                    notifications: notifications.close(),
                });
            }
        };
        let mouse_clock = clock.clone();
        let mouse = MixedMouseHandler {
            enabled: enabled.clone(),
            actor: actor.clone(),
            emergency: emergency.clone(),
            access: access.clone(),
            clock: move || mouse_clock.now(),
        };
        if let Err((error, _)) = hook.install_mouse_handler(Box::new(mouse)) {
            let closed = hook.close_with_mouse();
            return Err(SessionError::MouseInstall {
                hook: error,
                keyboard_unhook: closed.keyboard_result,
                notifications: notifications.close(),
            });
        }
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
    pub fn shutdown_report(&self) -> Option<&MixedShutdownReport> {
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
            super::super::hook_thread::revoke_access();
        }
        if result.is_err() || matches!(result, Ok(SessionStep::Quit | SessionStep::Emergency)) {
            self.shutdown();
        }
        self.progress_epoch = next_epoch;
        result
    }
    fn check_notifications(&self) -> Result<(), SessionError> {
        if self.notifications.access_lost() {
            super::super::hook_thread::session_changed();
            return Err(HookSessionError::Callback(
                super::super::hook_thread::current_fault().unwrap_or(HookFault::SessionChanged),
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
                        super::super::application::sample,
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
        let before = self.hook.held_keys()? + self.hook.held_mouse_buttons()?;
        // Win32 may deliver callbacks during queries: do not retain any borrow.
        let sample = sample()?;
        check_input_access_in_session(self.session)?;
        self.check_notifications()?;
        let after = self.hook.held_keys()? + self.hook.held_mouse_buttons()?;
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
    pub fn shutdown(&mut self) -> &MixedShutdownReport {
        self.enabled.set(false);
        self.shutdown.get_or_insert_with(|| {
            let untracked_input = self.hook.untracked_input();
            let closed = self.hook.close_with_mouse();
            let unhook = closed.keyboard_result;
            let mouse_unhook = closed.mouse_result;
            let access_lost =
                super::super::hook_thread::access_lost() || self.notifications.access_lost();
            let recovery = if access_lost {
                self.actor
                    .borrow_mut()
                    .stop_without_delivery_after_unhook(RuntimeError::AccessLost)
            } else if untracked_input {
                self.actor
                    .borrow_mut()
                    .stop_without_delivery_after_unhook(RuntimeError::UntrackedInput)
            } else {
                self.actor.borrow_mut().stop_after_unhook()
            };
            let notifications = self.notifications.close();
            MixedShutdownReport {
                application_provider: self.application.failure,
                access_lost: access_lost
                    || super::super::hook_thread::access_lost()
                    || self.notifications.access_lost(),
                session_changed: self.notifications.access_lost(),
                notifications,
                untracked_input,
                unhook,
                mouse_unhook,
                recovery,
            }
        })
    }
}
impl<B: SendMixedBackend + 'static> Drop for MixedCaptureSession<B> {
    fn drop(&mut self) {
        self.enabled.set(false);
        let _ = self.hook.close();
        self.notifications.close();
    }
}

#[cfg(test)]
#[path = "mixed_tests.rs"]
mod tests;
