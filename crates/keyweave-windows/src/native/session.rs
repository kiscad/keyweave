//! Explicit same-thread passive startup; no executable or automatic capture.
use super::{
    hook_thread::{HookSession, HookSessionError, LoopStep},
    input_probe::sample_input_state,
};
use crate::{
    hook::{HookEvent, HookFault, HookHandler},
    keys::{IdentityPolicy, InjectionTag},
    pump::SendBackend,
    reactor::{SessionClock, SessionControl, WaitPlan},
    runtime::{ActorHandler, ActorPhase, RuntimeError, SessionActor},
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
    #[error("session is closed")]
    Closed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStep {
    Waiting,
    Activated,
    Continue,
    Quit,
}
#[derive(Debug)]
pub struct ShutdownReport {
    pub unhook: Result<(), HookSessionError>,
    pub recovery: Result<(), RuntimeError>,
}
struct Passive<H> {
    enabled: Rc<Cell<bool>>,
    handler: H,
}
impl<H: HookHandler> HookHandler for Passive<H> {
    fn handle(&mut self, event: HookEvent) -> Result<CaptureDecision, HookFault> {
        if self.enabled.get() {
            self.handler.handle(event)
        } else {
            Ok(CaptureDecision::Pass)
        }
    }
}
/// Construct only on the dedicated session thread. This component does not
/// supply repeat, emergency, desktop supervision or a deployment-ready host.
/// Call shutdown explicitly: Drop disables the Hook without injecting releases.
pub struct CaptureSession<B: SendBackend + 'static> {
    hook: HookSession,
    actor: Rc<RefCell<SessionActor<B>>>,
    enabled: Rc<Cell<bool>>,
    clock: SessionClock,
    deadline: MonoTime,
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
        let handler = Passive {
            enabled: enabled.clone(),
            handler: ActorHandler {
                actor: actor.clone(),
                clock: move || handler_clock.now(),
            },
        };
        let hook =
            HookSession::install(Box::new(handler), tag, policy).map_err(|(error, _)| error)?;
        Ok(Self {
            hook,
            actor,
            enabled,
            clock,
            deadline,
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
        let result = self.poll_inner();
        if result.is_err() || matches!(result, Ok(SessionStep::Quit)) {
            self.shutdown();
        }
        result
    }
    fn poll_inner(&mut self) -> Result<SessionStep, SessionError> {
        if self.enabled.get() {
            let clock = self.clock.clone();
            let mut control = SessionControl {
                actor: self.actor.clone(),
                clock: move || clock.now(),
            };
            return Ok(
                match self.hook.poll_with_control(&mut || control.next_wait())? {
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
        self.check_deadline(now(&self.clock))?;
        if sample == InputSample::Held || before != 0 || after != 0 {
            return Ok(SessionStep::Waiting);
        }
        // No native calls or message dispatch between the last check and switch.
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
