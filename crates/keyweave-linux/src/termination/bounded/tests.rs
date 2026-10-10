#![allow(clippy::unwrap_used)]
use super::*;
use crate::runtime::StopReason;
use crate::termination::{ProtocolError, TerminalFrame};
use std::{collections::VecDeque, io::Cursor};
fn facts() -> TerminationReport {
    TerminationReport {
        reason: StopReason::Pause,
        input_owners_released: true,
        engine_reset_confirmed: true,
        recovery_failed: false,
        recovery_budget_exhausted: false,
        submitted_remaining: false,
        report_pending: false,
        acceptance_unknown: false,
        output_recovery_confirmed: true,
        output_owner_dropped: true,
        backend_transition_confirmed: true,
    }
}
fn frame() -> Vec<u8> {
    TerminalFrame {
        run_id: 7,
        report: facts(),
    }
    .encode()
    .unwrap()
    .to_vec()
}
#[derive(Default)]
struct Schedule {
    times: VecDeque<io::Result<MonoTime>>,
    waits: VecDeque<io::Result<(MonoTime, bool)>>,
    now_calls: usize,
    wait_calls: usize,
}
impl Schedule {
    fn time(&mut self) -> io::Result<MonoTime> {
        self.now_calls += 1;
        self.times.pop_front().unwrap_or(Ok(MonoTime(0)))
    }
    fn step(&mut self) -> io::Result<(MonoTime, bool)> {
        self.wait_calls += 1;
        self.waits.pop_front().unwrap_or(Ok((MonoTime(0), false)))
    }
}
impl<W: Write> SendSchedule<W> for Schedule {
    fn now(&mut self) -> io::Result<MonoTime> {
        self.time()
    }
    fn wait(&mut self, _: &TerminalWriter<W>, _: MonoTime) -> io::Result<(MonoTime, bool)> {
        self.step()
    }
}
impl<R: Read> ReadSchedule<R> for Schedule {
    fn now(&mut self) -> io::Result<MonoTime> {
        self.time()
    }
    fn wait(&mut self, _: &TerminalReader<R>, _: MonoTime) -> io::Result<(MonoTime, bool)> {
        self.step()
    }
}
struct Writer {
    bytes: Vec<u8>,
    block: bool,
    calls: usize,
}
impl Write for Writer {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.block {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.bytes.push(b[0]);
        Ok(1)
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("no stdio flush")
    }
}
#[test]
fn sender_preserves_partial_frame_and_finishes_within_attempt_budget() {
    let mut w = TerminalWriter::new(
        Writer {
            bytes: vec![],
            block: false,
            calls: 0,
        },
        7,
    )
    .unwrap();
    let mut s = Schedule::default();
    let r = send_bounded(&mut w, Some(facts()), &mut s);
    assert!(r.frame_sent && r.failure.is_none());
    assert_eq!(r.attempts, 32);
    assert_eq!(s.wait_calls, 31);
    assert_eq!(w.writer.bytes, frame());
    assert_eq!(w.writer.calls, 32);
}
#[test]
fn would_block_sender_has_finite_attempts_even_when_clock_does_not_advance() {
    let mut w = TerminalWriter::new(
        Writer {
            bytes: vec![],
            block: true,
            calls: 0,
        },
        7,
    )
    .unwrap();
    let mut s = Schedule::default();
    let r = send_bounded(&mut w, Some(facts()), &mut s);
    assert!(!r.frame_sent);
    assert!(matches!(
        r.failure,
        Some(DeliveryFailure::Schedule(ScheduleFailure::AttemptBudget))
    ));
    assert_eq!(r.attempts, MAX_ATTEMPTS);
    assert_eq!(s.wait_calls, MAX_ATTEMPTS - 1);
    assert_eq!(w.writer.calls, MAX_ATTEMPTS);
}
#[test]
fn no_report_or_invalid_initial_clock_performs_no_write() {
    let mut w = TerminalWriter::new(Vec::new(), 7).unwrap();
    let mut s = Schedule::default();
    let r = send_bounded(&mut w, None, &mut s);
    assert!(matches!(r.failure, Some(DeliveryFailure::MissingReport)));
    assert_eq!(s.now_calls, 0);
    assert!(w.writer.is_empty());
    for times in [
        vec![Ok(MonoTime(u64::MAX))],
        vec![Err(io::ErrorKind::PermissionDenied.into())],
        vec![Ok(MonoTime(2)), Ok(MonoTime(1))],
        vec![Ok(MonoTime(0)), Ok(MonoTime(FINALIZE_NS))],
    ] {
        let mut w = TerminalWriter::new(Vec::new(), 7).unwrap();
        let mut s = Schedule {
            times: times.into(),
            ..Default::default()
        };
        let r = send_bounded(&mut w, Some(facts()), &mut s);
        assert!(r.failure.is_some());
        assert_eq!(r.attempts, 0);
        assert!(w.writer.is_empty());
        assert_eq!(s.wait_calls, 0);
    }
}
#[test]
fn sent_frame_is_retained_with_late_deadline_or_postwrite_clock_failure() {
    for after in [
        Ok(MonoTime(FINALIZE_NS)),
        Err(io::ErrorKind::PermissionDenied.into()),
    ] {
        let mut w = TerminalWriter::new(Vec::new(), 7).unwrap();
        let mut s = Schedule {
            times: VecDeque::from([Ok(MonoTime(0)), Ok(MonoTime(0)), after]),
            ..Default::default()
        };
        let r = send_bounded(&mut w, Some(facts()), &mut s);
        assert!(r.frame_sent && r.failure.is_some());
        assert_eq!(r.attempts, 1);
        assert_eq!(w.writer, frame());
        assert_eq!(s.wait_calls, 0);
    }
}
#[test]
fn sender_wait_failure_endpoint_and_deadline_prevent_another_write() {
    for (wait, expected) in [
        (
            Err(io::ErrorKind::Interrupted.into()),
            ScheduleFailure::Wait(SystemFailure {
                kind: io::ErrorKind::Interrupted,
                code: None,
            }),
        ),
        (Ok((MonoTime(2), true)), ScheduleFailure::Endpoint),
        (
            Ok((MonoTime(FINALIZE_NS + 2), false)),
            ScheduleFailure::Deadline,
        ),
        (Ok((MonoTime(1), false)), ScheduleFailure::Backwards),
    ] {
        let mut w = TerminalWriter::new(
            Writer {
                bytes: vec![],
                block: true,
                calls: 0,
            },
            7,
        )
        .unwrap();
        let mut s = Schedule {
            times: VecDeque::from([Ok(MonoTime(2)), Ok(MonoTime(2)), Ok(MonoTime(2))]),
            waits: VecDeque::from([wait]),
            ..Default::default()
        };
        let r = send_bounded(&mut w, Some(facts()), &mut s);
        assert!(matches!(r.failure,Some(DeliveryFailure::Schedule(actual)) if actual==expected));
        assert_eq!(w.writer.calls, 1);
        assert_eq!(s.wait_calls, 1);
    }
}
struct Reader {
    bytes: Cursor<Vec<u8>>,
    block: bool,
    calls: usize,
}
impl Read for Reader {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.block {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = b.len().min(1);
        self.bytes.read(&mut b[..n])
    }
}
#[test]
fn receiver_requires_full_frame_and_eof_under_a_finite_schedule() {
    let mut reader = TerminalReader::new(
        Reader {
            bytes: Cursor::new(frame()),
            block: false,
            calls: 0,
        },
        7,
    )
    .unwrap();
    let mut s = Schedule::default();
    let r = observe_bounded(&mut reader, &mut s);
    assert_eq!(
        r.status,
        ReadStatus::Complete(TerminalFrame {
            run_id: 7,
            report: facts()
        })
    );
    assert!(r.failure.is_none());
    assert_eq!(r.attempts, 33);
    assert_eq!(s.wait_calls, 32);
    assert_eq!(reader.reader.calls, 33);
}
#[test]
fn receiver_budget_and_wait_errors_preserve_pending_without_extra_reads() {
    let mut reader = TerminalReader::new(
        Reader {
            bytes: Cursor::new(vec![]),
            block: true,
            calls: 0,
        },
        7,
    )
    .unwrap();
    let mut s = Schedule::default();
    let r = observe_bounded(&mut reader, &mut s);
    assert_eq!(r.status, ReadStatus::Pending);
    assert_eq!(r.failure, Some(ScheduleFailure::AttemptBudget));
    assert_eq!(reader.reader.calls, MAX_ATTEMPTS);
    assert_eq!(s.wait_calls, MAX_ATTEMPTS - 1);
    for wait in [
        Err(io::ErrorKind::PermissionDenied.into()),
        Ok((MonoTime(0), true)),
        Ok((MonoTime(FINALIZE_NS), false)),
    ] {
        let mut reader = TerminalReader::new(
            Reader {
                bytes: Cursor::new(vec![]),
                block: true,
                calls: 0,
            },
            7,
        )
        .unwrap();
        let mut s = Schedule {
            waits: VecDeque::from([wait]),
            ..Default::default()
        };
        let r = observe_bounded(&mut reader, &mut s);
        assert_eq!(r.status, ReadStatus::Pending);
        assert!(r.failure.is_some());
        assert_eq!(reader.reader.calls, 1);
    }
}
#[test]
fn receiver_keeps_complete_missing_and_invalid_status_alongside_late_clock_failure() {
    for (bytes, status) in [
        (
            frame(),
            ReadStatus::Complete(TerminalFrame {
                run_id: 7,
                report: facts(),
            }),
        ),
        (vec![], ReadStatus::Missing),
        (vec![1; 32], ReadStatus::Invalid(ProtocolError::Frame)),
    ] {
        let mut reader = TerminalReader::new(Cursor::new(bytes), 7).unwrap();
        if status != ReadStatus::Missing {
            assert_eq!(reader.poll(), ReadStatus::Pending);
        }
        let mut s = Schedule {
            times: VecDeque::from([
                Ok(MonoTime(0)),
                Ok(MonoTime(0)),
                Err(io::ErrorKind::PermissionDenied.into()),
            ]),
            ..Default::default()
        };
        let r = observe_bounded(&mut reader, &mut s);
        assert_eq!(r.status, status);
        assert!(matches!(r.failure, Some(ScheduleFailure::Clock(_))));
        assert_eq!(r.attempts, 1);
        assert_eq!(s.wait_calls, 0);
    }
}
