//! One cold preparation job per control owner. Never call from input callbacks.
//! No native input, file IO, progress, clock or acknowledgement writes.
use crate::{linux_preparation as linux, preparation as windows};
use keyweave_engine::{ConfigSnapshot, Engine};
use keyweave_progress::reload::{
    Ack, Outcome, ReloadError, Request,
    transport::{Received, Receiver, TransferError},
};
use keyweave_types::ConfigGeneration;
use std::{io::Read, sync::Arc, thread::JoinHandle};

#[derive(Clone)]
pub enum Target {
    Linux(keyweave_linux::output::DeviceSpec),
    LinuxStaged(keyweave_linux::output::DeviceSpec),
    LinuxRelative(keyweave_linux::output::DeviceSpec),
    Windows,
    WindowsStaged,
    WindowsMixed,
}
impl Target {
    pub fn windows_prepared(prepared: &windows::PreparedConfig) -> Self {
        match (prepared.capture_scope(), prepared.execution_mode()) {
            (windows::CaptureScope::StagedMixed, _) => Self::WindowsMixed,
            (_, windows::ExecutionMode::StagedKeyboard) => Self::WindowsStaged,
            _ => Self::Windows,
        }
    }
    pub fn linux_prepared(prepared: &linux::PreparedConfig) -> Self {
        match (prepared.capture_scope(), prepared.execution_mode()) {
            (linux::CaptureScope::StagedRelative, _) => {
                Self::LinuxRelative(prepared.output_spec().clone())
            }
            (linux::CaptureScope::Keyboard, linux::ExecutionMode::StagedKeyboard) => {
                Self::LinuxStaged(prepared.output_spec().clone())
            }
            (linux::CaptureScope::Keyboard, linux::ExecutionMode::Legacy) => {
                Self::Linux(prepared.output_spec().clone())
            }
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Platform {
    Linux,
    Windows,
}
struct Candidate {
    platform: Platform,
    snapshot: Arc<ConfigSnapshot>,
    linux_mode: Option<keyweave_linux::runtime::ExecutionMode>,
    linux_scope: Option<linux::CaptureScope>,
    windows_mode: Option<keyweave_windows::runtime::ExecutionMode>,
    windows_scope: Option<windows::CaptureScope>,
}
struct Completion {
    request: Request,
    candidate: Option<Candidate>,
}

#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    #[error(transparent)]
    Protocol(#[from] ReloadError),
    #[error(transparent)]
    Context(#[from] keyweave_windows::startup::ProbeError),
    #[error("cold preparation thread could not start")]
    Spawn,
    #[error("cold preparation thread panicked")]
    Panicked,
    #[error(transparent)]
    Transfer(#[from] TransferError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationStep {
    Idle,
    Preparing,
    Ready,
    Closed,
}

/// Keep the same owner after Stop until its running job is reaped. Close never
/// waits for compilation or starts a replacement. Drop detaches an unfinished
/// job; native integration must retain/poll this owner during bounded recovery.
/// Thread/allocator/OS scheduling costs are not hard realtime bounds.
#[derive(Default)]
pub struct PreparationSlot {
    job: Option<JoinHandle<Completion>>,
    ready: Option<Completion>,
    closed: bool,
}
impl PreparationSlot {
    pub fn new() -> Self {
        Self::default()
    }
    /// Metadata only; closed control owners retain this job until reaped.
    pub fn job_pending(&self) -> bool {
        self.job.is_some()
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.ready = None;
    }
    pub fn start(&mut self, received: Received, target: Target) -> Result<(), PreparationError> {
        keyweave_windows::startup::ensure_preparation_context()?;
        if self.closed {
            return Err(ReloadError::Closed.into());
        }
        if self.job.is_some() || self.ready.is_some() {
            return Err(ReloadError::Busy.into());
        }
        let request = received.request;
        if let Err(error) = request.validate() {
            self.close();
            return Err(error.into());
        }
        if received.body.len() as u64 != request.byte_length {
            self.close();
            return Err(ReloadError::Length.into());
        }
        match std::thread::Builder::new()
            .name("keyweave-config".into())
            .spawn(move || {
                let candidate = match target {
                    Target::Linux(spec) => linux::PreparedConfig::from_bytes_for_generation(
                        received.body,
                        spec,
                        request.target,
                    )
                    .ok()
                    .map(|p| Candidate {
                        platform: Platform::Linux,
                        snapshot: p.snapshot().clone(),
                        linux_mode: Some(p.execution_mode()),
                        linux_scope: Some(p.capture_scope()),
                        windows_mode: None,
                        windows_scope: None,
                    }),
                    Target::LinuxStaged(spec) => {
                        linux::PreparedConfig::from_bytes_for_generation_with_mode(
                            received.body,
                            spec,
                            request.target,
                            keyweave_linux::runtime::ExecutionMode::StagedKeyboard,
                        )
                        .ok()
                        .map(|p| Candidate {
                            platform: Platform::Linux,
                            snapshot: p.snapshot().clone(),
                            linux_mode: Some(p.execution_mode()),
                            linux_scope: Some(p.capture_scope()),
                            windows_mode: None,
                            windows_scope: None,
                        })
                    }
                    Target::LinuxRelative(spec) => {
                        linux::PreparedConfig::from_bytes_for_generation_with_scope(
                            received.body,
                            spec,
                            request.target,
                            linux::ExecutionMode::StagedKeyboard,
                            linux::CaptureScope::StagedRelative,
                        )
                        .ok()
                        .map(|p| Candidate {
                            platform: Platform::Linux,
                            snapshot: p.snapshot().clone(),
                            linux_mode: Some(p.execution_mode()),
                            linux_scope: Some(p.capture_scope()),
                            windows_mode: None,
                            windows_scope: None,
                        })
                    }
                    Target::WindowsMixed => {
                        windows::PreparedConfig::from_bytes_for_generation_with_scope(
                            received.body,
                            request.target,
                            windows::ExecutionMode::StagedKeyboard,
                            windows::CaptureScope::StagedMixed,
                        )
                        .ok()
                        .map(|p| Candidate {
                            platform: Platform::Windows,
                            snapshot: p.snapshot().clone(),
                            linux_mode: None,
                            linux_scope: None,
                            windows_mode: Some(p.execution_mode()),
                            windows_scope: Some(p.capture_scope()),
                        })
                    }
                    Target::WindowsStaged => {
                        windows::PreparedConfig::from_bytes_for_generation_with_mode(
                            received.body,
                            request.target,
                            keyweave_windows::runtime::ExecutionMode::StagedKeyboard,
                        )
                        .ok()
                        .map(|p| Candidate {
                            platform: Platform::Windows,
                            snapshot: p.snapshot().clone(),
                            linux_mode: None,
                            linux_scope: None,
                            windows_mode: Some(p.execution_mode()),
                            windows_scope: Some(p.capture_scope()),
                        })
                    }
                    Target::Windows => windows::PreparedConfig::from_bytes_for_generation(
                        received.body,
                        request.target,
                    )
                    .ok()
                    .map(|p| Candidate {
                        platform: Platform::Windows,
                        snapshot: p.snapshot().clone(),
                        linux_mode: None,
                        linux_scope: None,
                        windows_mode: Some(p.execution_mode()),
                        windows_scope: Some(p.capture_scope()),
                    }),
                };
                // Exact bytes and cold diagnostics are dropped here, not on the
                // actor's input path. Only immutable IR crosses back to control.
                Completion { request, candidate }
            }) {
            Ok(job) => {
                self.job = Some(job);
                Ok(())
            }
            Err(_) => {
                self.close();
                Err(PreparationError::Spawn)
            }
        }
    }
    /// No join while the job is running. Closed owners still reap a completed
    /// job and discard its result; a late success never reopens the lane.
    pub fn poll(&mut self) -> Result<PreparationStep, PreparationError> {
        keyweave_windows::startup::ensure_preparation_context()?;
        if self.job.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(job) = self.job.take()
        {
            match job.join() {
                Ok(result) if !self.closed => self.ready = Some(result),
                Ok(_) => {}
                Err(_) => {
                    self.close();
                    return Err(PreparationError::Panicked);
                }
            }
        }
        Ok(if self.closed {
            PreparationStep::Closed
        } else if self.ready.is_some() {
            PreparationStep::Ready
        } else if self.job.is_some() {
            PreparationStep::Preparing
        } else {
            PreparationStep::Idle
        })
    }
    /// Control-thread only, after independent Stop/lease/deadline checks. This
    /// applies on the actual actor, then builds ack from its current generation.
    /// No output IO or progress is performed. AckWriter/AckSent remain required.
    pub fn deliver<R: Read, A: ReloadActor>(
        &mut self,
        receiver: &mut Receiver<R>,
        actor: &mut A,
    ) -> Result<Ack, PreparationError> {
        if let Err(error) = keyweave_windows::startup::ensure_preparation_context() {
            self.close();
            receiver.close();
            return Err(error.into());
        }
        if self.closed {
            receiver.close();
            return Err(ReloadError::Closed.into());
        }
        let Some(result) = self.ready.take() else {
            return Err(ReloadError::NoPending.into());
        };
        if !receiver.awaiting_commit(result.request) || actor.generation() != result.request.base {
            self.close();
            receiver.close();
            return Err(ReloadError::Generation.into());
        }
        let accepted = result.candidate.is_some_and(|candidate| {
            candidate.snapshot.generation == result.request.target
                && actor.supports(candidate.platform == Platform::Linux)
                && candidate
                    .linux_mode
                    .is_none_or(|mode| actor.supports_linux_mode(mode))
                && candidate
                    .linux_scope
                    .is_none_or(|scope| actor.supports_linux_scope(scope))
                && candidate
                    .windows_mode
                    .is_none_or(|mode| actor.supports_windows_mode(mode))
                && candidate
                    .windows_scope
                    .is_none_or(|scope| actor.supports_windows_scope(scope))
                && actor.apply(candidate.snapshot)
        });
        let outcome = if accepted {
            Outcome::Accepted
        } else {
            Outcome::Rejected
        };
        match receiver.finish(outcome, actor.generation()) {
            Ok(ack) => Ok(ack),
            Err(error) => {
                self.close();
                Err(error.into())
            }
        }
    }
}

mod sealed {
    use super::*;
    pub trait Actor {
        fn generation(&self) -> ConfigGeneration;
        fn supports(&self, linux: bool) -> bool;
        fn supports_linux_mode(&self, mode: keyweave_linux::runtime::ExecutionMode) -> bool {
            mode == keyweave_linux::runtime::ExecutionMode::Legacy
        }
        fn supports_linux_scope(&self, scope: linux::CaptureScope) -> bool {
            scope == linux::CaptureScope::Keyboard
        }
        fn supports_windows_scope(&self, scope: windows::CaptureScope) -> bool {
            scope == windows::CaptureScope::Keyboard
        }
        fn supports_windows_mode(&self, mode: keyweave_windows::runtime::ExecutionMode) -> bool {
            mode == keyweave_windows::runtime::ExecutionMode::Legacy
        }
        fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool;
    }
}
/// Sealed to engine and actual platform actors, not caller-supplied ack claims.
pub trait ReloadActor: sealed::Actor {}
impl sealed::Actor for Engine {
    fn generation(&self) -> ConfigGeneration {
        self.config_generation()
    }
    fn supports(&self, _: bool) -> bool {
        true
    }
    fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
        self.activate(snapshot).is_ok()
    }
}
impl ReloadActor for Engine {}
impl<S: keyweave_linux::capture::CaptureSource, B: keyweave_linux::pump::FrameBackend> sealed::Actor
    for keyweave_linux::runtime::Runtime<S, B>
{
    fn generation(&self) -> ConfigGeneration {
        self.config_generation()
    }
    fn supports(&self, linux: bool) -> bool {
        linux
    }
    fn supports_linux_mode(&self, mode: keyweave_linux::runtime::ExecutionMode) -> bool {
        mode == self.execution_mode()
    }
    fn supports_linux_scope(&self, scope: linux::CaptureScope) -> bool {
        match scope {
            linux::CaptureScope::Keyboard => !self.relative_enabled(),
            linux::CaptureScope::StagedRelative => self.relative_enabled(),
        }
    }
    fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
        self.activate(snapshot).is_ok()
    }
}
impl<S: keyweave_linux::capture::CaptureSource, B: keyweave_linux::pump::FrameBackend> ReloadActor
    for keyweave_linux::runtime::Runtime<S, B>
{
}
impl<B: keyweave_windows::pump::SendBackend> sealed::Actor
    for keyweave_windows::runtime::SessionActor<B>
{
    fn generation(&self) -> ConfigGeneration {
        self.config_generation()
    }
    fn supports(&self, linux: bool) -> bool {
        !linux
    }
    fn supports_windows_mode(&self, mode: keyweave_windows::runtime::ExecutionMode) -> bool {
        mode == self.execution_mode()
    }
    fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
        self.activate(snapshot).is_ok()
    }
}
impl<B: keyweave_windows::pump::SendBackend> ReloadActor
    for keyweave_windows::runtime::SessionActor<B>
{
}

impl<B: keyweave_windows::mixed::SendMixedBackend> sealed::Actor
    for keyweave_windows::mixed_actor::MixedSessionActor<B>
{
    fn generation(&self) -> ConfigGeneration {
        self.config_generation()
    }
    fn supports(&self, linux: bool) -> bool {
        !linux
    }
    fn supports_windows_mode(&self, mode: windows::ExecutionMode) -> bool {
        mode == windows::ExecutionMode::StagedKeyboard
    }
    fn supports_windows_scope(&self, scope: windows::CaptureScope) -> bool {
        scope == windows::CaptureScope::StagedMixed
    }
    fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
        self.activate(snapshot).is_ok()
    }
}
impl<B: keyweave_windows::mixed::SendMixedBackend> ReloadActor
    for keyweave_windows::mixed_actor::MixedSessionActor<B>
{
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn stop_keeps_same_running_job_until_reaped_and_never_reopens() {
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let job = std::thread::spawn(move || {
            wait.recv().unwrap();
            Completion {
                request: Request {
                    run_id: 1,
                    id: 1,
                    base: ConfigGeneration(1),
                    target: ConfigGeneration(2),
                    byte_length: 1,
                },
                candidate: None,
            }
        });
        let mut slot = PreparationSlot {
            job: Some(job),
            ready: None,
            closed: false,
        };
        assert_eq!(slot.poll().unwrap(), PreparationStep::Preparing);
        slot.close();
        assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
        assert!(slot.job.is_some());
        release.send(()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while slot.job.is_some() {
            assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(slot.ready.is_none());
    }
    #[test]
    fn panicked_job_closes_without_a_candidate() {
        let mut slot = PreparationSlot {
            job: Some(std::thread::spawn(|| panic!("test cold panic"))),
            ready: None,
            closed: false,
        };
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match slot.poll() {
                Err(PreparationError::Panicked) => break,
                Ok(PreparationStep::Preparing) => {}
                other => panic!("unexpected {other:?}"),
            }
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
    }
    const BODY:&[u8]=br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
    const POINT:&[u8]=br#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"pointing_button":"button1"},"to":[{"pointing_button":"button2"}]}]}]}}]}"#;
    struct ScopeActor {
        engine: Engine,
        scope: linux::CaptureScope,
    }
    impl sealed::Actor for ScopeActor {
        fn generation(&self) -> ConfigGeneration {
            self.engine.config_generation()
        }
        fn supports(&self, linux: bool) -> bool {
            linux
        }
        fn supports_linux_mode(&self, mode: linux::ExecutionMode) -> bool {
            mode == linux::ExecutionMode::StagedKeyboard
        }
        fn supports_linux_scope(&self, scope: linux::CaptureScope) -> bool {
            scope == self.scope
        }
        fn apply(&mut self, snapshot: Arc<ConfigSnapshot>) -> bool {
            self.engine.activate(snapshot).is_ok()
        }
    }
    impl ReloadActor for ScopeActor {}
    fn actor(scope: linux::CaptureScope) -> ScopeActor {
        ScopeActor {
            scope,
            engine: Engine::new(
                windows::PreparedConfig::from_bytes(BODY.to_vec())
                    .unwrap()
                    .snapshot()
                    .clone(),
            )
            .unwrap(),
        }
    }
    fn spec(buttons: &[u16]) -> keyweave_linux::output::DeviceSpec {
        linux::default_output_spec()
            .unwrap()
            .with_relative_axes(keyweave_linux::relative::RelativeAxes::new(&[0, 1]).unwrap())
            .with_pointing_buttons(keyweave_linux::keys::PointingButtons::new(buttons).unwrap())
            .unwrap()
    }
    fn receiver(body: &[u8]) -> Receiver<std::io::Cursor<Vec<u8>>> {
        use keyweave_progress::reload::{Worker, transport::ReceiveStep};
        let request = Request {
            run_id: 42,
            id: 1,
            base: ConfigGeneration(1),
            target: ConfigGeneration(2),
            byte_length: body.len() as u64,
        };
        let mut wire = request.encode().unwrap().to_vec();
        wire.extend_from_slice(body);
        let mut receiver = Receiver::new(
            std::io::Cursor::new(wire),
            Worker::new(42, ConfigGeneration(1)).unwrap(),
        );
        assert_eq!(receiver.poll().unwrap(), ReceiveStep::Pending);
        assert_eq!(receiver.poll().unwrap(), ReceiveStep::Ready);
        receiver
    }
    fn complete(slot: &mut PreparationSlot) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while slot.poll().unwrap() != PreparationStep::Ready {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    struct NoSend;
    impl keyweave_windows::pump::SendBackend for NoSend {
        fn submit(
            &mut self,
            _: &[keyweave_windows::keys::KeyboardPacket],
        ) -> Result<keyweave_windows::pump::Insertion, keyweave_windows::pump::SubmitError>
        {
            panic!("reload must not submit input")
        }
    }
    impl keyweave_windows::mixed::SendMixedBackend for NoSend {
        fn submit_mixed(
            &mut self,
            _: &[keyweave_windows::mixed::NativePacket],
        ) -> Result<keyweave_windows::pump::Insertion, keyweave_windows::pump::SubmitError>
        {
            panic!("reload must not submit input")
        }
    }
    fn mixed_actor() -> keyweave_windows::mixed_actor::MixedSessionActor<NoSend> {
        let snapshot = windows::PreparedConfig::from_bytes_for_generation_with_scope(
            BODY.to_vec(),
            ConfigGeneration(1),
            windows::ExecutionMode::StagedKeyboard,
            windows::CaptureScope::StagedMixed,
        )
        .unwrap()
        .snapshot()
        .clone();
        let mut actor = keyweave_windows::mixed_actor::MixedSessionActor::new(
            NoSend,
            keyweave_windows::keys::InjectionTag::new(1).unwrap(),
            snapshot,
        )
        .unwrap();
        actor.start_after_neutral().unwrap();
        actor
    }
    #[test]
    fn windows_mixed_candidate_applies_actual_mixed_actor_before_accepted_ack() {
        let mut actor = mixed_actor();
        let mut receiver = receiver(POINT);
        let mut slot = PreparationSlot::new();
        slot.start(receiver.take().unwrap(), Target::WindowsMixed)
            .unwrap();
        complete(&mut slot);
        let ack = slot.deliver(&mut receiver, &mut actor).unwrap();
        assert_eq!(ack.outcome, Outcome::Accepted);
        assert_eq!(ack.active, ConfigGeneration(2));
        assert_eq!(actor.config_generation(), ConfigGeneration(2));
    }
    #[test]
    fn windows_mixed_and_keyboard_candidates_cannot_cross_same_staged_mode() {
        let mut actor = mixed_actor();
        let mut receiver = receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(receiver.take().unwrap(), Target::WindowsStaged)
            .unwrap();
        complete(&mut slot);
        assert_eq!(
            slot.deliver(&mut receiver, &mut actor).unwrap().outcome,
            Outcome::Rejected
        );
        assert_eq!(actor.config_generation(), ConfigGeneration(1));
        let snapshot = windows::PreparedConfig::from_bytes(BODY.to_vec())
            .unwrap()
            .snapshot()
            .clone();
        let mut actor = keyweave_windows::runtime::SessionActor::new_staged(
            NoSend,
            keyweave_windows::keys::InjectionTag::new(1).unwrap(),
            snapshot,
        )
        .unwrap();
        actor.start_after_neutral().unwrap();
        let mut receiver = self::receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(receiver.take().unwrap(), Target::WindowsMixed)
            .unwrap();
        complete(&mut slot);
        assert_eq!(
            slot.deliver(&mut receiver, &mut actor).unwrap().outcome,
            Outcome::Rejected
        );
        assert_eq!(actor.config_generation(), ConfigGeneration(1));
    }
    #[test]
    fn windows_prepared_selects_frozen_scope_and_mode_for_reload() {
        for (mode, scope) in [
            (
                windows::ExecutionMode::Legacy,
                windows::CaptureScope::Keyboard,
            ),
            (
                windows::ExecutionMode::StagedKeyboard,
                windows::CaptureScope::Keyboard,
            ),
            (
                windows::ExecutionMode::StagedKeyboard,
                windows::CaptureScope::StagedMixed,
            ),
        ] {
            let prepared = windows::PreparedConfig::from_bytes_for_generation_with_scope(
                BODY.to_vec(),
                ConfigGeneration(1),
                mode,
                scope,
            )
            .unwrap();
            let target = Target::windows_prepared(&prepared);
            assert!(matches!(
                (target, mode, scope),
                (
                    Target::Windows,
                    windows::ExecutionMode::Legacy,
                    windows::CaptureScope::Keyboard
                ) | (
                    Target::WindowsStaged,
                    windows::ExecutionMode::StagedKeyboard,
                    windows::CaptureScope::Keyboard
                ) | (
                    Target::WindowsMixed,
                    windows::ExecutionMode::StagedKeyboard,
                    windows::CaptureScope::StagedMixed
                )
            ));
        }
    }
    #[test]
    fn relative_candidate_applies_actual_engine_generation_before_ack() {
        let mut actor = actor(linux::CaptureScope::StagedRelative);
        let mut receiver = receiver(POINT);
        let mut slot = PreparationSlot::new();
        slot.start(
            receiver.take().unwrap(),
            Target::LinuxRelative(spec(&[0x110, 0x111])),
        )
        .unwrap();
        complete(&mut slot);
        assert_eq!(actor.engine.config_generation(), ConfigGeneration(1));
        let ack = slot.deliver(&mut receiver, &mut actor).unwrap();
        assert_eq!(ack.outcome, Outcome::Accepted);
        assert_eq!(ack.active, ConfigGeneration(2));
        assert_eq!(actor.engine.config_generation(), ConfigGeneration(2));
    }
    #[test]
    fn relative_candidate_cannot_apply_to_keyboard_scope_with_same_mode() {
        let mut actor = actor(linux::CaptureScope::Keyboard);
        let mut receiver = receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(
            receiver.take().unwrap(),
            Target::LinuxRelative(spec(&[0x110])),
        )
        .unwrap();
        complete(&mut slot);
        let ack = slot.deliver(&mut receiver, &mut actor).unwrap();
        assert_eq!(ack.outcome, Outcome::Rejected);
        assert_eq!(actor.engine.config_generation(), ConfigGeneration(1));
    }
    #[test]
    fn keyboard_staged_candidate_cannot_apply_to_relative_scope() {
        let mut actor = actor(linux::CaptureScope::StagedRelative);
        let mut receiver = receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(
            receiver.take().unwrap(),
            Target::LinuxStaged(linux::default_output_spec().unwrap()),
        )
        .unwrap();
        complete(&mut slot);
        let ack = slot.deliver(&mut receiver, &mut actor).unwrap();
        assert_eq!(ack.outcome, Outcome::Rejected);
        assert_eq!(actor.engine.config_generation(), ConfigGeneration(1));
    }
    #[test]
    fn relative_cold_failure_never_changes_generation_or_acknowledges_applied() {
        let mut actor = actor(linux::CaptureScope::StagedRelative);
        let mut receiver = receiver(POINT);
        let mut slot = PreparationSlot::new();
        slot.start(
            receiver.take().unwrap(),
            Target::LinuxRelative(spec(&[0x110])),
        )
        .unwrap();
        complete(&mut slot);
        assert_eq!(
            slot.deliver(&mut receiver, &mut actor).unwrap().outcome,
            Outcome::Rejected
        );
        assert_eq!(actor.engine.config_generation(), ConfigGeneration(1));
    }
    #[test]
    fn prepared_scope_selects_fixed_reload_target() {
        let keyboard =
            linux::PreparedConfig::from_bytes(BODY.to_vec(), linux::default_output_spec().unwrap())
                .unwrap();
        assert!(matches!(
            Target::linux_prepared(&keyboard),
            Target::Linux(_)
        ));
        let staged = linux::PreparedConfig::read_with_mode(
            BODY,
            linux::default_output_spec().unwrap(),
            linux::ExecutionMode::StagedKeyboard,
        )
        .unwrap();
        assert!(matches!(
            Target::linux_prepared(&staged),
            Target::LinuxStaged(_)
        ));
        let relative = linux::PreparedConfig::from_bytes_for_generation_with_scope(
            POINT.to_vec(),
            spec(&[0x110, 0x111]),
            ConfigGeneration(1),
            linux::ExecutionMode::StagedKeyboard,
            linux::CaptureScope::StagedRelative,
        )
        .unwrap();
        assert!(matches!(
            Target::linux_prepared(&relative),
            Target::LinuxRelative(_)
        ));
    }
    #[test]
    fn closed_relative_lane_does_not_apply_late_candidate() {
        let mut actor = actor(linux::CaptureScope::StagedRelative);
        let mut receiver = receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(
            receiver.take().unwrap(),
            Target::LinuxRelative(spec(&[0x110])),
        )
        .unwrap();
        complete(&mut slot);
        slot.close();
        assert!(slot.deliver(&mut receiver, &mut actor).is_err());
        assert_eq!(actor.engine.config_generation(), ConfigGeneration(1));
        assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
    }
}
