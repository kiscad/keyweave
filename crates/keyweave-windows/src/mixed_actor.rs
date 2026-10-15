//! Explicit aggregate staged keyboard/mouse actor. Host owns native lifecycle gates.
use crate::{
    hook::HookEvent,
    keys::{self, InjectionTag},
    mixed::{MixedOutputPump, SendMixedBackend, TimedOutput},
    mouse_hook::{MouseHookEvent, MouseInputEvent},
    pointer::{self, MouseAction},
    pump::Phase,
    runtime::{ActorPhase, MAX_PENDING_INPUTS, MAX_RECOVERY, MAX_WORK, RuntimeError},
};
use keyweave_engine::{
    Condition, ConfigSnapshot,
    physical_controller::{PhysicalStageController, PhysicalStep},
    pointer_work::PointerMotion,
};
use keyweave_types::*;
use std::{collections::VecDeque, sync::Arc};
const STREAM: DeviceInstanceId = DeviceInstanceId {
    id: 1,
    generation: 1,
};
fn validate(config: &ConfigSnapshot) -> Result<(), RuntimeError> {
    for mapping in config.simple.iter().chain(&config.complex) {
        if mapping
            .extras
            .conditions
            .iter()
            .chain(mapping.actions().flat_map(|a| &a.conditions))
            .any(|c| matches!(c, Condition::Device { .. }))
        {
            return Err(RuntimeError::DeviceCondition);
        }
        if mapping.from.page == BUTTON_PAGE {
            pointer::encode(
                MouseAction::Button {
                    key: mapping.from,
                    edge: KeyEdge::Down,
                },
                InjectionTag::new(1)?,
            )
            .map_err(crate::mixed::MixedError::from)?;
        }
        for action in mapping.actions() {
            for key in action
                .key()
                .into_iter()
                .chain(action.modifiers.iter().copied())
            {
                if key.page == BUTTON_PAGE {
                    pointer::encode(
                        MouseAction::Button {
                            key,
                            edge: KeyEdge::Down,
                        },
                        InjectionTag::new(1)?,
                    )
                    .map_err(crate::mixed::MixedError::from)?;
                } else {
                    keys::output_scan(key)?;
                }
            }
        }
    }
    Ok(())
}
fn prepare_controller(
    config: Arc<ConfigSnapshot>,
) -> Result<PhysicalStageController, RuntimeError> {
    validate(&config)?;
    let mut controller = PhysicalStageController::new_with_chord_model(config)?;
    controller.observe_device(STREAM, false, MonoTime(0))?;
    Ok(controller)
}
/// Pure full mixed configuration admission, no backend or native resources.
pub fn preflight_mixed_config(config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
    prepare_controller(config).map(drop)
}
pub struct MixedSessionActor<B: SendMixedBackend> {
    controller: PhysicalStageController,
    output: MixedOutputPump<B>,
    phase: ActorPhase,
    pending: VecDeque<(MonoTime, InputEvent)>,
    now: MonoTime,
    seq: u64,
    failure: Option<RuntimeError>,
}
impl<B: SendMixedBackend> MixedSessionActor<B> {
    pub fn new(
        backend: B,
        tag: InjectionTag,
        config: Arc<ConfigSnapshot>,
    ) -> Result<Self, RuntimeError> {
        let controller = prepare_controller(config)?;
        Ok(Self {
            controller,
            output: MixedOutputPump::new(backend, tag),
            phase: ActorPhase::Standby,
            pending: VecDeque::new(),
            now: MonoTime(0),
            seq: 0,
            failure: None,
        })
    }
    pub fn phase(&self) -> ActorPhase {
        self.phase
    }
    pub fn output(&self) -> &MixedOutputPump<B> {
        &self.output
    }
    pub fn failure(&self) -> Option<&RuntimeError> {
        self.failure.as_ref()
    }
    pub fn queued_inputs(&self) -> usize {
        self.pending.len()
    }
    pub fn config_generation(&self) -> ConfigGeneration {
        self.controller.snapshot().generation
    }
    pub fn next_wakeup(&self) -> Option<MonoTime> {
        if self.phase != ActorPhase::Active {
            return None;
        }
        [
            self.output.next_deadline(),
            self.controller.next_deadline(),
            self.pending.front().map(|v| v.0),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    pub fn start_after_neutral(&mut self) -> Result<(), RuntimeError> {
        if self.phase != ActorPhase::Standby {
            return Err(RuntimeError::NotActive);
        }
        self.output.resume()?;
        self.phase = ActorPhase::Active;
        Ok(())
    }
    fn checked<T>(&mut self, result: Result<T, RuntimeError>) -> Result<T, RuntimeError> {
        if result.is_err() && self.phase == ActorPhase::Active {
            self.phase = ActorPhase::StopRequired;
        }
        result
    }
    pub fn control_failed(&mut self, error: RuntimeError) {
        if self.phase == ActorPhase::Active {
            self.phase = ActorPhase::StopRequired;
        }
        self.failure.get_or_insert(error);
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
    fn append(&mut self, events: Vec<InputEvent>) -> Result<(), RuntimeError> {
        if self.pending.len() + events.len() > MAX_PENDING_INPUTS {
            return Err(RuntimeError::Budget);
        }
        self.pending
            .extend(events.into_iter().map(|event| (self.now, event)));
        Ok(())
    }
    fn accept_step(&mut self, step: PhysicalStep) -> Result<(), RuntimeError> {
        let mut output = step
            .outputs
            .into_iter()
            .map(TimedOutput::Key)
            .collect::<Vec<_>>();
        for pointer in step.forwarded_pointers {
            let at = pointer.delivered_at;
            match pointer.event.payload {
                PointerMotion::Wheel {
                    horizontal,
                    vertical,
                } => {
                    if horizontal != 0 {
                        output.push(TimedOutput::Mouse {
                            at,
                            action: MouseAction::Wheel {
                                horizontal: true,
                                delta: horizontal,
                            },
                        });
                    }
                    if vertical != 0 {
                        output.push(TimedOutput::Mouse {
                            at,
                            action: MouseAction::Wheel {
                                horizontal: false,
                                delta: vertical,
                            },
                        });
                    }
                }
                PointerMotion::Activity { .. } => {}
                PointerMotion::Move { .. } => return Err(RuntimeError::StagedPointer),
            }
        }
        self.output.enqueue(&output)?;
        Ok(())
    }
    fn due(&mut self) -> Result<(), RuntimeError> {
        for _ in 0..MAX_WORK {
            let cutoff = self.pending.front().map_or(self.now, |v| v.0.min(self.now));
            if self.output.next_deadline().is_some_and(|at| at <= self.now) {
                self.output.poll(self.now)?;
                if self.output.phase() != Phase::Normal {
                    return Err(RuntimeError::Incomplete);
                }
            } else if self
                .controller
                .next_deadline()
                .is_some_and(|at| at <= cutoff)
            {
                let step = self
                    .controller
                    .poll(cutoff)?
                    .ok_or(RuntimeError::Decision)?;
                self.accept_step(step)?;
            } else if self.pending.front().is_some_and(|v| v.0 <= self.now) {
                let (at, event) = self.pending.pop_front().ok_or(RuntimeError::Decision)?;
                let repeat = match event {
                    InputEvent::Key {
                        key,
                        edge: KeyEdge::Repeat,
                    } => Some(key),
                    _ => None,
                };
                let is_key = matches!(event, InputEvent::Key { .. });
                let seq = self.seq.checked_add(1).ok_or(RuntimeError::Sequence)?;
                let step = self.controller.handle(InputEnvelope {
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
                self.accept_step(step)?;
                if let Some(source) = repeat
                    && let Some(key) = self.controller.physical_repeat_target(STREAM, source)
                    && key.page != BUTTON_PAGE
                    && self.output.ledger().desired().contains(&key)
                    && self.output.ledger().submitted().contains(&key)
                {
                    self.output.enqueue(&[TimedOutput::Key(OutputIntent {
                        at_ns: self.now,
                        key,
                        edge: KeyEdge::Repeat,
                    })])?;
                }
            } else {
                return Ok(());
            }
        }
        Ok(())
    }
    pub fn tick(&mut self, now: MonoTime) -> Result<(), RuntimeError> {
        let result = self.advance(now).and_then(|()| self.due());
        self.checked(result)
    }
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
            self.append(vec![InputEvent::Key {
                key: event.identity.key,
                edge: event.edge,
            }])?;
            self.due()?;
            Ok(CaptureDecision::Capture)
        })();
        self.checked(result)
    }
    pub fn mouse(
        &mut self,
        event: MouseHookEvent,
        now: MonoTime,
    ) -> Result<CaptureDecision, RuntimeError> {
        let result = (|| {
            self.advance(now)?;
            if event.binding == Some(CaptureDecision::Pass)
                || event
                    .pair_bindings
                    .is_some_and(|v| v.contains(&Some(CaptureDecision::Pass)))
            {
                return Err(RuntimeError::Decision);
            }
            let (events, decision) = match event.input {
                MouseInputEvent::Position { .. } => (
                    vec![InputEvent::PointerActivity {
                        moved: true,
                        scrolled: false,
                    }],
                    CaptureDecision::Pass,
                ),
                MouseInputEvent::Wheel { horizontal, delta } => (
                    vec![InputEvent::Wheel {
                        horizontal: if horizontal { delta } else { 0 },
                        vertical: if horizontal { 0 } else { delta },
                    }],
                    CaptureDecision::Capture,
                ),
                MouseInputEvent::Button { key, edge } => {
                    pointer::encode(
                        MouseAction::Button {
                            key,
                            edge: if edge == KeyEdge::Repeat {
                                KeyEdge::Down
                            } else {
                                edge
                            },
                        },
                        InjectionTag::new(1)?,
                    )
                    .map_err(crate::mixed::MixedError::from)?;
                    (
                        vec![InputEvent::Key { key, edge }],
                        CaptureDecision::Capture,
                    )
                }
                MouseInputEvent::ButtonPair { edges } => (
                    edges
                        .into_iter()
                        .enumerate()
                        .map(|(i, edge)| InputEvent::Key {
                            key: KeyId::new(BUTTON_PAGE, 4 + i as u16),
                            edge,
                        })
                        .collect(),
                    CaptureDecision::Capture,
                ),
            };
            self.append(events)?;
            self.due()?;
            Ok(decision)
        })();
        self.checked(result)
    }
    pub fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
        if self.phase != ActorPhase::Active {
            return Err(RuntimeError::NotActive);
        }
        if !self.pending.is_empty() {
            return Err(RuntimeError::StagedInputPending);
        }
        validate(&config)?;
        self.controller.activate(config)?;
        Ok(())
    }
    pub fn context_changed(
        &mut self,
        context: ContextSnapshot,
        now: MonoTime,
    ) -> Result<(), RuntimeError> {
        let result = (|| {
            self.advance(now)?;
            let revision = self
                .pending
                .iter()
                .filter_map(|(_, event)| match event {
                    InputEvent::ContextChanged { context } => Some(context.revision),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            self.controller
                .validate_context_candidate(&context, revision, now)?;
            self.append(vec![InputEvent::ContextChanged { context }])?;
            self.due()
        })();
        self.checked(result)
    }
    /// Both native hooks must be disabled/unhooked before recovery.
    pub fn stop_after_unhook(&mut self) -> Result<(), RuntimeError> {
        if matches!(self.phase, ActorPhase::Stopped | ActorPhase::Unresolved) {
            return Err(RuntimeError::NotActive);
        }
        self.phase = ActorPhase::StopRequired;
        self.pending.clear();
        let reset = self.controller.reset(self.now);
        if self.output.begin_recovery(self.now).is_err() {
            self.phase = ActorPhase::Unresolved;
            return Err(RuntimeError::Unresolved);
        }
        for _ in 0..MAX_RECOVERY {
            if self.output.phase() == Phase::Paused {
                break;
            }
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
        reset?;
        Ok(())
    }
    pub fn stop_without_delivery_after_unhook(
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
        self.pending.clear();
        let _ = self.controller.reset(self.now);
        self.output.quarantine_without_delivery();
        self.phase = ActorPhase::Unresolved;
        Err(RuntimeError::Unresolved)
    }
}

/// Two wrappers may share one Rc actor for keyboard/mouse registrations.
/// Clocks must be bounded and use the same monotonic domain; native lifecycle
/// code must still enforce independent session/lease/emergency gates.
pub struct MixedActorHandler<B: SendMixedBackend, C: FnMut() -> Option<MonoTime>> {
    pub actor: std::rc::Rc<std::cell::RefCell<MixedSessionActor<B>>>,
    pub clock: C,
}
impl<B: SendMixedBackend, C: FnMut() -> Option<MonoTime>> crate::hook::HookHandler
    for MixedActorHandler<B, C>
{
    fn handle(&mut self, event: HookEvent) -> Result<CaptureDecision, crate::hook::HookFault> {
        let now = (self.clock)();
        let mut actor = self
            .actor
            .try_borrow_mut()
            .map_err(|_| crate::hook::HookFault::Reentrant)?;
        let Some(now) = now else {
            actor.control_failed(RuntimeError::Clock);
            return Err(crate::hook::HookFault::Handler);
        };
        match actor.key(event, now) {
            Ok(decision) => Ok(decision),
            Err(error) => {
                actor.control_failed(error);
                Err(crate::hook::HookFault::Handler)
            }
        }
    }
}
impl<B: SendMixedBackend, C: FnMut() -> Option<MonoTime>> crate::mouse_hook::MouseHandler
    for MixedActorHandler<B, C>
{
    fn handle_mouse(
        &mut self,
        event: MouseHookEvent,
    ) -> Result<CaptureDecision, crate::mouse_hook::MouseFault> {
        let now = (self.clock)();
        let mut actor = self
            .actor
            .try_borrow_mut()
            .map_err(|_| crate::mouse_hook::MouseFault::Handler)?;
        let Some(now) = now else {
            actor.control_failed(RuntimeError::Clock);
            return Err(crate::mouse_hook::MouseFault::Handler);
        };
        match actor.mouse(event, now) {
            Ok(decision) => Ok(decision),
            Err(error) => {
                actor.control_failed(error);
                Err(crate::mouse_hook::MouseFault::Handler)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        keys::ResolvedKey,
        mixed::NativePacket,
        pump::{Insertion, SubmitError},
    };
    use keyweave_engine::{Mapping, MappingExtras};
    use std::{cell::RefCell, rc::Rc};
    type Log = Rc<RefCell<Vec<Vec<NativePacket>>>>;
    struct Backend {
        log: Log,
        replies: VecDeque<Result<Insertion, SubmitError>>,
    }
    impl SendMixedBackend for Backend {
        fn submit_mixed(&mut self, p: &[NativePacket]) -> Result<Insertion, SubmitError> {
            self.log.borrow_mut().push(p.to_vec());
            self.replies.pop_front().unwrap_or(Ok(Insertion {
                accepted: p.len(),
                last_error: None,
            }))
        }
    }
    fn key(n: u16) -> KeyId {
        KeyId::new(KEYBOARD_PAGE, n)
    }
    fn button(n: u16) -> KeyId {
        KeyId::new(BUTTON_PAGE, n)
    }
    fn config(generation: u64, maps: Vec<(KeyId, KeyId)>) -> Arc<ConfigSnapshot> {
        Arc::new(ConfigSnapshot {
            generation: ConfigGeneration(generation),
            name: "mixed".into(),
            simple: maps
                .into_iter()
                .map(|(from, to)| Mapping {
                    id: "map".into(),
                    from,
                    to: vec![to.into()],
                    allow_extra_modifiers: true,
                    extras: MappingExtras::default(),
                })
                .collect(),
            complex: vec![],
        })
    }
    fn actor(
        config: Arc<ConfigSnapshot>,
        replies: Vec<Result<Insertion, SubmitError>>,
    ) -> (MixedSessionActor<Backend>, Log) {
        let log = Rc::new(RefCell::new(vec![]));
        let mut actor = MixedSessionActor::new(
            Backend {
                log: log.clone(),
                replies: replies.into(),
            },
            InjectionTag::new(7).unwrap(),
            config,
        )
        .unwrap();
        actor.start_after_neutral().unwrap();
        (actor, log)
    }
    fn event(input: MouseInputEvent, binding: Option<CaptureDecision>) -> MouseHookEvent {
        MouseHookEvent {
            input,
            binding,
            pair_bindings: None,
        }
    }
    fn ke(edge: KeyEdge) -> HookEvent {
        HookEvent {
            identity: ResolvedKey {
                key: key(4),
                known_aliases: false,
            },
            edge,
            binding: if edge == KeyEdge::Down {
                None
            } else {
                Some(CaptureDecision::Capture)
            },
        }
    }
    fn drain(actor: &mut MixedSessionActor<Backend>, at: u64) {
        for _ in 0..32 {
            if !actor.next_wakeup().is_some_and(|due| due <= MonoTime(at)) {
                return;
            }
            actor.tick(MonoTime(at)).unwrap();
        }
        panic!("bounded fixture did not drain");
    }
    #[test]
    fn keyboard_to_button_and_button_to_keyboard_use_same_accepted_ledger() {
        let (mut a, log) = actor(
            config(1, vec![(key(4), button(1)), (button(2), key(5))]),
            vec![],
        );
        a.key(ke(KeyEdge::Down), MonoTime(0)).unwrap();
        drain(&mut a, 0);
        assert!(a.output().ledger().submitted().contains(&button(1)));
        a.mouse(
            event(
                MouseInputEvent::Button {
                    key: button(2),
                    edge: KeyEdge::Down,
                },
                None,
            ),
            MonoTime(1),
        )
        .unwrap();
        drain(&mut a, 1);
        assert!(a.output().ledger().submitted().contains(&key(5)));
        a.stop_after_unhook().unwrap();
        assert_eq!(a.phase(), ActorPhase::Stopped);
        assert!(a.output().ledger().submitted().is_empty());
        let calls = log.borrow();
        assert!(
            calls
                .iter()
                .flatten()
                .any(|p| matches!(p,NativePacket::Mouse(packet) if packet.flags()==4))
        );
        assert!(
            calls
                .iter()
                .flatten()
                .any(|p| matches!(p,NativePacket::Keyboard(packet) if packet.flags&2!=0))
        );
    }
    #[test]
    fn wheel_receipt_preserves_delta_and_position_never_injects_motion() {
        let (mut a, log) = actor(config(1, vec![]), vec![]);
        assert_eq!(
            a.mouse(
                event(MouseInputEvent::Position { x: -300, y: 200 }, None),
                MonoTime(0)
            )
            .unwrap(),
            CaptureDecision::Pass
        );
        drain(&mut a, 0);
        assert!(log.borrow().is_empty());
        for (horizontal, delta) in [(true, -1), (false, -120)] {
            assert_eq!(
                a.mouse(
                    event(MouseInputEvent::Wheel { horizontal, delta }, None),
                    MonoTime(1)
                )
                .unwrap(),
                CaptureDecision::Capture
            );
            drain(&mut a, 1);
        }
        let packets = log.borrow().iter().flatten().copied().collect::<Vec<_>>();
        assert_eq!(packets.len(), 2);
        assert!(
            matches!(packets[0],NativePacket::Mouse(p) if p.flags()==0x1000&&p.data()==u32::MAX)
        );
        assert!(
            matches!(packets[1],NativePacket::Mouse(p) if p.flags()==0x800&&p.data()==u32::from_ne_bytes((-120i32).to_ne_bytes()))
        );
    }
    #[test]
    fn held_button_keeps_old_mapping_across_activation() {
        let (mut a, _) = actor(config(1, vec![(button(1), button(2))]), vec![]);
        a.mouse(
            event(
                MouseInputEvent::Button {
                    key: button(1),
                    edge: KeyEdge::Down,
                },
                None,
            ),
            MonoTime(0),
        )
        .unwrap();
        drain(&mut a, 0);
        a.activate(config(2, vec![(button(1), button(3))])).unwrap();
        assert_eq!(a.config_generation(), ConfigGeneration(2));
        a.mouse(
            event(
                MouseInputEvent::Button {
                    key: button(1),
                    edge: KeyEdge::Up,
                },
                Some(CaptureDecision::Capture),
            ),
            MonoTime(1),
        )
        .unwrap();
        drain(&mut a, 1);
        assert!(a.output().ledger().submitted().is_empty());
        a.mouse(
            event(
                MouseInputEvent::Button {
                    key: button(1),
                    edge: KeyEdge::Down,
                },
                None,
            ),
            MonoTime(2),
        )
        .unwrap();
        drain(&mut a, 2);
        assert!(a.output().ledger().submitted().contains(&button(3)));
    }
    #[test]
    fn combined_buttons_enter_together_and_repeated_buttons_do_not_repeat_send() {
        let (mut a, log) = actor(config(1, vec![]), vec![]);
        a.mouse(
            event(
                MouseInputEvent::ButtonPair {
                    edges: [KeyEdge::Down; 2],
                },
                None,
            ),
            MonoTime(0),
        )
        .unwrap();
        drain(&mut a, 0);
        assert_eq!(a.output().ledger().submitted().len(), 2);
        let calls = log.borrow().len();
        a.mouse(
            MouseHookEvent {
                input: MouseInputEvent::ButtonPair {
                    edges: [KeyEdge::Repeat; 2],
                },
                binding: Some(CaptureDecision::Capture),
                pair_bindings: Some([Some(CaptureDecision::Capture); 2]),
            },
            MonoTime(1),
        )
        .unwrap();
        drain(&mut a, 1);
        assert_eq!(log.borrow().len(), calls);
        a.mouse(
            event(
                MouseInputEvent::ButtonPair {
                    edges: [KeyEdge::Up; 2],
                },
                Some(CaptureDecision::Capture),
            ),
            MonoTime(2),
        )
        .unwrap();
        drain(&mut a, 2);
        assert!(a.output().ledger().submitted().is_empty());
    }
    #[test]
    fn unknown_acceptance_and_access_loss_do_not_emit_recovery_ups() {
        let (mut a, log) = actor(config(1, vec![]), vec![Err(SubmitError::Unknown)]);
        assert!(
            a.mouse(
                event(
                    MouseInputEvent::Button {
                        key: button(1),
                        edge: KeyEdge::Down
                    },
                    None
                ),
                MonoTime(0)
            )
            .is_err()
        );
        assert_eq!(a.phase(), ActorPhase::StopRequired);
        assert!(a.stop_after_unhook().is_err());
        assert_eq!(a.phase(), ActorPhase::Unresolved);
        assert_eq!(log.borrow().len(), 1);
        let (mut a, log) = actor(config(1, vec![]), vec![]);
        a.mouse(
            event(
                MouseInputEvent::Button {
                    key: button(1),
                    edge: KeyEdge::Down,
                },
                None,
            ),
            MonoTime(0),
        )
        .unwrap();
        drain(&mut a, 0);
        assert!(
            a.stop_without_delivery_after_unhook(RuntimeError::AccessLost)
                .is_err()
        );
        assert_eq!(log.borrow().len(), 1);
        assert_eq!(a.output().ledger().submitted().len(), 1);
    }
    #[test]
    fn keyboard_mouse_dispatchers_share_actor_and_clock_failure_preserves_cause() {
        use crate::{
            hook::HookDispatcher,
            keys::{HookRecord, IdentityPolicy},
            mouse_hook::{MouseDispatcher, MouseHookRecord},
        };
        let (a, log) = actor(
            config(1, vec![(key(4), button(1)), (button(2), key(5))]),
            vec![],
        );
        let actor = Rc::new(RefCell::new(a));
        let mut keyboard = HookDispatcher::new(
            MixedActorHandler {
                actor: actor.clone(),
                clock: || Some(MonoTime(0)),
            },
            InjectionTag::new(7).unwrap(),
            IdentityPolicy::CanonicalScan,
        );
        let mut mouse = MouseDispatcher::new(
            MixedActorHandler {
                actor: actor.clone(),
                clock: || Some(MonoTime(0)),
            },
            InjectionTag::new(7).unwrap(),
        );
        assert_eq!(
            keyboard.dispatch(
                0,
                Some(HookRecord {
                    message: 0x100,
                    vk_code: 0x41,
                    scan_code: 0x1e,
                    flags: 0,
                    extra_info: 0
                })
            ),
            CaptureDecision::Capture
        );
        assert_eq!(
            mouse.dispatch(
                0,
                Some(MouseHookRecord {
                    message: 0x204,
                    x: 0,
                    y: 0,
                    data: 0,
                    flags: 0,
                    extra_info: 0
                })
            ),
            CaptureDecision::Capture
        );
        drain(&mut actor.borrow_mut(), 0);
        assert_eq!(actor.borrow().output().ledger().submitted().len(), 2);
        assert!(!log.borrow().is_empty());
        let mut failed = MouseDispatcher::new(
            MixedActorHandler {
                actor: actor.clone(),
                clock: || None,
            },
            InjectionTag::new(7).unwrap(),
        );
        assert_eq!(
            failed.dispatch(
                0,
                Some(MouseHookRecord {
                    message: 0x200,
                    x: 1,
                    y: 1,
                    data: 0,
                    flags: 0,
                    extra_info: 0
                })
            ),
            CaptureDecision::Pass
        );
        assert!(matches!(
            actor.borrow().failure(),
            Some(RuntimeError::Clock)
        ));
        assert_eq!(actor.borrow().phase(), ActorPhase::StopRequired);
        actor.borrow_mut().stop_after_unhook().unwrap();
    }
    #[test]
    fn cold_unsupported_button_and_full_pair_append_reject_before_mutation() {
        let log = Rc::new(RefCell::new(vec![]));
        assert!(
            MixedSessionActor::new(
                Backend {
                    log: log.clone(),
                    replies: VecDeque::new()
                },
                InjectionTag::new(7).unwrap(),
                config(1, vec![(key(4), button(6))])
            )
            .is_err()
        );
        assert!(log.borrow().is_empty());
        let (mut a, log) = actor(config(1, vec![]), vec![]);
        a.pending.extend((0..MAX_PENDING_INPUTS - 1).map(|_| {
            (
                MonoTime(0),
                InputEvent::PointerActivity {
                    moved: true,
                    scrolled: false,
                },
            )
        }));
        assert!(
            a.mouse(
                event(
                    MouseInputEvent::ButtonPair {
                        edges: [KeyEdge::Down; 2]
                    },
                    None
                ),
                MonoTime(0)
            )
            .is_err()
        );
        assert_eq!(a.queued_inputs(), MAX_PENDING_INPUTS - 1);
        assert!(a.output().ledger().desired().is_empty());
        assert!(log.borrow().is_empty());
    }
}
