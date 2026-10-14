use super::*;
pub(super) enum MixedWork {
    Keys(usize),
    Relative { at: MonoTime, frame: RelativeFrame },
}
impl<B: FrameBackend> OutputPump<B> {
    pub(super) fn clear_mixed(&mut self) {
        self.order.clear();
        self.relative_units = 0;
        self.tail_at = self.now;
    }
    pub fn queued_relative_events(&self) -> usize {
        self.relative_units
    }
    /// Native delivery deadline, including relative FIFO barriers and report flush.
    pub fn next_deadline(&self) -> Option<MonoTime> {
        match self.phase {
            Phase::ReportPending => Some(self.now),
            Phase::Recovery => self.queue.next_deadline(),
            Phase::Quarantined | Phase::Detached => None,
            Phase::Normal => match self.order.front() {
                Some(MixedWork::Keys(_)) => self.queue.next_deadline(),
                Some(MixedWork::Relative { at, .. }) => Some(*at),
                None => None,
            },
        }
    }
    /// Cold preflight of the entire step, then keys before each relative frame.
    /// Relative timestamps and keys share the same FIFO normalization ceiling.
    pub fn enqueue_mixed(
        &mut self,
        edges: &[OutputIntent],
        frames: &[(MonoTime, RelativeFrame)],
    ) -> Result<(), PumpError> {
        if self.phase != Phase::Normal {
            return Err(PumpError::NotReady);
        }
        if edges.len() > keyweave_output::MAX_ENQUEUE_EDGES
            || frames.len() > keyweave_output::MAX_QUEUED_EDGES
        {
            return Err(OutputError::Capacity.into());
        }
        let backend = self.backend.as_ref().ok_or(PumpError::NotReady)?;
        let native = self.native(edges)?;
        for chunk in native.chunks(crate::events::MAX_FRAME_EVENTS) {
            backend
                .spec()
                .validate(chunk)
                .map_err(PumpError::Rejected)?;
        }
        let mut added = 0usize;
        for (_, frame) in frames {
            if frame.records().is_empty() {
                continue;
            }
            let axes = backend.relative_axes().ok_or_else(|| {
                PumpError::Rejected(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "backend has no relative axes",
                ))
            })?;
            if frame
                .records()
                .iter()
                .any(|record| !axes.contains(record.code))
            {
                return Err(PumpError::Rejected(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unadvertised relative axis",
                )));
            }
            added = added.saturating_add(frame.records().len());
        }
        let total = self
            .queue
            .queued_edges()
            .saturating_add(self.relative_units)
            .saturating_add(edges.len())
            .saturating_add(added);
        if total > keyweave_output::MAX_QUEUED_EDGES {
            return Err(OutputError::Capacity.into());
        }
        let mut tail = self.tail_at.max(self.now);
        let normalized: Vec<_> = edges
            .iter()
            .map(|edge| {
                tail = tail.max(edge.at_ns);
                OutputIntent {
                    at_ns: tail,
                    ..edge.clone()
                }
            })
            .collect();
        self.queue.enqueue(&normalized)?;
        if !edges.is_empty() {
            if let Some(MixedWork::Keys(count)) = self.order.back_mut() {
                *count += edges.len();
            } else {
                self.order.push_back(MixedWork::Keys(edges.len()));
            }
        }
        for (at, frame) in frames
            .iter()
            .filter(|(_, frame)| !frame.records().is_empty())
        {
            tail = tail.max(*at);
            self.order.push_back(MixedWork::Relative {
                at: tail,
                frame: frame.clone(),
            });
        }
        self.relative_units += added;
        self.tail_at = tail;
        Ok(())
    }
    pub(super) fn finish_keys(&mut self, accepted: usize) -> Result<(), PumpError> {
        let Some(MixedWork::Keys(count)) = self.order.front_mut() else {
            return Err(PumpError::InvalidReceipt);
        };
        *count = count
            .checked_sub(accepted)
            .ok_or(PumpError::InvalidReceipt)?;
        if *count == 0 {
            self.order.pop_front();
        }
        Ok(())
    }
    pub(super) fn poll_relative(&mut self, now: MonoTime) -> Result<PumpStep, PumpError> {
        let Some(MixedWork::Relative { at, frame }) = self.order.front() else {
            return Err(PumpError::InvalidReceipt);
        };
        if *at > now {
            return Ok(PumpStep::Idle);
        }
        let count = frame.records().len();
        let result = self
            .backend
            .as_mut()
            .ok_or(PumpError::NotReady)?
            .submit_relative(frame);
        match result {
            Ok(Ok(FrameWrite::Complete { accepted })) if accepted == count => {
                self.order.pop_front();
                self.relative_units -= count;
                Ok(PumpStep::Submitted { accepted })
            }
            Ok(Ok(FrameWrite::Incomplete {
                accepted,
                report_pending: true,
            })) if accepted <= count => {
                self.queue.begin_recovery(now)?;
                self.clear_mixed();
                self.phase = Phase::ReportPending;
                Ok(PumpStep::ReportPending { accepted })
            }
            Ok(Ok(_)) => {
                self.quarantine();
                Err(PumpError::InvalidReceipt)
            }
            Ok(Err(WriteError::InvalidFrame(error))) => {
                self.queue.begin_recovery(now)?;
                self.clear_mixed();
                self.phase = Phase::Recovery;
                Err(WriteError::InvalidFrame(error).into())
            }
            Ok(Err(error)) => {
                self.quarantine();
                Err(error.into())
            }
            Err(error) => {
                self.queue.begin_recovery(now)?;
                self.clear_mixed();
                self.phase = Phase::Recovery;
                Err(PumpError::Rejected(error))
            }
        }
    }
}
