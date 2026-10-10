//! Fixed terminal evidence, independent of progress leases and actual process exit.
use crate::runtime::{StopReason, TerminationReport};
use std::io::{self, Read, Write};
pub const FRAME_BYTES: usize = 32;
const KNOWN_FLAGS: u16 = 0x03ff;
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid terminal frame")]
    Frame,
    #[error("terminal run identity mismatch")]
    Identity,
    #[error("inconsistent terminal recovery facts")]
    Facts,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalFrame {
    pub run_id: u64,
    pub report: TerminationReport,
}
fn reason_code(reason: StopReason) -> u8 {
    match reason {
        StopReason::Pause => 1,
        StopReason::Emergency => 2,
        StopReason::InputFault => 3,
        StopReason::InputLost => 4,
        StopReason::EngineFault => 5,
        StopReason::OutputFault => 6,
        StopReason::ClockFault => 7,
        StopReason::ControlFault => 8,
        StopReason::NoDevices => 9,
        StopReason::EmergencyUnavailable => 10,
    }
}
fn reason(code: u8) -> Result<StopReason, ProtocolError> {
    Ok(match code {
        1 => StopReason::Pause,
        2 => StopReason::Emergency,
        3 => StopReason::InputFault,
        4 => StopReason::InputLost,
        5 => StopReason::EngineFault,
        6 => StopReason::OutputFault,
        7 => StopReason::ClockFault,
        8 => StopReason::ControlFault,
        9 => StopReason::NoDevices,
        10 => StopReason::EmergencyUnavailable,
        _ => return Err(ProtocolError::Frame),
    })
}
fn validate(frame: TerminalFrame) -> Result<(), ProtocolError> {
    if frame.run_id == 0 {
        return Err(ProtocolError::Identity);
    }
    let r = frame.report;
    if r.output_recovery_confirmed
        && (r.recovery_failed
            || r.recovery_budget_exhausted
            || r.submitted_remaining
            || r.report_pending
            || r.acceptance_unknown)
    {
        return Err(ProtocolError::Facts);
    }
    Ok(())
}
impl TerminalFrame {
    pub fn encode(self) -> Result<[u8; FRAME_BYTES], ProtocolError> {
        validate(self)?;
        let mut bytes = [0; FRAME_BYTES];
        bytes[..4].copy_from_slice(b"KWLT");
        bytes[4] = 1;
        bytes[5] = reason_code(self.report.reason);
        let r = self.report;
        let flags = [
            r.input_owners_released,
            r.engine_reset_confirmed,
            r.recovery_failed,
            r.recovery_budget_exhausted,
            r.submitted_remaining,
            r.report_pending,
            r.acceptance_unknown,
            r.output_recovery_confirmed,
            r.output_owner_dropped,
            r.backend_transition_confirmed,
        ]
        .into_iter()
        .enumerate()
        .fold(0u16, |bits, (i, set)| bits | (u16::from(set) << i));
        bytes[6..8].copy_from_slice(&flags.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.run_id.to_le_bytes());
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], expected_run: u64) -> Result<Self, ProtocolError> {
        if bytes.len() != FRAME_BYTES
            || &bytes[..4] != b"KWLT"
            || bytes[4] != 1
            || bytes[16..].iter().any(|b| *b != 0)
        {
            return Err(ProtocolError::Frame);
        }
        let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
        if flags & !KNOWN_FLAGS != 0 {
            return Err(ProtocolError::Frame);
        }
        let mut id = [0; 8];
        id.copy_from_slice(&bytes[8..16]);
        let run_id = u64::from_le_bytes(id);
        if run_id != expected_run {
            return Err(ProtocolError::Identity);
        }
        let bit = |i: u32| flags & (1u16 << i) != 0u16;
        let frame = Self {
            run_id,
            report: TerminationReport {
                reason: reason(bytes[5])?,
                input_owners_released: bit(0),
                engine_reset_confirmed: bit(1),
                recovery_failed: bit(2),
                recovery_budget_exhausted: bit(3),
                submitted_remaining: bit(4),
                report_pending: bit(5),
                acceptance_unknown: bit(6),
                output_recovery_confirmed: bit(7),
                output_owner_dropped: bit(8),
                backend_transition_confirmed: bit(9),
            },
        };
        validate(frame)?;
        Ok(frame)
    }
}
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("terminal frame already prepared")]
    AlreadyPrepared,
    #[error("terminal frame not prepared")]
    NotPrepared,
    #[error("terminal channel failed; retry prohibited")]
    Failed,
    #[error(transparent)]
    Io(#[from] io::Error),
}
/// Own one fixed frame. Completion only acknowledges writes, not process exit.
pub struct TerminalWriter<W: Write> {
    writer: W,
    run_id: u64,
    frame: Option<[u8; FRAME_BYTES]>,
    written: usize,
    failed: bool,
}
impl<W: Write> TerminalWriter<W> {
    pub fn new(writer: W, run_id: u64) -> Result<Self, ProtocolError> {
        if run_id == 0 {
            return Err(ProtocolError::Identity);
        }
        Ok(Self {
            writer,
            run_id,
            frame: None,
            written: 0,
            failed: false,
        })
    }
    pub fn prepare(&mut self, report: TerminationReport) -> Result<(), SendError> {
        if self.frame.is_some() {
            return Err(SendError::AlreadyPrepared);
        }
        self.frame = Some(
            TerminalFrame {
                run_id: self.run_id,
                report,
            }
            .encode()?,
        );
        Ok(())
    }
    /// At most one nonblocking write. Caller provides finite scheduling/deadline.
    pub fn flush(&mut self) -> Result<bool, SendError> {
        if self.failed {
            return Err(SendError::Failed);
        }
        let frame = self.frame.as_ref().ok_or(SendError::NotPrepared)?;
        if self.written == FRAME_BYTES {
            return Ok(true);
        }
        match self.writer.write(&frame[self.written..]) {
            Ok(0) => {
                self.failed = true;
                return Err(io::ErrorKind::WriteZero.into());
            }
            Ok(n) if n > FRAME_BYTES - self.written => {
                self.failed = true;
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(n) => self.written += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
            Err(e) => {
                self.failed = true;
                return Err(e.into());
            }
        }
        Ok(self.written == FRAME_BYTES)
    }
}
impl From<io::ErrorKind> for SendError {
    fn from(kind: io::ErrorKind) -> Self {
        Self::Io(kind.into())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadStatus {
    Unsupported,
    NotExited,
    Pending,
    Complete(TerminalFrame),
    Missing,
    Invalid(ProtocolError),
    Io(io::ErrorKind),
}
pub struct TerminalReader<R: Read> {
    reader: R,
    run_id: u64,
    bytes: [u8; FRAME_BYTES + 1],
    read: usize,
    status: ReadStatus,
}
impl<R: Read> TerminalReader<R> {
    pub fn new(reader: R, run_id: u64) -> Result<Self, ProtocolError> {
        if run_id == 0 {
            return Err(ProtocolError::Identity);
        }
        Ok(Self {
            reader,
            run_id,
            bytes: [0; FRAME_BYTES + 1],
            read: 0,
            status: ReadStatus::Pending,
        })
    }
    /// Owner calls only after actual child exit. Complete requires EOF; callers
    /// must bound observation of a stray writer that still holds the pipe open.
    pub fn poll(&mut self) -> ReadStatus {
        if self.status != ReadStatus::Pending {
            return self.status;
        }
        match self.reader.read(&mut self.bytes[self.read..]) {
            Ok(0) => {
                self.status = if self.read != FRAME_BYTES {
                    ReadStatus::Missing
                } else {
                    match TerminalFrame::decode(&self.bytes[..FRAME_BYTES], self.run_id) {
                        Ok(frame) => ReadStatus::Complete(frame),
                        Err(e) => ReadStatus::Invalid(e),
                    }
                }
            }
            Ok(n) if n > FRAME_BYTES + 1 - self.read => {
                self.status = ReadStatus::Invalid(ProtocolError::Frame)
            }
            Ok(n) => {
                self.read += n;
                if self.read > FRAME_BYTES {
                    self.status = ReadStatus::Invalid(ProtocolError::Frame);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => (),
            Err(e) => self.status = ReadStatus::Io(e.kind()),
        }
        self.status
    }
}
#[cfg(test)]
mod tests;
