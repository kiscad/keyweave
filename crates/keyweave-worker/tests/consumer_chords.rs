//! Actual compiler/canonical evdev mapping/staged controller/output ledger.
//! Mock frame sender only: no device files, grabs, session or native injection.
#![allow(clippy::unwrap_used)]
use keyweave_engine::{ConfigSnapshot, physical_controller::PhysicalStageController};
use keyweave_linux::{
    events::{FrameWrite, NativeKey},
    keys::{self, IdentityPolicy},
    output::{DeviceSpec, WriteError},
    pump::{FrameBackend, OutputPump},
};
use keyweave_types::*;
use keyweave_worker::linux_preparation::{ExecutionMode, PreparedConfig, default_output_spec};
use std::{cell::RefCell, io, rc::Rc, sync::Arc};
struct Sender {
    spec: DeviceSpec,
    log: Rc<RefCell<Vec<NativeKey>>>,
}
impl FrameBackend for Sender {
    fn spec(&self) -> &DeviceSpec {
        &self.spec
    }
    fn submit(
        &mut self,
        keys: &[NativeKey],
        _: bool,
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        self.log.borrow_mut().extend_from_slice(keys);
        Ok(Ok(FrameWrite::Complete {
            accepted: keys.len(),
        }))
    }
    fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        Ok(Ok(true))
    }
}
const DEVICE: DeviceInstanceId = DeviceInstanceId {
    id: 1,
    generation: 1,
};
struct Driver {
    controller: PhysicalStageController,
    pump: OutputPump<Sender>,
    log: Rc<RefCell<Vec<NativeKey>>>,
    seq: u64,
}
impl Driver {
    fn new(snapshot: Arc<ConfigSnapshot>) -> Self {
        let mut controller = PhysicalStageController::new_with_chord_model(snapshot).unwrap();
        controller
            .observe_device(DEVICE, false, MonoTime(0))
            .unwrap();
        let log = Rc::new(RefCell::new(vec![]));
        let mut pump = OutputPump::new(Sender {
            spec: default_output_spec().unwrap(),
            log: log.clone(),
        });
        pump.resume().unwrap();
        Self {
            controller,
            pump,
            log,
            seq: 0,
        }
    }
    fn flush(&mut self, at: u64) {
        let mut done = false;
        for _ in 0..256 {
            match self.controller.poll(MonoTime(at)).unwrap() {
                Some(step) => self.pump.enqueue(&step.outputs).unwrap(),
                None => {
                    done = true;
                    break;
                }
            }
        }
        assert!(done, "controller fixture budget exhausted");
        for _ in 0..256 {
            if !self
                .pump
                .ledger()
                .next_deadline()
                .is_some_and(|due| due <= MonoTime(at))
            {
                return;
            }
            self.pump.poll(MonoTime(at)).unwrap();
        }
        panic!("output fixture budget exhausted");
    }
    fn input(&mut self, at: u64, code: u16, edge: KeyEdge) {
        self.flush(at);
        self.seq += 1;
        let resolved = keys::resolve_input(code, None, IdentityPolicy::CanonicalEvdev).unwrap();
        if code == 113 {
            assert_eq!(resolved.key, KeyId::new(CONSUMER_PAGE, 0xe2));
        }
        let step = self
            .controller
            .handle(InputEnvelope {
                seq: self.seq,
                at_ns: MonoTime(at),
                device: DEVICE,
                provenance: Provenance::Physical,
                event: InputEvent::Key {
                    key: resolved.key,
                    edge,
                },
            })
            .unwrap();
        self.pump.enqueue(&step.outputs).unwrap();
        self.flush(at);
    }
}
fn prepared(consumer_first: bool) -> PreparedConfig {
    let members = if consumer_first {
        r#"{"consumer_key_code":"mute"},{"key_code":"a"}"#
    } else {
        r#"{"key_code":"a"},{"consumer_key_code":"mute"}"#
    };
    let text = format!(
        r#"{{"profiles":[{{"selected":true,"complex_modifications":{{"rules":[{{"manipulators":[{{"type":"basic","from":{{"simultaneous":[{members}],"simultaneous_options":{{"key_up_when":"all"}}}},"to":[{{"key_code":"x"}}]}}]}}]}}}}]}}"#
    );
    PreparedConfig::read_with_mode(
        text.as_bytes(),
        default_output_spec().unwrap(),
        ExecutionMode::StagedKeyboard,
    )
    .unwrap()
}
#[test]
fn linux_media_and_keyboard_chord_matches_in_both_first_member_orders() {
    for consumer_first in [true, false] {
        let prepared = prepared(consumer_first);
        let mut driver = Driver::new(prepared.snapshot().clone());
        let first = if consumer_first { 113 } else { 30 };
        let second = if consumer_first { 30 } else { 113 };
        driver.input(0, first, KeyEdge::Down);
        assert!(driver.log.borrow().is_empty());
        driver.input(1, second, KeyEdge::Down);
        driver.flush(50_000_000);
        assert_eq!(
            driver
                .pump
                .ledger()
                .submitted()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            ["x".parse().unwrap()]
        );
        driver.input(50_000_001, first, KeyEdge::Up);
        assert_eq!(driver.pump.ledger().submitted().len(), 1);
        driver.input(50_000_002, second, KeyEdge::Up);
        assert!(driver.pump.ledger().submitted().is_empty());
        assert_eq!(
            driver
                .log
                .borrow()
                .iter()
                .map(|key| (key.code, key.value))
                .collect::<Vec<_>>(),
            [(45, 1), (45, 0)]
        );
    }
}
#[test]
fn linux_unmatched_media_chord_replays_only_the_saved_media_source() {
    let prepared = prepared(true);
    let mut driver = Driver::new(prepared.snapshot().clone());
    driver.input(0, 113, KeyEdge::Down);
    assert!(driver.log.borrow().is_empty());
    driver.flush(50_000_001);
    assert!(
        driver
            .pump
            .ledger()
            .submitted()
            .contains(&KeyId::new(CONSUMER_PAGE, 0xe2))
    );
    driver.input(50_000_002, 113, KeyEdge::Up);
    assert!(driver.pump.ledger().submitted().is_empty());
    assert_eq!(
        driver
            .log
            .borrow()
            .iter()
            .map(|key| (key.code, key.value))
            .collect::<Vec<_>>(),
        [(113, 1), (113, 0)]
    );
}
