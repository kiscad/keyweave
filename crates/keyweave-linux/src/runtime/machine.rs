use super::*;
use keyweave_engine::{
    EngineStep,
    physical_controller::{PhysicalStageController, PhysicalStep},
};
pub(super) enum Machine {
    Legacy(Box<Engine>),
    Staged(Box<PhysicalStageController>, bool),
}
pub(super) struct NativeStep {
    pub(super) capture: CaptureDecision,
    pub(super) outputs: Vec<OutputIntent>,
    pub(super) registered_pointer: Option<SourceToken>,
    pub(super) pointers:
        Vec<keyweave_engine::timeline::DueEvent<keyweave_engine::pointer_work::PointerMotion>>,
}
fn stage_step(step: PhysicalStep, relative: bool) -> Result<NativeStep, RuntimeError> {
    if !relative && (!step.forwarded_pointers.is_empty() || step.registered_pointer.is_some()) {
        return Err(RuntimeError::StagedPointer);
    }
    Ok(NativeStep {
        capture: step.capture,
        outputs: step.outputs,
        registered_pointer: step.registered_pointer,
        pointers: step.forwarded_pointers,
    })
}
fn legacy_step(step: EngineStep) -> NativeStep {
    NativeStep {
        capture: step.capture,
        outputs: step.outputs,
        registered_pointer: None,
        pointers: vec![],
    }
}
impl Machine {
    pub(super) fn relative_enabled(&self) -> bool {
        matches!(self, Self::Staged(_, true))
    }
    pub(super) fn relative_config(&self) -> Result<Arc<ConfigSnapshot>, RuntimeError> {
        match self {
            Self::Staged(controller, true) => Ok(controller.snapshot().clone()),
            _ => Err(RuntimeError::StagedPointer),
        }
    }
    pub(super) fn execution_mode(&self) -> ExecutionMode {
        match self {
            Self::Legacy(_) => ExecutionMode::Legacy,
            Self::Staged(..) => ExecutionMode::StagedKeyboard,
        }
    }

    pub(super) fn staged(
        config: Arc<ConfigSnapshot>,
        devices: &[DeviceInstanceId],
    ) -> Result<Self, RuntimeError> {
        Self::staged_with_relative(config, devices, false)
    }
    pub(super) fn staged_with_relative(
        config: Arc<ConfigSnapshot>,
        devices: &[DeviceInstanceId],
        relative: bool,
    ) -> Result<Self, RuntimeError> {
        let mut controller = PhysicalStageController::new_with_chord_model(config)?;
        for &device in devices {
            controller.observe_device(device, false, MonoTime(0))?;
        }
        Ok(Self::Staged(Box::new(controller), relative))
    }
    pub(super) fn next_deadline(&self) -> Option<MonoTime> {
        match self {
            Self::Legacy(engine) => engine.next_deadline().map(|(at, _)| at),
            Self::Staged(controller, _) => controller.next_deadline(),
        }
    }
    pub(super) fn stage_due(&self, at: MonoTime) -> bool {
        matches!(self, Self::Staged(controller, _) if controller.next_deadline().is_some_and(|due| due <= at))
    }
    pub(super) fn poll_stage(&mut self, at: MonoTime) -> Result<Option<NativeStep>, RuntimeError> {
        match self {
            Self::Staged(controller, relative) => controller
                .poll(at)?
                .map(|step| stage_step(step, *relative))
                .transpose(),
            Self::Legacy(_) => Ok(None),
        }
    }
    pub(super) fn legacy_timer_due(&self, at: MonoTime) -> Option<DeadlineToken> {
        match self {
            Self::Legacy(engine) => engine
                .next_deadline()
                .filter(|(due, _)| *due <= at)
                .map(|(_, token)| token),
            Self::Staged(..) => None,
        }
    }
    pub(super) fn next_delayed(&self) -> Option<DeadlineToken> {
        match self {
            Self::Legacy(engine) => engine.next_delayed(),
            Self::Staged(..) => None,
        }
    }
    pub(super) fn config_generation(&self) -> ConfigGeneration {
        match self {
            Self::Legacy(engine) => engine.config_generation(),
            Self::Staged(controller, _) => controller.snapshot().generation,
        }
    }
    pub(super) fn state(&self) -> EngineState {
        match self {
            Self::Legacy(engine) => engine.state(),
            Self::Staged(controller, _) => controller.state(),
        }
    }
    pub(super) fn activate(&mut self, config: Arc<ConfigSnapshot>) -> Result<(), RuntimeError> {
        match self {
            Self::Legacy(engine) => engine.activate(config)?,
            Self::Staged(controller, _) => controller.activate(config)?,
        }
        Ok(())
    }
    pub(super) fn handle(&mut self, input: InputEnvelope) -> Result<NativeStep, RuntimeError> {
        match self {
            Self::Legacy(engine) => Ok(legacy_step(engine.handle(input)?)),
            Self::Staged(controller, relative) => {
                if !*relative
                    && matches!(
                        input.event,
                        InputEvent::Wheel { .. }
                            | InputEvent::PointerMove { .. }
                            | InputEvent::PointerActivity { .. }
                    )
                {
                    return Err(RuntimeError::StagedPointer);
                }
                stage_step(controller.handle(input)?, *relative)
            }
        }
    }
    pub(super) fn reset(&mut self, at: MonoTime) -> Result<Vec<OutputIntent>, RuntimeError> {
        match self {
            Self::Legacy(engine) => Ok(engine.reset(at)?),
            Self::Staged(controller, _) => Ok(controller.reset(at)?.outputs),
        }
    }
    #[cfg(test)]
    pub(super) fn context_revision(&self) -> u64 {
        match self {
            Self::Legacy(engine) => engine.context_revision(),
            Self::Staged(controller, _) => controller.context_revision(),
        }
    }
}
