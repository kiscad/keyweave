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
