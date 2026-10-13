//! Owned cold threads and pure engine; no devices, files or native input.
#![allow(clippy::unwrap_used)]
use keyweave_engine::Engine;
use keyweave_progress::reload::{
    Outcome, ReloadError, Request, Worker,
    transport::{AckWriter, ReceiveStep, Received, Receiver},
};
use keyweave_types::ConfigGeneration;
use keyweave_worker::{
    linux_preparation as linux, preparation as windows,
    reload_preparation::{PreparationError, PreparationSlot, PreparationStep, Target},
};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};
const BODY: &[u8] = br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
fn request(body: &[u8]) -> Request {
    Request {
        run_id: 42,
        id: 1,
        base: ConfigGeneration(1),
        target: ConfigGeneration(2),
        byte_length: body.len() as u64,
    }
}
fn receiver(body: &[u8]) -> Receiver<Cursor<Vec<u8>>> {
    let mut wire = request(body).encode().unwrap().to_vec();
    wire.extend_from_slice(body);
    let mut rx = Receiver::new(
        Cursor::new(wire),
        Worker::new(42, ConfigGeneration(1)).unwrap(),
    );
    assert_eq!(rx.poll().unwrap(), ReceiveStep::Pending);
    assert_eq!(rx.poll().unwrap(), ReceiveStep::Ready);
    rx
}
fn engine() -> Engine {
    Engine::new(
        windows::PreparedConfig::from_bytes(BODY.to_vec())
            .unwrap()
            .snapshot()
            .clone(),
    )
    .unwrap()
}
fn wait(slot: &mut PreparationSlot) -> PreparationStep {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let step = slot.poll().unwrap();
        if step != PreparationStep::Preparing {
            return step;
        }
        assert!(
            Instant::now() < until,
            "same job observation budget expired"
        );
        std::thread::yield_now();
    }
}
#[test]
fn both_cold_factories_deliver_to_actual_engine_before_ack() {
    for target in [
        Target::Linux(linux::default_output_spec().unwrap()),
        Target::Windows,
    ] {
        let mut actor = engine();
        let mut rx = receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(rx.take().unwrap(), target).unwrap();
        assert_eq!(wait(&mut slot), PreparationStep::Ready);
        assert_eq!(actor.config_generation(), ConfigGeneration(1));
        assert_eq!(rx.poll().unwrap(), ReceiveStep::AwaitingCommit);
        let ack = slot.deliver(&mut rx, &mut actor).unwrap();
        assert_eq!(ack.outcome, Outcome::Accepted);
        assert_eq!(actor.config_generation(), ConfigGeneration(2));
        assert_eq!(rx.active(), actor.config_generation());
        assert_eq!(rx.poll().unwrap(), ReceiveStep::AwaitingAck);
        let mut writer = AckWriter::new(Vec::new(), ack).unwrap();
        rx.ack_sent(writer.poll().unwrap().unwrap()).unwrap();
        assert_eq!(slot.poll().unwrap(), PreparationStep::Idle);
    }
}
#[test]
fn compile_failure_rejects_without_changing_actual_actor() {
    let mut rx = receiver(b"private invalid config");
    let mut actor = engine();
    let mut slot = PreparationSlot::new();
    slot.start(rx.take().unwrap(), Target::Windows).unwrap();
    wait(&mut slot);
    let ack = slot.deliver(&mut rx, &mut actor).unwrap();
    assert_eq!(ack.outcome, Outcome::Rejected);
    assert_eq!(ack.active, ConfigGeneration(1));
    assert_eq!(actor.config_generation(), ConfigGeneration(1));
    assert_eq!(rx.poll().unwrap(), ReceiveStep::AwaitingAck);
}
#[test]
fn ready_job_keeps_slot_busy_and_delivers_once() {
    let mut rx = receiver(BODY);
    let mut actor = engine();
    let mut slot = PreparationSlot::new();
    slot.start(rx.take().unwrap(), Target::Windows).unwrap();
    wait(&mut slot);
    assert!(matches!(
        slot.start(
            Received {
                request: request(BODY),
                body: BODY.to_vec()
            },
            Target::Windows
        ),
        Err(PreparationError::Protocol(ReloadError::Busy))
    ));
    assert_eq!(
        slot.deliver(&mut rx, &mut actor).unwrap().outcome,
        Outcome::Accepted
    );
    assert!(matches!(
        slot.deliver(&mut rx, &mut actor),
        Err(PreparationError::Protocol(ReloadError::NoPending))
    ));
    assert_eq!(actor.config_generation(), ConfigGeneration(2));
}
#[test]
fn closed_or_wrong_receiver_prevents_mutation_before_finish() {
    for close in [false, true] {
        let mut original = receiver(BODY);
        let mut actor = engine();
        let mut slot = PreparationSlot::new();
        slot.start(original.take().unwrap(), Target::Windows)
            .unwrap();
        wait(&mut slot);
        let mut other = Receiver::new(
            Cursor::new(Vec::new()),
            Worker::new(43, ConfigGeneration(1)).unwrap(),
        );
        let rx = if close {
            original.close();
            &mut original
        } else {
            &mut other
        };
        assert!(slot.deliver(rx, &mut actor).is_err());
        assert_eq!(actor.config_generation(), ConfigGeneration(1));
        assert_eq!(slot.poll().unwrap(), PreparationStep::Closed);
    }
}
#[test]
fn stale_actual_generation_closes_lane_without_rollback() {
    let mut rx = receiver(BODY);
    let mut actor = engine();
    let mut slot = PreparationSlot::new();
    slot.start(rx.take().unwrap(), Target::Windows).unwrap();
    wait(&mut slot);
    actor
        .activate(
            windows::PreparedConfig::from_bytes_for_generation(BODY.to_vec(), ConfigGeneration(3))
                .unwrap()
                .snapshot()
                .clone(),
        )
        .unwrap();
    assert!(slot.deliver(&mut rx, &mut actor).is_err());
    assert_eq!(actor.config_generation(), ConfigGeneration(3));
    assert!(rx.poll().is_err());
}
#[test]
fn stop_discards_ready_success_and_invalid_body_never_spawns() {
    let mut rx = receiver(BODY);
    let mut actor = engine();
    let mut slot = PreparationSlot::new();
    slot.start(rx.take().unwrap(), Target::Windows).unwrap();
    wait(&mut slot);
    slot.close();
    assert!(slot.deliver(&mut rx, &mut actor).is_err());
    assert_eq!(actor.config_generation(), ConfigGeneration(1));
    assert!(
        slot.start(
            Received {
                request: request(BODY),
                body: BODY.to_vec()
            },
            Target::Windows
        )
        .is_err()
    );
    let mut invalid = PreparationSlot::new();
    assert!(matches!(
        invalid.start(
            Received {
                request: request(BODY),
                body: vec![0]
            },
            Target::Windows
        ),
        Err(PreparationError::Protocol(ReloadError::Length))
    ));
    assert_eq!(invalid.poll().unwrap(), PreparationStep::Closed);
}

