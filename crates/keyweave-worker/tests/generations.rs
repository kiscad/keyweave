//! Cold parsing and deterministic engine only; no files, worker or input APIs.
#![allow(clippy::unwrap_used)]
use keyweave_engine::{ConfigSnapshot, Engine, EngineError};
use keyweave_types::{
    ConfigGeneration, DeviceInstanceId, InputEnvelope, InputEvent, KEYBOARD_PAGE, KeyEdge, KeyId,
    MonoTime, Provenance,
};
use keyweave_worker::{
    LoadError, linux_preparation as linux, next_config_generation, preparation as windows,
};
use std::{
    io::{self, Read},
    sync::Arc,
};
const OLD:&[u8]=br#"// private exact bytes
{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
const NEW:&[u8]=br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"c"}]}]}]}"#;
#[derive(Clone, Copy)]
enum Platform {
    Linux,
    Windows,
}
fn initial(platform: Platform) -> Arc<ConfigSnapshot> {
    match platform {
        Platform::Linux => linux::PreparedConfig::read(OLD, linux::default_output_spec().unwrap())
            .unwrap()
            .snapshot()
            .clone(),
        Platform::Windows => windows::PreparedConfig::read(OLD)
            .unwrap()
            .snapshot()
            .clone(),
    }
}
fn next(
    platform: Platform,
    bytes: &[u8],
    current: ConfigGeneration,
) -> Result<Arc<ConfigSnapshot>, String> {
    match platform {
        Platform::Linux => {
            linux::PreparedConfig::read_next(bytes, linux::default_output_spec().unwrap(), current)
                .map(|p| p.snapshot().clone())
                .map_err(|e| e.to_string())
        }
        Platform::Windows => windows::PreparedConfig::read_next(bytes, current)
            .map(|p| p.snapshot().clone())
            .map_err(|e| e.to_string()),
    }
}
fn event(seq: u64, edge: KeyEdge) -> InputEnvelope {
    InputEnvelope {
        seq,
        at_ns: MonoTime(seq),
        device: DeviceInstanceId {
            id: 1,
            generation: 1,
        },
        provenance: Provenance::Physical,
        event: InputEvent::Key {
            key: KeyId::new(KEYBOARD_PAGE, 4),
            edge,
        },
    }
}
#[test]
fn both_prepared_factories_keep_old_held_binding_until_release() {
    for platform in [Platform::Linux, Platform::Windows] {
        let old = initial(platform);
        let new = next(platform, NEW, old.generation).unwrap();
        assert_eq!(old.generation, ConfigGeneration(1));
        assert_eq!(new.generation, ConfigGeneration(2));
        let mut engine = Engine::new(old.clone()).unwrap();
        let down = engine.handle(event(1, KeyEdge::Down)).unwrap();
        assert_eq!(down.outputs[0].key, KeyId::new(KEYBOARD_PAGE, 5));
        engine.activate(new).unwrap();
        assert_eq!(
            engine.state().binding_generations,
            vec![ConfigGeneration(1)]
        );
        let repeat = engine.handle(event(2, KeyEdge::Repeat)).unwrap();
        assert!(
            repeat
                .outputs
                .iter()
                .all(|output| output.key == KeyId::new(KEYBOARD_PAGE, 5))
        );
        let up = engine.handle(event(3, KeyEdge::Up)).unwrap();
        assert_eq!(up.outputs[0].key, KeyId::new(KEYBOARD_PAGE, 5));
        assert_eq!(up.outputs[0].edge, KeyEdge::Up);
        let down = engine.handle(event(4, KeyEdge::Down)).unwrap();
        assert_eq!(down.outputs[0].key, KeyId::new(KEYBOARD_PAGE, 6));
        assert_eq!(
            engine.state().binding_generations,
            vec![ConfigGeneration(2)]
        );
        assert_eq!(old.generation, ConfigGeneration(1));
    }
}
struct Unreadable;
impl Read for Unreadable {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        panic!("invalid/exhausted generation must fail before IO");
    }
}
#[test]
fn zero_and_exhausted_generations_fail_before_read_on_both_platforms() {
    assert!(matches!(
        next_config_generation(ConfigGeneration(0)),
        Err(LoadError::Generation)
    ));
    assert!(matches!(
        next_config_generation(ConfigGeneration(u64::MAX)),
        Err(LoadError::Generation)
    ));
    assert!(matches!(
        windows::PreparedConfig::read_for_generation(Unreadable, ConfigGeneration(0)),
        Err(windows::PreparationError::Load(LoadError::Generation))
    ));
    assert!(matches!(
        linux::PreparedConfig::read_for_generation(
            Unreadable,
            linux::default_output_spec().unwrap(),
            ConfigGeneration(0)
        ),
        Err(linux::PreparationError::Load(LoadError::Generation))
    ));
    for current in [ConfigGeneration(0), ConfigGeneration(u64::MAX)] {
        assert!(matches!(
            windows::PreparedConfig::read_next(Unreadable, current),
            Err(windows::PreparationError::Load(LoadError::Generation))
        ));
        assert!(matches!(
            linux::PreparedConfig::read_next(
                Unreadable,
                linux::default_output_spec().unwrap(),
                current
            ),
            Err(linux::PreparationError::Load(LoadError::Generation))
        ));
    }
}
#[test]
fn preparation_is_not_activation_ack_and_stale_candidate_cannot_rebind() {
    for platform in [Platform::Linux, Platform::Windows] {
        let mut engine = Engine::new(initial(platform)).unwrap();
        engine.handle(event(1, KeyEdge::Down)).unwrap();
        assert!(next(platform, b"invalid private body", ConfigGeneration(1)).is_err());
        let candidate = next(platform, NEW, ConfigGeneration(1)).unwrap();
        let stale = next(platform, OLD, ConfigGeneration(1)).unwrap();
        assert_eq!(candidate.generation, stale.generation);
        engine.activate(candidate).unwrap();
        let before = engine.state();
        assert_eq!(engine.activate(stale), Err(EngineError::InvalidGeneration));
        assert_eq!(engine.state(), before);
        assert_eq!(
            engine.handle(event(2, KeyEdge::Up)).unwrap().outputs[0].key,
            KeyId::new(KEYBOARD_PAGE, 5)
        );
        assert_eq!(
            engine.handle(event(3, KeyEdge::Down)).unwrap().outputs[0].key,
            KeyId::new(KEYBOARD_PAGE, 6)
        );
    }
}
#[test]
fn explicit_last_generation_retains_exact_bytes_without_wrapping() {
    let windows = windows::PreparedConfig::from_bytes_for_generation(
        OLD.to_vec(),
        ConfigGeneration(u64::MAX),
    )
    .unwrap();
    let linux = linux::PreparedConfig::from_bytes_for_generation(
        OLD.to_vec(),
        linux::default_output_spec().unwrap(),
        ConfigGeneration(u64::MAX),
    )
    .unwrap();
    assert_eq!(windows.bytes(), OLD);
    assert_eq!(linux.bytes(), OLD);
    assert_eq!(windows.snapshot().generation, ConfigGeneration(u64::MAX));
    assert_eq!(linux.snapshot().generation, ConfigGeneration(u64::MAX));
    assert_eq!(
        next_config_generation(ConfigGeneration(u64::MAX - 1)).unwrap(),
        ConfigGeneration(u64::MAX)
    );
}

