//! Deterministic virtual-clock driver; never captures or submits native input.
use super::*;
use keyweave_engine::{
    Engine,
    physical_controller::{PhysicalStageController, PhysicalStep},
    pointer_work::PointerMotion,
};
const MAX_ITEMS: usize = 100_000;
const MAX_WORK: usize = 1_000_000;
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Step {
    Input {
        input: InputEnvelope,
    },
    Advance {
        at_ns: MonoTime,
    },
    Activate {
        at_ns: MonoTime,
        generation: ConfigGeneration,
        config: Value,
    },
}
pub(super) struct ReplayResult {
    pub outputs: Vec<OutputIntent>,
    pub pointers: Vec<Value>,
    pub repeat_targets: Vec<Value>,
    pub state: EngineState,
    pub variables: std::collections::BTreeMap<String, VariableValue>,
    pub next_deadline: Value,
}
fn compiled(
    config: &Value,
    generation: ConfigGeneration,
    mode: ExecutionMode,
) -> Result<Arc<keyweave_engine::ConfigSnapshot>> {
    let result = compile_with_mode(
        &serde_json::to_string(config)?,
        &BackendCapabilities::offline(),
        generation,
        mode.compile_mode(),
    )?;
    ensure_lossless(&result)?;
    Ok(Arc::new(result.snapshot))
}
struct Trace {
    outputs: Vec<OutputIntent>,
    pointers: Vec<Value>,
    repeats: Vec<Value>,
    work: usize,
}
impl Trace {
    fn charge(&mut self) -> Result<()> {
        self.work += 1;
        if self.work > MAX_WORK {
            return Err("replay work limit exceeded".into());
        }
        Ok(())
    }
    fn collect(&mut self, step: PhysicalStep) -> Result<()> {
        if step.outputs.len() > MAX_ITEMS.saturating_sub(self.outputs.len())
            || step.forwarded_pointers.len() > MAX_ITEMS.saturating_sub(self.pointers.len())
        {
            return Err("replay output limit exceeded".into());
        }
        self.outputs.extend(step.outputs);
        for pointer in step.forwarded_pointers {
            let motion = match pointer.event.payload {
                PointerMotion::Move { x, y } => json!({"type":"move", "x":x,"y":y}),
                PointerMotion::Wheel {
                    horizontal,
                    vertical,
                } => json!({"type":"wheel", "horizontal":horizontal,"vertical":vertical}),
            };
            self.pointers.push(json!({"at_ns":pointer.delivered_at,"original_at_ns":pointer.event.at_ns,"device":pointer.event.device,"motion":motion,"after_keyboard_outputs":self.outputs.len()}));
        }
        Ok(())
    }
    fn drain(&mut self, actor: &mut PhysicalStageController, at: MonoTime) -> Result<()> {
        while let Some(due) = actor.next_deadline().filter(|due| *due <= at) {
            self.charge()?;
            let step = actor
                .poll(due)?
                .ok_or("staged replay stalled at a due deadline")?;
            self.collect(step)?;
        }
        // Advance actual actor clock even when no timer work exists.
        self.charge()?;
        if let Some(step) = actor.poll(at)? {
            self.collect(step)?;
        }
        Ok(())
    }
}
pub(super) fn run(fixture: &Fixture, mode: ExecutionMode) -> Result<ReplayResult> {
    match fixture.schema_version {
        1 if !fixture.steps.is_empty() => return Err("schema 1 cannot contain steps".into()),
        2 if mode != ExecutionMode::StagedKeyboard || !fixture.inputs.is_empty() => {
            return Err("schema 2 requires staged-keyboard and steps only".into());
        }
        1 | 2 => {}
        _ => return Err("replay schema_version must be 1 or 2".into()),
    }
    if fixture.inputs.len().saturating_add(fixture.steps.len()) > MAX_ITEMS {
        return Err("replay input limit exceeded".into());
    }
    let config = compiled(&fixture.config, ConfigGeneration(1), mode)?;
    let mut trace = Trace {
        outputs: vec![],
        pointers: vec![],
        repeats: vec![],
        work: 0,
    };
    if mode == ExecutionMode::Legacy {
        let mut actor = Engine::new(config)?;
        for input in &fixture.inputs {
            let step = actor.handle(input.clone())?;
            if step.outputs.len() > MAX_ITEMS.saturating_sub(trace.outputs.len()) {
                return Err("replay output limit exceeded".into());
            }
            trace.outputs.extend(step.outputs);
        }
        return Ok(ReplayResult {
            outputs: trace.outputs,
            pointers: trace.pointers,
            repeat_targets: trace.repeats,
            state: actor.state(),
            variables: actor.variable_values(),
            next_deadline: actor
                .next_deadline()
                .map_or(Value::Null, |(at, token)| json!({"at_ns":at,"token":token})),
        });
    }
    let mut actor = PhysicalStageController::new_with_chord_model(config)?;
    let mut now = MonoTime(0);
    let input = |actor: &mut PhysicalStageController,
                 trace: &mut Trace,
                 now: &mut MonoTime,
                 event: &InputEnvelope|
     -> Result<()> {
        if event.at_ns < *now {
            return Err("replay time moved backwards".into());
        }
        if !matches!(
            event.event,
            InputEvent::Reset | InputEvent::DeviceRemoved | InputEvent::ContextChanged { .. }
        ) {
            trace.drain(actor, event.at_ns)?;
        }
        trace.charge()?;
        let step = actor.handle(event.clone())?;
        if let Some(key) = step.repeat_target {
            trace
                .repeats
                .push(json!({"seq":event.seq,"at_ns":event.at_ns,"key":key}));
        }
        trace.collect(step)?;
        trace.drain(actor, event.at_ns)?;
        *now = event.at_ns;
        Ok(())
    };
    for event in &fixture.inputs {
        input(&mut actor, &mut trace, &mut now, event)?;
    }
    for step in &fixture.steps {
        match step {
            Step::Input { input: event } => input(&mut actor, &mut trace, &mut now, event)?,
            Step::Advance { at_ns } | Step::Activate { at_ns, .. } => {
                if *at_ns < now {
                    return Err("replay time moved backwards".into());
                }
                trace.drain(&mut actor, *at_ns)?;
                now = *at_ns;
                if let Step::Activate {
                    generation, config, ..
                } = step
                {
                    actor.activate(compiled(config, *generation, mode)?)?;
                }
            }
        }
    }
    Ok(ReplayResult {
        outputs: trace.outputs,
        pointers: trace.pointers,
        repeat_targets: trace.repeats,
        state: actor.state(),
        variables: actor.output_variable_values(),
        next_deadline: actor
            .next_deadline()
            .map_or(Value::Null, |at| json!({"at_ns":at})),
    })
}
