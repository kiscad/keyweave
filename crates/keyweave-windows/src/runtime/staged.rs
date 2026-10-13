use super::*;
pub(super) struct PendingInput {
    pub(super) at: MonoTime,
    event: InputEvent,
}
impl<B: SendBackend> SessionActor<B> {
    /// Queue occupancy only, not capture or output acceptance proof.
    pub fn queued_inputs(&self) -> usize {
        self.pending_inputs.len()
    }
    pub(super) fn enqueue_input(
        &mut self,
        event: InputEvent,
        at: MonoTime,
    ) -> Result<(), RuntimeError> {
        if self.pending_inputs.len() >= MAX_PENDING_INPUTS {
            return Err(RuntimeError::Budget);
        }
        self.pending_inputs.push_back(PendingInput { at, event });
        Ok(())
    }
    pub(super) fn enqueue_context(
        &mut self,
        context: ContextSnapshot,
        at: MonoTime,
    ) -> Result<(), RuntimeError> {
        let revision = self
            .pending_inputs
            .iter()
            .filter_map(|input| match &input.event {
                InputEvent::ContextChanged { context } => Some(context.revision),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        self.engine
            .validate_staged_context(&context, revision, at)?;
        self.enqueue_input(InputEvent::ContextChanged { context }, at)
    }
    pub(super) fn deliver_pending_input(&mut self, work: &mut usize) -> Result<(), RuntimeError> {
        let input = self
            .pending_inputs
            .pop_front()
            .ok_or(RuntimeError::Decision)?;
        let repeat = match input.event {
            InputEvent::Key {
                key,
                edge: KeyEdge::Repeat,
            } => Some(key),
            _ => None,
        };
        self.dispatch_at(input.event, input.at, work)?;
        if let Some(source) = repeat
            && let Some(key) = self.engine.physical_repeat_target(STREAM, source)
            && self.output.ledger().desired().contains(&key)
            && self.output.ledger().submitted().contains(&key)
        {
            self.output.enqueue(&[OutputIntent {
                at_ns: self.now,
                key,
                edge: KeyEdge::Repeat,
            }])?;
        }
        Ok(())
    }
}