#[test]
fn reload_protocol_ack_follows_engine_commit_and_not_prepared_candidate() {
    use keyweave_progress::reload::{Ack, Client, MAX_UPDATE_BYTES, Outcome, Request, Worker};
    assert_eq!(MAX_UPDATE_BYTES as usize, keyweave_config::MAX_CONFIG_BYTES);
    for platform in [Platform::Linux, Platform::Windows] {
        let mut engine = Engine::new(initial(platform)).unwrap();
        engine.handle(event(1, KeyEdge::Down)).unwrap();
        let mut client = Client::new(7, engine.config_generation(), MonoTime(0), 100).unwrap();
        let mut worker = Worker::new(7, engine.config_generation()).unwrap();
        let request = Request::decode(
            &client
                .begin(NEW.len() as u64, MonoTime(1))
                .unwrap()
                .encode()
                .unwrap(),
        )
        .unwrap();
        worker.begin(request).unwrap();
        let candidate = next(platform, NEW, request.base).unwrap();
        assert_eq!(engine.config_generation(), ConfigGeneration(1));
        assert_eq!(client.active(), ConfigGeneration(1));
        engine.activate(candidate).unwrap();
        assert_eq!(engine.config_generation(), ConfigGeneration(2));
        assert_eq!(
            engine.state().binding_generations,
            vec![ConfigGeneration(1)]
        );
        let ack = Ack::decode(
            &worker
                .finish(Outcome::Accepted, engine.config_generation())
                .unwrap()
                .encode()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(client.active(), ConfigGeneration(1));
        client.accept(ack, MonoTime(2)).unwrap();
        assert_eq!(client.active(), ConfigGeneration(2));
        assert_eq!(
            engine.handle(event(2, KeyEdge::Up)).unwrap().outputs[0].key,
            KeyId::new(KEYBOARD_PAGE, 5)
        );
        assert_eq!(
            engine.handle(event(3, KeyEdge::Down)).unwrap().outputs[0].key,
            KeyId::new(KEYBOARD_PAGE, 6)
        );
    }
}
