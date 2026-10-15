#![allow(clippy::unwrap_used)]
use keyweave_config::{CompileMode, compile_with_mode};
use keyweave_engine::{
    ConfigSnapshot, Engine, EngineState, physical_controller::PhysicalStageController,
};
use keyweave_types::*;
use serde_json::{Value, json};
use std::sync::Arc;
fn config(explicit: bool, mode: CompileMode) -> Arc<ConfigSnapshot> {
    let mut suppress = json!({"type":"basic","from":{"key_code":"a"}});
    if explicit {
        suppress["to"] = json!([]);
    }
    let raw = json!({"profiles":[{"selected":true,"complex_modifications":{"rules":[{"manipulators":[suppress,{"type":"basic","from":{"key_code":"a"},"to":[{"key_code":"z"}]}]}]}}]});
    let compiled = compile_with_mode(
        &raw.to_string(),
        &BackendCapabilities::offline(),
        ConfigGeneration(1),
        mode,
    )
    .unwrap();
    assert!(
        compiled.executable_without_loss(),
        "{:?}",
        compiled.diagnostics
    );
    assert!(compiled.snapshot.complex[0].to.is_empty());
    assert_eq!(compiled.raw, raw);
    Arc::new(compiled.snapshot)
}
enum Runner {
    Legacy(Box<Engine>),
    Staged(Box<PhysicalStageController>),
}
impl Runner {
    fn new(config: Arc<ConfigSnapshot>, mode: CompileMode) -> Self {
        match mode {
            CompileMode::Legacy => Self::Legacy(Box::new(Engine::new(config).unwrap())),
            CompileMode::StagedKeyboard => {
                Self::Staged(Box::new(PhysicalStageController::new(config).unwrap()))
            }
        }
    }
    fn step(&mut self, seq: u64, event: InputEvent) -> Vec<(KeyId, KeyEdge)> {
        let at = MonoTime(seq);
        let input = InputEnvelope {
            seq,
            at_ns: at,
            device: DeviceInstanceId {
                id: 1,
                generation: 1,
            },
            provenance: Provenance::Physical,
            event,
        };
        let mut output = match self {
            Self::Legacy(engine) => engine.handle(input).unwrap().outputs,
            Self::Staged(actor) => {
                let step = actor.handle(input).unwrap();
                let mut output = step.outputs;
                for _ in 0..128 {
                    match actor.poll(at).unwrap() {
                        Some(step) => output.extend(step.outputs),
                        None => return output.into_iter().map(|o| (o.key, o.edge)).collect(),
                    }
                }
                panic!("bounded fixture failed to drain")
            }
        };
        output.drain(..).map(|o| (o.key, o.edge)).collect()
    }
    fn key(&mut self, seq: u64, name: &str, edge: KeyEdge) -> Vec<(KeyId, KeyEdge)> {
        self.step(
            seq,
            InputEvent::Key {
                key: name.parse().unwrap(),
                edge,
            },
        )
    }
    fn activate_empty(&mut self) {
        let empty = Arc::new(ConfigSnapshot::empty(ConfigGeneration(2)));
        match self {
            Self::Legacy(e) => e.activate(empty).unwrap(),
            Self::Staged(e) => e.activate(empty).unwrap(),
        }
    }
    fn state(&self) -> EngineState {
        match self {
            Self::Legacy(e) => e.state(),
            Self::Staged(e) => e.state(),
        }
    }
}
#[test]
fn omitted_to_matches_empty_to_and_prevents_fallthrough_in_both_execution_modes() {
    for mode in [CompileMode::Legacy, CompileMode::StagedKeyboard] {
        let mut traces = vec![];
        for explicit in [false, true] {
            let mut runner = Runner::new(config(explicit, mode), mode);
            let mut trace = vec![];
            trace.extend(runner.key(1, "a", KeyEdge::Down));
            trace.extend(runner.key(2, "a", KeyEdge::Repeat));
            assert_eq!(runner.state().bindings, 1);
            trace.extend(runner.key(3, "b", KeyEdge::Down));
            trace.extend(runner.key(4, "a", KeyEdge::Up));
            trace.extend(runner.key(5, "b", KeyEdge::Up));
            assert_eq!(
                trace,
                vec![
                    ("b".parse().unwrap(), KeyEdge::Down),
                    ("b".parse().unwrap(), KeyEdge::Up)
                ]
            );
            assert_eq!(runner.state().bindings, 0);
            assert!(runner.state().pressed_outputs.is_empty());
            assert_eq!(runner.state().output_owners, 0);
            traces.push(trace);
        }
        assert_eq!(traces[0], traces[1]);
    }
}
#[test]
fn suppressed_binding_keeps_old_snapshot_across_reload_and_reset_never_invents_up() {
    for mode in [CompileMode::Legacy, CompileMode::StagedKeyboard] {
        let mut runner = Runner::new(config(false, mode), mode);
        assert!(runner.key(1, "a", KeyEdge::Down).is_empty());
        runner.activate_empty();
        assert!(runner.key(2, "a", KeyEdge::Repeat).is_empty());
        assert!(runner.key(3, "a", KeyEdge::Up).is_empty());
        assert_eq!(
            runner.key(4, "a", KeyEdge::Down),
            vec![("a".parse().unwrap(), KeyEdge::Down)]
        );
        assert_eq!(
            runner.key(5, "a", KeyEdge::Up),
            vec![("a".parse().unwrap(), KeyEdge::Up)]
        );
        let mut runner = Runner::new(config(false, mode), mode);
        assert!(runner.key(1, "a", KeyEdge::Down).is_empty());
        assert!(runner.step(2, InputEvent::Reset).is_empty());
        assert_eq!(runner.state().bindings, 0);
        assert_eq!(runner.state().output_owners, 0);
        assert!(runner.state().pressed_outputs.is_empty());
    }
}
#[test]
fn rule_notes_are_typed_metadata_while_unknown_functional_fields_still_reject() {
    let rule = json!({"description_notes":["note","中文"],"maintainers":["author"],"manipulators":[{"type":"basic","from":{"key_code":"a"}}]});
    let wrap = |rule: Value| json!({"profiles":[{"complex_modifications":{"rules":[rule]}}]});
    let raw = wrap(rule.clone());
    let compiled = compile_with_mode(
        &raw.to_string(),
        &BackendCapabilities::offline(),
        ConfigGeneration(1),
        CompileMode::Legacy,
    )
    .unwrap();
    assert!(compiled.executable_without_loss());
    assert_eq!(compiled.raw, raw);
    for notes in [json!(null), json!("string"), json!(["valid", 4])] {
        let mut bad = rule.clone();
        bad["description_notes"] = notes;
        assert!(
            compile_with_mode(
                &wrap(bad).to_string(),
                &BackendCapabilities::offline(),
                ConfigGeneration(1),
                CompileMode::Legacy
            )
            .is_err()
        );
    }
    let mut bad = rule;
    bad["unknown_behavior"] = json!(true);
    let compiled = compile_with_mode(
        &wrap(bad).to_string(),
        &BackendCapabilities::offline(),
        ConfigGeneration(1),
        CompileMode::Legacy,
    )
    .unwrap();
    assert!(!compiled.executable_without_loss());
    assert!(compiled.snapshot.complex.is_empty());
    for mapping in [
        json!({"from":{"key_code":"a"}}),
        json!({"from":{"key_code":"a"},"to":[]}),
    ] {
        let raw = json!({"profiles":[{"simple_modifications":[mapping]}]});
        let compiled = compile_with_mode(
            &raw.to_string(),
            &BackendCapabilities::offline(),
            ConfigGeneration(1),
            CompileMode::Legacy,
        )
        .unwrap();
        assert!(!compiled.executable_without_loss());
    }
}
