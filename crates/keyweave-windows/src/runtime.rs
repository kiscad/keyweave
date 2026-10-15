//! Single-thread engine/output owner. Lifecycle gates belong to the session host.
mod machine;
mod staged;
use crate::{
    hook::{HookEvent, HookFault, HookHandler},
    keys::{self, InjectionTag, KeyError},
    pump::{OutputPump, Phase, PumpError, SendBackend},
};
use keyweave_engine::{Condition, ConfigSnapshot, Engine, EngineError};
use keyweave_types::*;
use machine::Machine;
use staged::PendingInput;
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Arc};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    Legacy,
    StagedKeyboard,
}
pub const MAX_WORK: usize = 64;
pub const MAX_RECOVERY: usize = 8;
pub const MAX_PENDING_INPUTS: usize = 128;
const STREAM: DeviceInstanceId = DeviceInstanceId {
    id: 1,
    generation: 1,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorPhase {
    Standby,
    Active,
    StopRequired,
    Stopped,
    Unresolved,
}
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Mixed(#[from] crate::mixed::MixedError),
    #[error("application context provider failed: {0}")]
    ApplicationProvider(crate::startup::ProbeError),
    #[error("untracked input may have changed global output ownership")]
    UntrackedInput,
    #[error("desktop/session access was revoked")]
    AccessLost,
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Staged(#[from] keyweave_engine::physical_controller::PhysicalControllerError),
    #[error("keyboard session does not support staged pointer input or delivery")]
    StagedPointer,
    #[error("staged input remains queued; configuration activation must retry after drain")]
    StagedInputPending,
    #[error(transparent)]
    Output(#[from] PumpError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("Hook has no trustworthy per-event device identity")]
    DeviceCondition,
    #[error("actor requires a lifecycle gate")]
    NotActive,
    #[error("clock moved backwards")]
    Time,
    #[error("clock provider failed")]
    Clock,
    #[error("callback work budget exhausted")]
    Budget,
    #[error("sequence space exhausted")]
    Sequence,
    #[error("key capture policy disagrees with engine")]
    Decision,
    #[error("native output incomplete")]
    Incomplete,
    #[error("output recovery remains unresolved")]
    Unresolved,
}
fn validate(config: &ConfigSnapshot) -> Result<(), RuntimeError> {
    for mapping in config.simple.iter().chain(&config.complex) {
        if mapping
            .extras
            .conditions
            .iter()
            .chain(mapping.actions().flat_map(|action| &action.conditions))
            .any(|condition| matches!(condition, Condition::Device { .. }))
        {
            return Err(RuntimeError::DeviceCondition);
        }
        for action in mapping.actions() {
            for key in action
                .key()
                .into_iter()
                .chain(action.modifiers.iter().copied())
            {
                keys::output_scan(key)?;
            }
        }
    }
    Ok(())
}
fn prepare_engine(config: Arc<ConfigSnapshot>) -> Result<Engine, RuntimeError> {
    validate(&config)?;
    Ok(Engine::new(config)?)
}
/// Cold pure output/Engine admission. No backend/OS/clock/activation operations.
/// Doesn't prove input access, neutral state or actual output acceptance.
pub fn preflight_config(config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
    prepare_engine(config).map(drop)
}
/// Cold staged keyboard validation only; no neutral/access/output acceptance proof.
pub fn preflight_staged_config(config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
    Machine::staged(config).map(drop)
}
pub struct SessionActor<B: SendBackend> {
    engine: Machine,
    output: OutputPump<B>,
    phase: ActorPhase,
    now: MonoTime,
    seq: u64,
    failure: Option<RuntimeError>,
    pending_inputs: VecDeque<PendingInput>,
}
impl<B: SendBackend> SessionActor<B> {
    pub fn new(
        backend: B,
        tag: InjectionTag,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_mode(backend, tag, config, ExecutionMode::Legacy)
    }
    /// Explicit staged keyboard mode; lifecycle gates remain caller-owned.
    pub fn new_staged(
        backend: B,
        tag: InjectionTag,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, RuntimeError> {
        Self::new_with_mode(backend, tag, config, ExecutionMode::StagedKeyboard)
    }
    pub fn new_with_mode(
        backend: B,
        tag: InjectionTag,
        config: Arc<ConfigSnapshot>,
        mode: ExecutionMode,
    ) -> Result<Self, RuntimeError> {
        let engine = match mode {
            ExecutionMode::Legacy => Machine::Legacy(Box::new(prepare_engine(config)?)),
            ExecutionMode::StagedKeyboard => Machine::staged(config)?,
        };
        Ok(Self {
            engine,
            output: OutputPump::new(backend, tag),
            phase: ActorPhase::Standby,
            now: MonoTime(0),
            seq: 0,
            failure: None,
            pending_inputs: VecDeque::new(),
        })
    }
    pub fn execution_mode(&self) -> ExecutionMode {
        self.engine.execution_mode()
    }
    pub fn phase(&self) -> ActorPhase {
        self.phase
    }
    pub fn output(&self) -> &OutputPump<B> {
        &self.output
    }
    /// Detailed primary error preserved by the Hook adapter, without key logging.
    pub fn failure(&self) -> Option<&RuntimeError> {
        self.failure.as_ref()
    }
    pub(crate) fn control_failed(&mut self, error: RuntimeError) {
        if self.phase == ActorPhase::Active {
            self.phase = ActorPhase::StopRequired;
        }
        self.failure.get_or_insert(error);
    }
    pub fn next_wakeup(&self) -> Option<MonoTime> {
        if self.phase != ActorPhase::Active {
            return None;
        }
        [
            self.output.ledger().next_deadline(),
            self.engine.next_deadline(),
            self.pending_inputs.front().map(|input| input.at),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    /// Real output/session/physical neutral gates must be proved by the host.
    /// This actor cannot be restarted after stop with old captured bindings.
    pub fn start_after_neutral(&mut self) -> Result<(), RuntimeError> {
        if self.phase != ActorPhase::Standby {
            return Err(RuntimeError::NotActive);
        }
        self.output.resume()?;
        self.phase = ActorPhase::Active;
        Ok(())
    }
    /// Active snapshot metadata, separate from retained old down bindings.
    pub fn config_generation(&self) -> ConfigGeneration {
        self.engine.config_generation()
    }

    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
        if !matches!(self.phase, ActorPhase::Standby | ActorPhase::Active) {
            return Err(RuntimeError::NotActive);
        }
        if !self.pending_inputs.is_empty() {
            return Err(RuntimeError::StagedInputPending);
        }
        validate(&config)?;
        self.engine.activate(config)?;
        Ok(())
    }
    fn advance(&mut self, now: MonoTime) -> Result<(), RuntimeError> {
        if self.phase != ActorPhase::Active {
            return Err(RuntimeError::NotActive);
        }
        if now < self.now {
            return Err(RuntimeError::Time);
        }
        self.now = now;
        Ok(())
    }
    fn spend(work: &mut usize) -> Result<(), RuntimeError> {
        if *work == MAX_WORK {
            return Err(RuntimeError::Budget);
        }
        *work += 1;
        Ok(())
    }
    fn dispatch(
        &mut self,
        event: InputEvent,
        work: &mut usize,
    ) -> Result<CaptureDecision, RuntimeError> {
        self.dispatch_at(event, self.now, work)
    }
    fn dispatch_at(
        &mut self,
        event: InputEvent,
        at: MonoTime,
        work: &mut usize,
    ) -> Result<CaptureDecision, RuntimeError> {
        Self::spend(work)?;
        let seq = self.seq.checked_add(1).ok_or(RuntimeError::Sequence)?;
        let is_key = matches!(event, InputEvent::Key { .. });
        let step = self.engine.handle(InputEnvelope {
            seq,
            at_ns: at,
            device: STREAM,
            provenance: Provenance::Physical,
            event,
        })?;
        self.seq = seq;
        if is_key && step.capture != CaptureDecision::Capture {
            return Err(RuntimeError::Decision);
        }
        self.output.enqueue(&step.outputs)?;
        Ok(step.capture)
    }
    fn due(&mut self, work: &mut usize) -> Result<(), RuntimeError> {
        loop {
            if self.engine.is_staged() && *work == MAX_WORK {
                return Ok(());
            }
            let cutoff = self
                .pending_inputs
                .front()
                .map_or(self.now, |input| input.at.min(self.now));
            if self
                .output
                .ledger()
                .next_deadline()
                .is_some_and(|at| at <= self.now)
            {
                Self::spend(work)?;
                self.output.poll(self.now)?;
                if self.output.phase() != Phase::Normal {
                    return Err(RuntimeError::Incomplete);
                }
            } else if self.engine.stage_due(cutoff) {
                Self::spend(work)?;
                let step = self
                    .engine
                    .poll_stage(cutoff)?
                    .ok_or(RuntimeError::Decision)?;
                self.output.enqueue(&step.outputs)?;
            } else if self
                .pending_inputs
                .front()
                .is_some_and(|input| input.at <= self.now)
            {
                self.deliver_pending_input(work)?;
            } else if let Some(token) = self.engine.legacy_timer_due(self.now) {
                self.dispatch(InputEvent::TimerFired { token }, work)?;
            } else {
                return Ok(());
            }
        }
    }
    fn result<T>(&mut self, result: Result<T, RuntimeError>) -> Result<T, RuntimeError> {
        if result.is_err() && self.phase == ActorPhase::Active {
            self.phase = ActorPhase::StopRequired;
        }
        result
    }
    pub fn tick(&mut self, now: MonoTime) -> Result<(), RuntimeError> {
        let result = self.advance(now).and_then(|()| self.due(&mut 0));
        self.result(result)
    }
    /// Deliver a trusted provider snapshot outside the Hook callback. Due work
    /// uses the previous context before this control event, like ordinary input.
    /// The host supplies the actor's clock domain and merges independent fields;
    /// this method neither samples OS state nor proves provider completeness.
    /// Invalid snapshots stop capture; existing bindings still require unhook-
    /// first recovery. Updating context doesn't rematch held input or activate.
    pub fn context_changed(
        &mut self,
        context: ContextSnapshot,
        now: MonoTime,
    ) -> Result<(), RuntimeError> {
        let result = (|| {
            self.advance(now)?;
            if self.engine.is_staged() {
                self.enqueue_context(context, now)?;
                return self.due(&mut 0);
            }
            let mut work = 0;
            self.due(&mut work)?;
            self.dispatch(InputEvent::ContextChanged { context }, &mut work)?;
            self.due(&mut work)
        })();
        self.result(result)
    }
    /// Called synchronously by HookDispatcher, with resolved Scan 1 identity.
    pub fn key(
        &mut self,
        event: HookEvent,
        now: MonoTime,
    ) -> Result<CaptureDecision, RuntimeError> {
        let result = (|| {
            self.advance(now)?;
            if event.binding == Some(CaptureDecision::Pass) {
                return Err(RuntimeError::Decision);
            }
            if self.engine.is_staged() {
                self.enqueue_input(
                    InputEvent::Key {
                        key: event.identity.key,
                        edge: event.edge,
                    },
                    now,
                )?;
                self.due(&mut 0)?;
                return Ok(CaptureDecision::Capture);
            }
            let mut work = 0;
            self.due(&mut work)?;
            let input = InputEvent::Key {
                key: event.identity.key,
                edge: event.edge,
            };
            let decision = loop {
                match self.dispatch(input.clone(), &mut work) {
                    Err(RuntimeError::Engine(EngineError::CancellationPending)) => {
                        let token = self
                            .engine
                            .next_delayed()
                            .ok_or(EngineError::InvalidDeadline)?;
                        self.dispatch(InputEvent::CancelDelayed { token }, &mut work)?;
                        self.due(&mut work)?;
                    }
                    result => break result?,
                }
            };
            self.due(&mut work)?;
            if event.edge == KeyEdge::Repeat
                && let Some(key) = self
                    .engine
                    .physical_repeat_target(STREAM, event.identity.key)
                && self.output.ledger().desired().contains(&key)
                && self.output.ledger().submitted().contains(&key)
            {
                self.output.enqueue(&[OutputIntent {
                    at_ns: self.now,
                    key,
                    edge: KeyEdge::Repeat,
                }])?;
                self.due(&mut work)?;
            }
            Ok(decision)
        })();
        self.result(result)
    }
    /// Host MUST disable/unhook first. An untracked source can invalidate global
    /// ownership even when its native code appears unrelated. Never release a
    /// possibly shared down; only a known empty ledger can stop normally.
    pub fn stop_for_untracked_input_after_unhook(&mut self) -> Result<(), RuntimeError> {
        self.stop_without_delivery_after_unhook(RuntimeError::UntrackedInput)
    }
    /// A revoked desktop/session cannot receive recovery ups. Preserve unknown
    /// or nonempty submitted evidence; only known empty output can stop cleanly.
    pub fn stop_for_access_loss_after_unhook(&mut self) -> Result<(), RuntimeError> {
        self.stop_without_delivery_after_unhook(RuntimeError::AccessLost)
    }
    fn stop_without_delivery_after_unhook(
        &mut self,
        reason: RuntimeError,
    ) -> Result<(), RuntimeError> {
        if matches!(self.phase, ActorPhase::Stopped | ActorPhase::Unresolved) {
            return Err(RuntimeError::NotActive);
        }
        self.failure.get_or_insert(reason);
        if self.output.ledger().submitted().is_empty()
            && !matches!(self.output.phase(), Phase::Quarantined | Phase::Closed)
        {
            return self.stop_after_unhook();
        }
        self.phase = ActorPhase::Unresolved;
        self.pending_inputs.clear();
        let _ = self.engine.reset(self.now);
        self.output.quarantine_without_delivery();
        Err(RuntimeError::Unresolved)
    }
    /// Host MUST disable/unhook first. No OS calls or fabricated neutral proof.
    /// Use last trusted time; a failed clock must not prevent bounded recovery.
    pub fn stop_after_unhook(&mut self) -> Result<(), RuntimeError> {
        if matches!(self.phase, ActorPhase::Stopped | ActorPhase::Unresolved) {
            return Err(RuntimeError::NotActive);
        }
        self.phase = ActorPhase::StopRequired;
        self.pending_inputs.clear();
        let reset = self.engine.reset(self.now);
        if self.output.begin_recovery(self.now).is_err() {
            self.phase = ActorPhase::Unresolved;
            return Err(RuntimeError::Unresolved);
        }
        for _ in 0..MAX_RECOVERY {
            if self.output.phase() == Phase::Paused {
                break;
            }
            // Known zero/rejected recovery may retry; unknown/invalid receipt
            // stays quarantined. No sleeping or new clock/provider invocation.
            let _ = self.output.poll(self.now);
            if matches!(self.output.phase(), Phase::Quarantined | Phase::Closed) {
                break;
            }
        }
        if self.output.phase() != Phase::Paused {
            self.phase = ActorPhase::Unresolved;
            return Err(RuntimeError::Unresolved);
        }
        self.phase = ActorPhase::Stopped;
        reset?; // Actual accepted ledger cleanup ran even if Engine reset failed.
        Ok(())
    }
}
/// Same-thread bridge. Host retains Rc for tick/config and unhook-first recovery.
/// Clock must be bounded and return explicit monotonic delivery time.
pub struct ActorHandler<B: SendBackend, C: FnMut() -> Option<MonoTime>> {
    pub actor: Rc<RefCell<SessionActor<B>>>,
    pub clock: C,
}
impl<B: SendBackend, C: FnMut() -> Option<MonoTime>> HookHandler for ActorHandler<B, C> {
    fn handle(&mut self, event: HookEvent) -> Result<CaptureDecision, HookFault> {
        let mut actor = self
            .actor
            .try_borrow_mut()
            .map_err(|_| HookFault::Reentrant)?;
        let Some(now) = (self.clock)() else {
            actor.phase = ActorPhase::StopRequired;
            actor.failure.get_or_insert(RuntimeError::Clock);
            return Err(HookFault::Handler);
        };
        match actor.key(event, now) {
            Ok(decision) => Ok(decision),
            Err(error) => {
                actor.failure.get_or_insert(error);
                Err(HookFault::Handler)
            }
        }
    }
}

#[cfg(test)]
mod tests;
