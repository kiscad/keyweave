//! Actual cold compiler/Engine activation with a test-only backend ticket.
//! No native worker transport, progress receipt, input device or recovery claim.
#![allow(clippy::unwrap_used)]
use keyweave_engine::Engine;
use keyweave_progress::{
    reload,
    session_apply::{Admission, Backend, BackendError, Completion, Outcome, Request, Transaction},
    session_control::{Phase, Snapshot},
};
use keyweave_types::{
    ConfigGeneration, DeviceInstanceId, InputEnvelope, InputEvent, KeyEdge, KeyId, MonoTime,
    Provenance,
};
use keyweave_worker::linux_preparation::{PreparedConfig, default_output_spec};
const OLD:&[u8]=br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}"#;
const NEW:&[u8]=br#"{"profiles":[{"selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"c"}]}]}]}"#;
struct TestBackend {
    actor: Engine,
    pending: Option<(u64, Box<[u8]>, u64)>,
    next: u64,
    submit_count: usize,
    lose_confirmation: bool,
}
impl TestBackend {
    fn new() -> Self {
        let prepared =
            PreparedConfig::from_bytes(OLD.to_vec(), default_output_spec().unwrap()).unwrap();
        Self {
            actor: Engine::new(prepared.snapshot().clone()).unwrap(),
            pending: None,
            next: 1,
            submit_count: 0,
            lose_confirmation: false,
        }
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            session: 11,
            phase: Phase::Active,
            progress_epoch: 1,
            generation: Some(self.actor.config_generation().0),
            exit: None,
            recovery_confirmed: false,
            fault: false,
        }
    }
    fn input(&mut self, seq: u64, edge: KeyEdge) -> KeyId {
        self.actor
            .handle(InputEnvelope {
                seq,
                at_ns: MonoTime(seq),
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                provenance: Provenance::Physical,
                event: InputEvent::Key {
                    key: KeyId::new(7, 4),
                    edge,
                },
            })
            .unwrap()
            .outputs[0]
            .key
    }
}
impl Backend for TestBackend {
    type Ticket = u64;
    fn submit(
        &mut self,
        body: Box<[u8]>,
        base: u64,
        target: u64,
    ) -> Result<Admission<u64>, BackendError> {
        assert!(self.pending.is_none());
        assert_eq!(self.actor.config_generation().0, base);
        self.submit_count += 1;
        let ticket = self.next;
        self.next += 1;
        self.pending = Some((ticket, body, target));
        Ok(Admission::Queued(ticket))
    }
    fn completion(&mut self, ticket: &u64) -> Result<Option<Completion<u64>>, BackendError> {
        let Some((actual, body, target)) = self.pending.take() else {
            return Ok(None);
        };
        assert_eq!(actual, *ticket);
        // Test-only synchronous cold preparation. Production uses its existing
        // asynchronous worker lane and must not compile inside a host/UI poll.
        let outcome = match PreparedConfig::from_bytes_for_generation(
            body.into_vec(),
            default_output_spec().unwrap(),
            ConfigGeneration(target),
        ) {
            Ok(prepared) => {
                self.actor.activate(prepared.snapshot().clone()).unwrap();
                reload::Outcome::Accepted
            }
            Err(_) => reload::Outcome::Rejected,
        };
        if self.lose_confirmation {
            return Ok(None);
        }
        Ok(Some(Completion {
            ticket: actual,
            outcome,
            generation: self.actor.config_generation().0,
        }))
    }
}
fn transaction(bytes: &[u8]) -> Transaction<u64> {
    let request = Request {
        id: 5,
        session: 11,
        base: 1,
        target: 2,
        byte_length: bytes.len() as u64,
    };
    let encoded = request.encode().unwrap();
    Transaction::new(
        Request::decode(&encoded).unwrap(),
        bytes.into(),
        MonoTime(0),
        100,
    )
    .unwrap()
}
#[test]
fn public_apply_confirms_real_cold_activation_and_keeps_old_held_binding() {
    let mut backend = TestBackend::new();
    assert_eq!(backend.input(1, KeyEdge::Down), KeyId::new(7, 5));
    let mut tx = transaction(NEW);
    assert!(
        tx.step(backend.snapshot(), &mut backend, || MonoTime(1))
            .unwrap()
            .is_none()
    );
    assert_eq!(backend.actor.config_generation(), ConfigGeneration(1));
    let reply = tx
        .step(backend.snapshot(), &mut backend, || MonoTime(2))
        .unwrap()
        .unwrap();
    assert_eq!(reply.outcome, Outcome::Committed);
    assert_eq!(reply.observed, Some(2));
    assert_eq!(backend.actor.config_generation(), ConfigGeneration(2));
    assert_eq!(backend.input(2, KeyEdge::Up), KeyId::new(7, 5));
    assert_eq!(backend.input(3, KeyEdge::Down), KeyId::new(7, 6));
    assert_eq!(backend.input(4, KeyEdge::Up), KeyId::new(7, 6));
    assert_eq!(backend.submit_count, 1);
}
#[test]
fn compile_rejection_keeps_generation_and_lost_confirmation_is_not_rolled_back() {
    let mut backend = TestBackend::new();
    let mut rejected = transaction(b"{ invalid json");
    rejected
        .step(backend.snapshot(), &mut backend, || MonoTime(1))
        .unwrap();
    let reply = rejected
        .step(backend.snapshot(), &mut backend, || MonoTime(2))
        .unwrap()
        .unwrap();
    assert_eq!(reply.outcome, Outcome::Rejected);
    assert_eq!(reply.observed, Some(1));
    assert_eq!(backend.actor.config_generation(), ConfigGeneration(1));
    backend.lose_confirmation = true;
    let mut unknown = transaction(NEW);
    unknown
        .step(backend.snapshot(), &mut backend, || MonoTime(3))
        .unwrap();
    assert!(
        unknown
            .step(backend.snapshot(), &mut backend, || MonoTime(4))
            .unwrap()
            .is_none()
    );
    assert_eq!(backend.actor.config_generation(), ConfigGeneration(2));
    let failure = unknown
        .step(backend.snapshot(), &mut backend, || MonoTime(100))
        .unwrap_err();
    assert!(failure.may_have_applied);
    assert_eq!(backend.actor.config_generation(), ConfigGeneration(2));
    assert_eq!(backend.submit_count, 2);
    assert!(
        unknown
            .step(backend.snapshot(), &mut backend, || MonoTime(101))
            .is_err()
    );
    assert_eq!(backend.submit_count, 2);
}
