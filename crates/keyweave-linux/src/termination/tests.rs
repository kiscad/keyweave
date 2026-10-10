#![allow(clippy::unwrap_used)]
use super::*;
use std::{collections::VecDeque, io::Cursor};
fn report(flags: u16) -> TerminationReport {
    let bit = |i: u32| flags & (1u16 << i) != 0u16;
    TerminationReport {
        reason: StopReason::Pause,
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
    }
}
fn frame() -> TerminalFrame {
    TerminalFrame {
        run_id: 57,
        report: report(0x383),
    }
}
#[test]
fn every_consistent_mask_roundtrips_exact_flags_and_positive_run() {
    for mask in 0..=KNOWN_FLAGS {
        let f = TerminalFrame {
            run_id: u64::MAX,
            report: report(mask),
        };
        if mask & 0x80 != 0 && mask & 0x7c != 0 {
            assert_eq!(f.encode(), Err(ProtocolError::Facts));
            continue;
        }
        let bytes = f.encode().unwrap();
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), mask);
        assert_eq!(TerminalFrame::decode(&bytes, u64::MAX).unwrap(), f);
    }
}
#[test]
fn all_reason_codes_roundtrip_without_losing_independent_failures() {
    for code in 1..=10 {
        let mut f = frame();
        f.report = report(0x37f);
        f.report.reason = reason(code).unwrap();
        let b = f.encode().unwrap();
        assert_eq!(b[5], code);
        assert_eq!(TerminalFrame::decode(&b, 57).unwrap(), f);
    }
}
#[test]
fn malformed_length_magic_version_reason_reserved_and_identity_are_rejected() {
    let good = frame().encode().unwrap();
    for n in [0, 4, 31] {
        assert_eq!(
            TerminalFrame::decode(&good[..n], 57),
            Err(ProtocolError::Frame)
        );
    }
    let mut extra = good.to_vec();
    extra.push(0);
    assert_eq!(TerminalFrame::decode(&extra, 57), Err(ProtocolError::Frame));
    for index in [
        0, 1, 2, 3, 4, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
    ] {
        let mut b = good;
        b[index] ^= 0x80;
        assert_eq!(TerminalFrame::decode(&b, 57), Err(ProtocolError::Frame));
    }
    for code in [0, 11, 255] {
        let mut b = good;
        b[5] = code;
        assert_eq!(TerminalFrame::decode(&b, 57), Err(ProtocolError::Frame));
    }
    for bit in 10..16 {
        let mut b = good;
        let flags = 0x383u16 | (1 << bit);
        b[6..8].copy_from_slice(&flags.to_le_bytes());
        assert_eq!(TerminalFrame::decode(&b, 57), Err(ProtocolError::Frame));
    }
    assert_eq!(
        TerminalFrame::decode(&good, 58),
        Err(ProtocolError::Identity)
    );
    let mut f = frame();
    f.run_id = 0;
    assert_eq!(f.encode(), Err(ProtocolError::Identity));
}
#[test]
fn decoder_rejects_recovery_confirmation_that_hides_uncertainty() {
    let good = frame().encode().unwrap();
    for bit in 2..7 {
        let mut b = good;
        b[6] |= 1 << bit;
        assert_eq!(TerminalFrame::decode(&b, 57), Err(ProtocolError::Facts));
    }
}
struct ScriptWriter {
    steps: VecDeque<io::Result<usize>>,
    bytes: Vec<u8>,
    calls: usize,
}
impl Write for ScriptWriter {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        let n = self.steps.pop_front().unwrap_or(Ok(b.len()))?;
        self.bytes.extend_from_slice(&b[..n.min(b.len())]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("stdio flush must not be used")
    }
}
#[test]
fn writer_retains_one_frame_across_partial_and_would_block_and_no_io_after_complete() {
    let source = ScriptWriter {
        steps: VecDeque::from([Ok(3), Err(io::ErrorKind::WouldBlock.into()), Ok(7), Ok(22)]),
        bytes: vec![],
        calls: 0,
    };
    let mut w = TerminalWriter::new(source, 57).unwrap();
    assert!(matches!(w.flush(), Err(SendError::NotPrepared)));
    w.prepare(frame().report).unwrap();
    for _ in 0..3 {
        assert!(!w.flush().unwrap());
    }
    assert!(w.flush().unwrap());
    assert!(w.flush().unwrap());
    assert_eq!(w.writer.calls, 4);
    assert_eq!(w.writer.bytes, frame().encode().unwrap());
    assert!(matches!(
        w.prepare(report(0)),
        Err(SendError::AlreadyPrepared)
    ));
}
#[test]
fn fatal_writer_failure_does_not_retry_uncertain_bytes() {
    for step in [Ok(0), Ok(33), Err(io::ErrorKind::BrokenPipe.into())] {
        let source = ScriptWriter {
            steps: VecDeque::from([step]),
            bytes: vec![],
            calls: 0,
        };
        let mut w = TerminalWriter::new(source, 57).unwrap();
        w.prepare(frame().report).unwrap();
        assert!(w.flush().is_err());
        assert!(matches!(w.flush(), Err(SendError::Failed)));
        assert_eq!(w.writer.calls, 1);
    }
}
struct ScriptReader {
    bytes: Cursor<Vec<u8>>,
    sizes: VecDeque<io::Result<usize>>,
    calls: usize,
}
impl Read for ScriptReader {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        let n = self.sizes.pop_front().unwrap_or(Ok(b.len()))?;
        let len = n.min(b.len());
        self.bytes.read(&mut b[..len])
    }
}
#[test]
fn reader_requires_eof_and_bounds_each_partial_read() {
    let source = ScriptReader {
        bytes: Cursor::new(frame().encode().unwrap().to_vec()),
        sizes: VecDeque::from([Ok(3), Err(io::ErrorKind::WouldBlock.into()), Ok(29), Ok(1)]),
        calls: 0,
    };
    let mut r = TerminalReader::new(source, 57).unwrap();
    for _ in 0..3 {
        assert_eq!(r.poll(), ReadStatus::Pending);
    }
    assert_eq!(r.poll(), ReadStatus::Complete(frame()));
    assert_eq!(r.poll(), ReadStatus::Complete(frame()));
    assert_eq!(r.reader.calls, 4);
}
#[test]
fn eof_short_extra_wrong_run_and_io_failures_are_terminal_without_more_reads() {
    let good = frame().encode().unwrap();
    for (bytes, id, status) in [
        (vec![], 57, ReadStatus::Missing),
        (good[..31].to_vec(), 57, ReadStatus::Missing),
        (
            good.into_iter().chain([0]).collect(),
            57,
            ReadStatus::Invalid(ProtocolError::Frame),
        ),
        (
            good.to_vec(),
            58,
            ReadStatus::Invalid(ProtocolError::Identity),
        ),
    ] {
        let mut r = TerminalReader::new(Cursor::new(bytes), id).unwrap();
        let first = r.poll();
        let status_actual = if first == ReadStatus::Pending {
            r.poll()
        } else {
            first
        };
        assert_eq!(status_actual, status);
        assert_eq!(r.poll(), status);
    }
    let source = ScriptReader {
        bytes: Cursor::new(vec![]),
        sizes: VecDeque::from([Err(io::ErrorKind::PermissionDenied.into())]),
        calls: 0,
    };
    let mut r = TerminalReader::new(source, 57).unwrap();
    assert_eq!(r.poll(), ReadStatus::Io(io::ErrorKind::PermissionDenied));
    assert_eq!(r.poll(), ReadStatus::Io(io::ErrorKind::PermissionDenied));
    assert_eq!(r.reader.calls, 1);
}
