use super::*;
use keyweave_engine::{
    EngineStep,
    physical_controller::{PhysicalStageController, PhysicalStep},
};
pub(super) enum Machine {
    Legacy(Box<Engine>),
    Staged(Box<PhysicalStageController>),
}
fn keyboard_step(step: PhysicalStep) -> Result<EngineStep, RuntimeError> {
    if !step.forwarded_pointers.is_empty() {
        return Err(RuntimeError::StagedPointer);
    }
    Ok(EngineStep {
        capture: step.capture,
        outputs: step.outputs,
        next_deadline: step.next_deadline,
    })
}
impl Machine {
    pub(super) fn execution_mode(&self) -> ExecutionMode {
        match self {
            Self::Legacy(_) => ExecutionMode::Legacy,
            Self::Staged(_) => ExecutionMode::StagedKeyboard,
        }
    }

    pub(super) fn staged(
        config: Arc<ConfigSnapshot>,
        devices: &[DeviceInstanceId],
    ) -> Result<Self, RuntimeError> {
        let mut controller = PhysicalStageController::new_with_chord_model(config)?;
        for &device in devices {
            controller.observe_device(device, false, MonoTime(0))?;
        }
        Ok(Self::Staged(Box::new(controller)))
    }
    pub(super) fn next_deadline(&self) -> Option<MonoTime> {
        match self {
            Self::Legacy(engine) => engine.next_deadline().map(|(at, _)| at),
            Self::Staged(controller) => controller.next_deadline(),
        }
    }
    pub(super) fn stage_due(&self, at: MonoTime) -> bool {
        matches!(self, Self::Staged(controller) if controller.next_deadline().is_some_and(|due| due <= at))
    }
    pub(super) fn poll_stage(&mut self, at: MonoTime) -> Result<Option<EngineStep>, RuntimeError> {
        match self {
            Self::Staged(controller) => controller.poll(at)?.map(keyboard_step).transpose(),
            Self::Legacy(_) => Ok(None),
        }
    }
    pub(super) fn legacy_timer_due(&self, at: MonoTime) -> Option<DeadlineToken> {
        match self {
            Self::Legacy(engine) => engine
                .next_deadline()
                .filter(|(due, _)| *due <= at)
                .map(|(_, token)| token),
            Self::Staged(_) => None,
        }
    }
    pub(super) fn next_delayed(&self) -> Option<DeadlineToken> {
        match self {
            Self::Legacy(engine) => engine.next_delayed(),
            Self::Staged(_) => None,
        }
    }
    pub(super) fn config_generation(&self) -> ConfigGeneration {
        match self {
            Self::Legacy(engine) => engine.config_generation(),
            Self::Staged(controller) => controller.snapshot().generation,
        }
    }
    pub(super) fn state(&self) -> EngineState {
        match self {
            Self::Legacy(engine) => engine.state(),
            Self::Staged(controller) => controller.state(),
        }
    }
    pub(super) fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
        match self {
            Self::Legacy(engine) => engine.activate(config)?,
            Self::Staged(controller) => controller.activate(config)?,
        }
        Ok(())
    }
    pub(super) fn handle(&mut self, input: InputEnvelope) -> Result<EngineStep, RuntimeError> {
        match self {
            Self::Legacy(engine) => Ok(engine.handle(input)?),
            Self::Staged(controller) => {
                if matches!(
                    input.event,
                    InputEvent::Wheel { .. } | InputEvent::PointerMove { .. }
                ) {
                    return Err(RuntimeError::StagedPointer);
                }
                keyboard_step(controller.handle(input)?)
            }
        }
    }
    pub(super) fn reset(&mut self, at: MonoTime) -> Result<Vec<OutputIntent>, RuntimeError> {
        match self {
            Self::Legacy(engine) => Ok(engine.reset(at)?),
            Self::Staged(controller) => Ok(controller.reset(at)?.outputs),
        }
    }
    #[cfg(test)]
    pub(super) fn context_revision(&self) -> u64 {
        match self {
            Self::Legacy(engine) => engine.context_revision(),
            Self::Staged(controller) => controller.context_revision(),
        }
    }
}
