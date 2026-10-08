#![allow(clippy::unwrap_used)]
use keyweave_engine::{ConfigSnapshot, Engine, Mapping};
use keyweave_output::{Acknowledgment, Mode, OutputQueue};
use keyweave_types::*;
use std::sync::Arc;

#[test]
fn engine_desired_release_does_not_erase_a_partially_submitted_down() {
    let a: KeyId = "a".parse().unwrap();
    let b: KeyId = "b".parse().unwrap();
    let mut engine = Engine::new(Arc::new(ConfigSnapshot {
        generation: ConfigGeneration(1),
        name: "pipeline".into(),
        complex: vec![],
        simple: vec![Mapping {
            id: "a-b".into(),
            from: a,
            to: vec![b.into()],
            allow_extra_modifiers: true,
            extras: Default::default(),
        }],
    }))
    .unwrap();
    let mut output = OutputQueue::new();
    output.resume().unwrap();
    for (seq, edge) in [(1, KeyEdge::Down), (2, KeyEdge::Up)] {
        let step = engine
            .handle(InputEnvelope {
                seq,
                at_ns: MonoTime(0),
                device: DeviceInstanceId {
                    id: 1,
                    generation: 1,
                },
                provenance: Provenance::Physical,
                event: InputEvent::Key { key: a, edge },
            })
            .unwrap();
        output.enqueue(&step.outputs).unwrap();
    }
    assert!(engine.state().pressed_outputs.is_empty());
    assert!(output.desired().is_empty());
    let batch = output.begin_due(MonoTime(0)).unwrap().unwrap();
    assert_eq!(batch.edges.len(), 2);
    assert_eq!(
        output.acknowledge(batch.id, 1).unwrap(),
        Acknowledgment::RecoveryRequired
    );
    assert!(output.submitted().contains(&b));
    let cleanup = output.begin_due(MonoTime(0)).unwrap().unwrap();
    assert_eq!(
        cleanup.edges,
        [OutputIntent {
            at_ns: MonoTime(0),
            key: b,
            edge: KeyEdge::Up
        }]
    );
    output.acknowledge(cleanup.id, 1).unwrap();
    assert_eq!(output.mode(), Mode::Paused);
    assert!(output.submitted().is_empty());
}

#[test]
fn terminal_duration_execution_and_output_short_return_keep_separate_intent_and_submitted_state() {
    use keyweave_engine::{Action, action_plan::*, terminal_actor::TerminalActor, timeline::Stage};
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let mut actor = TerminalActor::new(config.clone()).unwrap();
    let device = DeviceInstanceId {
        id: 1,
        generation: 1,
    };
    actor.observe_device(device, false, MonoTime(0)).unwrap();
    let plan = plan_actions(
        &[Action {
            hold_down_ns: 100,
            .."b".parse::<KeyId>().unwrap().into()
        }],
        PlanOrigin {
            batch: BatchId(1),
            stage: Stage::Output,
            start: MonoTime(0),
            device,
            source: SourceToken(1),
            config,
        },
        true,
    )
    .unwrap();
    actor.enqueue(plan).unwrap();
    let mut output = OutputQueue::new();
    output.resume().unwrap();
    output
        .enqueue(&actor.poll(MonoTime(0)).unwrap().unwrap().outputs)
        .unwrap();
    let down = output.begin_due(MonoTime(0)).unwrap().unwrap();
    output.acknowledge(down.id, 1).unwrap();
    assert!(actor.poll(MonoTime(99)).unwrap().is_none());
    output
        .enqueue(&actor.poll(MonoTime(100)).unwrap().unwrap().outputs)
        .unwrap();
    assert!(actor.pressed_outputs().is_empty());
    let up = output.begin_due(MonoTime(100)).unwrap().unwrap();
    assert_eq!(
        output.acknowledge(up.id, 0).unwrap(),
        Acknowledgment::RecoveryRequired
    );
    assert_eq!(output.submitted().len(), 1);
    let recovery = output.begin_due(MonoTime(100)).unwrap().unwrap();
    assert_eq!(recovery.edges[0].edge, KeyEdge::Up);
    output.acknowledge(recovery.id, 1).unwrap();
    assert!(output.submitted().is_empty());
    assert_eq!(output.mode(), Mode::Paused);
}