struct NoOutput;
impl keyweave_windows::pump::SendBackend for NoOutput {
    fn submit(
        &mut self,
        _: &[keyweave_windows::keys::KeyboardPacket],
    ) -> Result<keyweave_windows::pump::Insertion, keyweave_windows::pump::SubmitError> {
        panic!("configuration activation must not invoke output");
    }
}
#[test]
fn actual_windows_actor_admission_and_platform_mismatch_never_submit_output() {
    for (target, stopped, expected) in [
        (Target::Windows, false, Outcome::Accepted),
        (Target::Windows, true, Outcome::Rejected),
        (
            Target::Linux(linux::default_output_spec().unwrap()),
            false,
            Outcome::Rejected,
        ),
    ] {
        let mut actor = keyweave_windows::runtime::SessionActor::new(
            NoOutput,
            keyweave_windows::keys::InjectionTag::new(1).unwrap(),
            windows::PreparedConfig::from_bytes(BODY.to_vec())
                .unwrap()
                .snapshot()
                .clone(),
        )
        .unwrap();
        if stopped {
            actor.stop_after_unhook().unwrap();
        }
        let mut rx = receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(rx.take().unwrap(), target).unwrap();
        wait(&mut slot);
        let ack = slot.deliver(&mut rx, &mut actor).unwrap();
        assert_eq!(ack.outcome, expected);
        assert_eq!(
            actor.config_generation(),
            if expected == Outcome::Accepted {
                ConfigGeneration(2)
            } else {
                ConfigGeneration(1)
            }
        );
        assert_eq!(ack.active, actor.config_generation());
    }
}
#[test]
fn staged_linux_candidate_cannot_apply_to_legacy_engine_even_when_ir_is_legacy_compatible() {
    let mut actor = engine();
    let mut rx = receiver(BODY);
    let mut slot = PreparationSlot::new();
    slot.start(
        rx.take().unwrap(),
        Target::LinuxStaged(linux::default_output_spec().unwrap()),
    )
    .unwrap();
    assert_eq!(wait(&mut slot), PreparationStep::Ready);
    let ack = slot.deliver(&mut rx, &mut actor).unwrap();
    assert_eq!(ack.outcome, Outcome::Rejected);
    assert_eq!(actor.config_generation(), ConfigGeneration(1));
    assert_eq!(rx.active(), ConfigGeneration(1));
}
#[test]
fn windows_staged_candidate_applies_only_to_same_fixed_session_mode() {
    for staged in [false, true] {
        let snapshot = windows::PreparedConfig::from_bytes(BODY.to_vec())
            .unwrap()
            .snapshot()
            .clone();
        let tag = keyweave_windows::keys::InjectionTag::new(1).unwrap();
        let mut actor = if staged {
            keyweave_windows::runtime::SessionActor::new_staged(NoOutput, tag, snapshot).unwrap()
        } else {
            keyweave_windows::runtime::SessionActor::new(NoOutput, tag, snapshot).unwrap()
        };
        let mut rx = receiver(BODY);
        let mut slot = PreparationSlot::new();
        slot.start(rx.take().unwrap(), Target::WindowsStaged)
            .unwrap();
        assert_eq!(wait(&mut slot), PreparationStep::Ready);
        let ack = slot.deliver(&mut rx, &mut actor).unwrap();
        assert_eq!(
            ack.outcome,
            if staged {
                Outcome::Accepted
            } else {
                Outcome::Rejected
            }
        );
        assert_eq!(
            actor.config_generation(),
            ConfigGeneration(if staged { 2 } else { 1 })
        );
        assert_eq!(
            actor.execution_mode(),
            if staged {
                keyweave_windows::runtime::ExecutionMode::StagedKeyboard
            } else {
                keyweave_windows::runtime::ExecutionMode::Legacy
            }
        );
    }
}
#[test]
fn windows_legacy_candidate_cannot_switch_staged_actor_mode() {
    let snapshot = windows::PreparedConfig::from_bytes(BODY.to_vec())
        .unwrap()
        .snapshot()
        .clone();
    let mut actor = keyweave_windows::runtime::SessionActor::new_staged(
        NoOutput,
        keyweave_windows::keys::InjectionTag::new(1).unwrap(),
        snapshot,
    )
    .unwrap();
    let mut rx = receiver(BODY);
    let mut slot = PreparationSlot::new();
    slot.start(rx.take().unwrap(), Target::Windows).unwrap();
    assert_eq!(wait(&mut slot), PreparationStep::Ready);
    assert_eq!(
        slot.deliver(&mut rx, &mut actor).unwrap().outcome,
        Outcome::Rejected
    );
    assert_eq!(actor.config_generation(), ConfigGeneration(1));
}

