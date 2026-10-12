#![allow(clippy::unwrap_used)]
use keyweave_engine::{action_plan::*, terminal_actor::*, timeline::*, *};
use keyweave_types::*;
use std::sync::Arc;
#[test]
fn saved_initial_targets_slots_and_old_arc_survive_fifo_shift_delivery_reload_and_saved_up() {
    let config = Arc::new(ConfigSnapshot::empty(ConfigGeneration(1)));
    let device = DeviceInstanceId {
        id: 1,
        generation: 1,
    };
    let mut actor = TerminalActor::new(config.clone()).unwrap();
    actor.observe_device(device, false, MonoTime(0)).unwrap();
    let key = |name: &str| name.parse::<KeyId>().unwrap();
    let origin = |batch, at| PlanOrigin {
        batch: BatchId(batch),
        stage: Stage::Output,
        start: MonoTime(at),
        device,
        source: SourceToken(batch),
        config: config.clone(),
    };
    actor
        .enqueue(
            plan_actions(
                &[Action {
                    hold_down_ns: 50,
                    ..key("x").into()
                }],
                origin(1, 100),
                true,
            )
            .unwrap(),
        )
        .unwrap();
    let plan = plan_actions(&[key("y").into()], origin(2, 110), false).unwrap();
    actor.enqueue(plan.clone()).unwrap();
    let saved = actor.saved_plan(BatchId(2)).unwrap();
    assert_eq!(saved.events()[0].at_ns, MonoTime(110));
    assert_eq!(saved.events()[0].payload, plan.events()[0].payload);
    assert!(Arc::ptr_eq(&saved.events()[0].config, &config));
    actor
        .activate(Arc::new(ConfigSnapshot::empty(ConfigGeneration(2))))
        .unwrap();
    while actor.poll(MonoTime(150)).unwrap().is_some() {}
    assert_eq!(
        actor.saved_plan(BatchId(2)).unwrap().events()[0].at_ns,
        MonoTime(110)
    );
    assert!(Arc::ptr_eq(
        &actor.saved_plan(BatchId(2)).unwrap().events()[0].config,
        &config
    ));
    assert!(actor.input_up(BatchId(2), MonoTime(160)).unwrap());
    assert_eq!(
        actor.saved_plan(BatchId(2)).unwrap().releases(),
        plan.releases()
    );
    while actor.poll(MonoTime(160)).unwrap().is_some() {}
    assert!(actor.saved_plan(BatchId(2)).is_none());
}
