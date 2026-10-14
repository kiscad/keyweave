//! Shared single-writer state machine; construction is not device/readiness proof.
use super::*;
use crate::relative::RelativeFrame;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputState {
    Ready,
    ReportPending,
    Recovery,
    Quarantined,
}
pub struct OutputChannel<W> {
    writer: W,
    layout: Layout,
    spec: DeviceSpec,
    state: OutputState,
}
impl<W: Write> OutputChannel<W> {
    pub fn new(writer: W, layout: Layout, spec: DeviceSpec) -> Self {
        Self {
            writer,
            layout,
            spec,
            state: OutputState::Ready,
        }
    }
    pub fn spec(&self) -> &DeviceSpec {
        &self.spec
    }
    pub fn state(&self) -> OutputState {
        self.state
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn writer(&self) -> &W {
        &self.writer
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }
    fn observe(&mut self, result: &Result<FrameWrite, WriteError>) {
        match result {
            Ok(FrameWrite::Incomplete { .. }) => self.state = OutputState::ReportPending,
            Err(WriteError::UnknownAcceptance(_)) => self.state = OutputState::Quarantined,
            _ => {}
        }
    }
    pub fn emit(&mut self, edges: &[NativeKey]) -> io::Result<Result<FrameWrite, WriteError>> {
        if self.state != OutputState::Ready {
            return Err(io::Error::other("normal output is paused"));
        }
        self.write_keys(edges)
    }
    pub fn emit_relative(
        &mut self,
        frame: &RelativeFrame,
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        if self.state != OutputState::Ready {
            return Err(io::Error::other("normal output is paused"));
        }
        self.spec.validate_relative(frame)?;
        let result = frame.write_once(&mut self.writer, self.layout);
        self.observe(&result);
        Ok(result)
    }
    pub fn release_submitted(
        &mut self,
        edges: &[NativeKey],
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        if !matches!(self.state, OutputState::Ready | OutputState::Recovery)
            || edges.iter().any(|edge| edge.value != 0)
        {
            return Err(io::Error::other("invalid output recovery"));
        }
        self.spec.validate(edges)?;
        self.state = OutputState::Recovery;
        self.write_keys(edges)
    }
    fn write_keys(&mut self, edges: &[NativeKey]) -> io::Result<Result<FrameWrite, WriteError>> {
        self.spec.validate(edges)?;
        let result = write_frame(&mut self.writer, self.layout, edges);
        self.observe(&result);
        Ok(result)
    }
    pub fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        if self.state != OutputState::ReportPending {
            return Err(io::Error::other("no known pending report"));
        }
        let result = super::flush_report(&mut self.writer, self.layout);
        match &result {
            Ok(true) => self.state = OutputState::Recovery,
            Err(WriteError::UnknownAcceptance(_)) => self.state = OutputState::Quarantined,
            _ => {}
        }
        Ok(result)
    }
}