const CHORD: &[u8] = br#"{"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[{"type":"basic","from":{"simultaneous":[{"key_code":"a"},{"key_code":"b"}],"simultaneous_options":{"to_after_key_up":[{"key_code":"d"}]}},"to":[{"key_code":"c"}]}]}]}}]}"#;
struct ChordOutput(std::rc::Rc<std::cell::RefCell<Vec<(u16, u32)>>>);
impl keyweave_windows::pump::SendBackend for ChordOutput {
    fn submit(
        &mut self,
        packets: &[keyweave_windows::keys::KeyboardPacket],
    ) -> Result<keyweave_windows::pump::Insertion, keyweave_windows::pump::SubmitError> {
        self.0
            .borrow_mut()
            .extend(packets.iter().map(|p| (p.scan, p.flags)));
        Ok(keyweave_windows::pump::Insertion {
            accepted: packets.len(),
            last_error: None,
        })
    }
}
fn chord_event(usage: u16, edge: keyweave_types::KeyEdge) -> keyweave_windows::hook::HookEvent {
    keyweave_windows::hook::HookEvent {
        identity: keyweave_windows::keys::ResolvedKey {
            key: keyweave_types::KeyId::new(keyweave_types::KEYBOARD_PAGE, usage),
            known_aliases: false,
        },
        edge,
        binding: if edge == keyweave_types::KeyEdge::Down {
            None
        } else {
            Some(keyweave_types::CaptureDecision::Capture)
        },
    }
}
#[test]
fn staged_chord_reload_ack_follows_actual_session_activation_and_execution() {
    use keyweave_types::{KeyEdge, MonoTime};
    let calls = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let mut actor = keyweave_windows::runtime::SessionActor::new_staged(
        ChordOutput(calls.clone()),
        keyweave_windows::keys::InjectionTag::new(1).unwrap(),
        windows::PreparedConfig::from_bytes(BODY.to_vec())
            .unwrap()
            .snapshot()
            .clone(),
    )
    .unwrap();
    let mut rx = receiver(CHORD);
    let mut slot = PreparationSlot::new();
    slot.start(rx.take().unwrap(), Target::WindowsStaged)
        .unwrap();
    assert_eq!(wait(&mut slot), PreparationStep::Ready);
    assert!(calls.borrow().is_empty());
    assert_eq!(actor.config_generation(), ConfigGeneration(1));
    let ack = slot.deliver(&mut rx, &mut actor).unwrap();
    assert_eq!(ack.outcome, Outcome::Accepted);
    assert_eq!(ack.active, actor.config_generation());
    assert_eq!(rx.active(), ConfigGeneration(2));
    let mut writer = AckWriter::new(Vec::new(), ack).unwrap();
    rx.ack_sent(writer.poll().unwrap().unwrap()).unwrap();
    actor.start_after_neutral().unwrap();
    actor
        .key(chord_event(4, KeyEdge::Down), MonoTime(100))
        .unwrap();
    assert!(calls.borrow().is_empty());
    actor
        .key(chord_event(5, KeyEdge::Down), MonoTime(110))
        .unwrap();
    actor
        .key(chord_event(4, KeyEdge::Up), MonoTime(120))
        .unwrap();
    actor
        .key(chord_event(5, KeyEdge::Up), MonoTime(130))
        .unwrap();
    assert_eq!(
        *calls.borrow(),
        [(0x2e, 8), (0x2e, 10), (0x20, 8), (0x20, 10)]
    );
    assert!(actor.output().ledger().submitted().is_empty());
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_chord_reload_keeps_held_old_group_after_acknowledging_new_config() {
    use keyweave_types::{KeyEdge, MonoTime};
    let calls = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
    let snapshot =
        windows::PreparedConfig::read_with_mode(CHORD, windows::ExecutionMode::StagedKeyboard)
            .unwrap()
            .snapshot()
            .clone();
    let mut actor = keyweave_windows::runtime::SessionActor::new_staged(
        ChordOutput(calls.clone()),
        keyweave_windows::keys::InjectionTag::new(1).unwrap(),
        snapshot,
    )
    .unwrap();
    actor.start_after_neutral().unwrap();
    actor
        .key(chord_event(4, KeyEdge::Down), MonoTime(100))
        .unwrap();
    actor
        .key(chord_event(5, KeyEdge::Down), MonoTime(110))
        .unwrap();
    let mut rx = receiver(BODY);
    let mut slot = PreparationSlot::new();
    slot.start(rx.take().unwrap(), Target::WindowsStaged)
        .unwrap();
    wait(&mut slot);
    assert_eq!(
        slot.deliver(&mut rx, &mut actor).unwrap().outcome,
        Outcome::Accepted
    );
    actor
        .key(chord_event(4, KeyEdge::Up), MonoTime(120))
        .unwrap();
    actor
        .key(chord_event(5, KeyEdge::Up), MonoTime(130))
        .unwrap();
    assert_eq!(
        *calls.borrow(),
        [(0x2e, 8), (0x2e, 10), (0x20, 8), (0x20, 10)]
    );
    actor
        .key(chord_event(4, KeyEdge::Down), MonoTime(140))
        .unwrap();
    actor
        .key(chord_event(4, KeyEdge::Up), MonoTime(150))
        .unwrap();
    assert_eq!(&calls.borrow()[4..], [(0x30, 8), (0x30, 10)]);
    actor.stop_after_unhook().unwrap();
}
#[test]
fn staged_chord_reload_rejects_wrong_mode_and_legacy_compilation_without_activation() {
    for (staged, target) in [(false, Target::WindowsStaged), (true, Target::Windows)] {
        let snapshot = windows::PreparedConfig::from_bytes(BODY.to_vec())
            .unwrap()
            .snapshot()
            .clone();
        let tag = keyweave_windows::keys::InjectionTag::new(1).unwrap();
        let mut actor = if staged {
            keyweave_windows::runtime::SessionActor::new_staged(NoOutput, tag, snapshot).unwrap()
        } else {
            keyweave_windows::runtime::SessionActor::new(NoOutput, tag, snapshot).unwrap()
        };
        let mut rx = receiver(CHORD);
        let mut slot = PreparationSlot::new();
        slot.start(rx.take().unwrap(), target).unwrap();
        wait(&mut slot);
        let ack = slot.deliver(&mut rx, &mut actor).unwrap();
        assert_eq!(ack.outcome, Outcome::Rejected);
        assert_eq!(ack.active, ConfigGeneration(1));
        assert_eq!(actor.config_generation(), ConfigGeneration(1));
        assert_eq!(rx.active(), ConfigGeneration(1));
    }
}
