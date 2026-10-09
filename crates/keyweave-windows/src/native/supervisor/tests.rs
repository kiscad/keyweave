#![allow(clippy::unwrap_used)]
use super::*;
use keyweave_progress::Challenge;
use std::collections::VecDeque;
#[derive(Default)]
struct FakeProcess {
    exit: Option<u32>,
    stops: usize,
    polls: usize,
    stop_fail: bool,
    wait_fail: bool,
}
impl Process for FakeProcess {
    fn poll_exit(&mut self) -> Result<Option<u32>, JobError> {
        self.polls += 1;
        if self.wait_fail {
            Err(JobError::Wait(13))
        } else {
            Ok(self.exit)
        }
    }
    fn stop(&mut self) -> Result<(), JobError> {
        self.stops += 1;
        if self.stop_fail {
            Err(JobError::Api {
                call: "stop",
                code: 5,
            })
        } else {
            Ok(())
        }
    }
}
#[derive(Default)]
struct Input {
    bytes: VecDeque<u8>,
    eof: bool,
    chunk: usize,
    calls: usize,
    bad_count: bool,
}
impl Read for Input {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.bad_count {
            return Ok(out.len() + 1);
        }
        if self.bytes.is_empty() {
            return if self.eof {
                Ok(0)
            } else {
                Err(io::ErrorKind::WouldBlock.into())
            };
        }
        let n = out.len().min(self.chunk).min(self.bytes.len());
        for b in &mut out[..n] {
            *b = self.bytes.pop_front().unwrap();
        }
        Ok(n)
    }
}
#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    chunk: usize,
    calls: usize,
    blocked: bool,
    zero: bool,
    bad_count: bool,
}
impl Write for Output {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.bad_count {
            return Ok(data.len() + 1);
        }
        if self.blocked {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        if self.zero {
            return Ok(0);
        }
        let n = data.len().min(self.chunk);
        self.bytes.extend_from_slice(&data[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
type Fixture = Core<FakeProcess, Input, Output>;
fn fixture() -> Fixture {
    let watchdog = ProgressWatchdog::new(
        7,
        Policy {
            startup_ns: 10,
            lease_ns: 5,
        },
        MonoTime(0),
    )
    .unwrap();
    let request = watchdog.challenge().unwrap().encode();
    Core::new(
        FakeProcess::default(),
        Input {
            chunk: 40,
            ..Input::default()
        },
        Output {
            chunk: 24,
            ..Output::default()
        },
        watchdog,
        request,
    )
}
fn response(s: &mut Fixture, id: u64, baseline: u64, epoch: u64, phase: ProgressPhase) {
    s.reader.bytes.extend(
        Receipt {
            challenge: Challenge { run_id: 7, id },
            baseline,
            epoch,
            phase,
        }
        .encode(),
    );
}
fn stopped(s: &Fixture, reason: TerminationReason, sent: bool) {
    assert_eq!(
        s.status(),
        SupervisorStatus::Stopping {
            reason,
            stop_sent: sent
        }
    );
}
#[test]
fn partial_streams_have_one_io_each_poll_and_cannot_mint_progress() {
    let mut s = fixture();
    s.writer.chunk = 3;
    s.reader.chunk = 4;
    response(&mut s, 1, 0, 1, ProgressPhase::Active);
    for _ in 0..16 {
        assert_eq!(s.poll(MonoTime(1)).unwrap(), SupervisorStatus::Running);
        assert_eq!(s.watchdog.epoch(), 0);
    }
    assert_eq!(s.writer.calls, 8);
    assert_eq!(s.reader.calls, 9);
    assert_eq!(s.process.polls, 16);
    s.poll(MonoTime(1)).unwrap();
    assert_eq!(s.watchdog.epoch(), 1);
    assert_eq!(s.watchdog.deadline(), MonoTime(6));
    assert_eq!(s.writer.bytes, Challenge { run_id: 7, id: 1 }.encode());
    s.poll(MonoTime(2)).unwrap();
    assert_eq!(s.writer.bytes.len(), 27);
    assert_eq!(s.watchdog.epoch(), 1);
}
#[test]
fn startup_expiry_precedes_queued_receipt_and_wouldblock_never_renews() {
    let mut s = fixture();
    s.writer.blocked = true;
    response(&mut s, 1, 0, 1, ProgressPhase::Active);
    for now in [0, 5, 9] {
        s.poll(MonoTime(now)).unwrap();
        assert_eq!(s.watchdog.deadline(), MonoTime(10));
    }
    s.poll(MonoTime(10)).unwrap();
    stopped(
        &s,
        TerminationReason::Progress(ProgressError::Expired),
        true,
    );
    assert_eq!(s.reader.calls, 0);
    assert_eq!(s.watchdog.epoch(), 0);
    s.poll(MonoTime(0)).unwrap();
    assert_eq!(s.process.stops, 1);
    assert_eq!(s.reader.calls, 0);
    s.process.exit = Some(259);
    assert!(matches!(
        s.poll(MonoTime(0)).unwrap(),
        SupervisorStatus::Exited { code: 259, .. }
    ));
}
#[test]
fn malformed_wrong_stale_frozen_and_backwards_receipts_lock_first_reason() {
    for kind in 0..7 {
        let mut s = fixture();
        response(
            &mut s,
            if kind == 1 { 2 } else { 1 },
            0,
            if kind == 2 { 0 } else { 1 },
            ProgressPhase::Active,
        );
        if kind == 0 {
            s.reader.bytes[0] = 0;
        }
        if kind == 3 {
            s.poll(MonoTime(2)).unwrap();
            response(&mut s, 1, 0, 2, ProgressPhase::Active);
        }
        if kind == 4 {
            s.poll(MonoTime(2)).unwrap();
        }
        if kind == 5 {
            s.poll(MonoTime(1)).unwrap();
            response(&mut s, 2, 1, 1, ProgressPhase::Active);
        }
        if kind == 6 {
            s.reader.bytes[8] = 8;
        }
        s.poll(MonoTime(if kind == 3 { 3 } else { 1 })).unwrap();
        let expect = match kind {
            0 => ProgressError::Frame,
            1 | 3 | 6 => ProgressError::Identity,
            2 | 5 => ProgressError::Epoch,
            4 => ProgressError::Clock,
            _ => unreachable!(),
        };
        stopped(&s, TerminationReason::Progress(expect), true);
        let reads = s.reader.calls;
        s.stop(TerminationReason::Requested).unwrap();
        s.poll(MonoTime(3)).unwrap();
        stopped(&s, TerminationReason::Progress(expect), true);
        assert_eq!(s.reader.calls, reads);
    }
}
#[test]
fn stopped_and_eof_wait_for_real_exit_and_have_finite_last_lease() {
    let mut s = fixture();
    response(&mut s, 1, 0, 1, ProgressPhase::Stopped);
    s.poll(MonoTime(1)).unwrap();
    s.reader.eof = true;
    s.poll(MonoTime(2)).unwrap();
    assert_eq!(s.status(), SupervisorStatus::Running);
    assert_eq!(s.writer.bytes.len(), 24);
    assert!(s.channel_closed);
    s.poll(MonoTime(6)).unwrap();
    stopped(
        &s,
        TerminationReason::Progress(ProgressError::Expired),
        true,
    );
    assert!(s.exit.is_none());
    s.process.exit = Some(259);
    s.poll(MonoTime(6)).unwrap();
    assert!(matches!(
        s.status(),
        SupervisorStatus::Exited { code: 259, .. }
    ));
}
#[test]
fn stop_failure_is_retained_retried_without_ipc_and_not_exit() {
    let mut s = fixture();
    s.process.stop_fail = true;
    assert_eq!(
        s.poll(MonoTime(10)).unwrap_err().cause,
        JobError::Api {
            call: "stop",
            code: 5
        }
    );
    stopped(
        &s,
        TerminationReason::Progress(ProgressError::Expired),
        false,
    );
    s.process.stop_fail = false;
    s.poll(MonoTime(11)).unwrap();
    stopped(
        &s,
        TerminationReason::Progress(ProgressError::Expired),
        true,
    );
    assert_eq!(s.process.stops, 2);
    assert_eq!(s.reader.calls, 0);
    assert_eq!(s.writer.calls, 0);
    s.process.exit = Some(7);
    s.poll(MonoTime(12)).unwrap();
    s.stop(TerminationReason::Requested).unwrap();
    assert_eq!(s.process.stops, 2);
}
#[test]
fn process_observation_failure_keeps_primary_and_stop_error_then_actual_exit() {
    let mut s = fixture();
    s.process.wait_fail = true;
    s.process.stop_fail = true;
    let e = s.poll(MonoTime(1)).unwrap_err();
    assert_eq!(e.cause, JobError::Wait(13));
    assert!(e.stop.is_some());
    stopped(&s, TerminationReason::Process(JobError::Wait(13)), false);
    s.process.wait_fail = false;
    s.process.stop_fail = false;
    s.poll(MonoTime(2)).unwrap();
    assert_eq!(s.reader.calls, 0);
    assert_eq!(s.writer.calls, 0);
    s.process.exit = Some(9);
    s.poll(MonoTime(3)).unwrap();
    assert!(matches!(
        s.status(),
        SupervisorStatus::Exited {
            code: 9,
            reason: Some(TerminationReason::Process(JobError::Wait(13)))
        }
    ));
}
#[test]
fn eof_half_frames_zero_write_and_invalid_counts_fail_closed() {
    for kind in 0..5 {
        let mut s = fixture();
        match kind {
            0 => s.reader.eof = true,
            1 => {
                s.reader.bytes.push_back(1);
                s.reader.eof = true;
            }
            2 => s.writer.zero = true,
            3 => s.reader.bad_count = true,
            _ => s.writer.bad_count = true,
        }
        s.poll(MonoTime(1)).unwrap();
        if kind == 1 {
            s.poll(MonoTime(2)).unwrap();
        }
        assert!(matches!(
            s.status(),
            SupervisorStatus::Stopping {
                stop_sent: true,
                ..
            }
        ));
        assert_eq!(s.process.stops, 1);
    }
}
#[test]
fn signaled_exit_wins_over_deadline_and_cached_exit_never_polls_or_stops() {
    let mut s = fixture();
    s.process.exit = Some(259);
    assert_eq!(
        s.poll(MonoTime(10)).unwrap(),
        SupervisorStatus::Exited {
            code: 259,
            reason: None
        }
    );
    s.poll(MonoTime(0)).unwrap();
    s.stop(TerminationReason::Requested).unwrap();
    assert_eq!(s.process.polls, 1);
    assert_eq!(s.process.stops, 0);
    assert_eq!(s.reader.calls, 0);
}
#[test]
fn callback_rejects_native_supervision_without_changing_owner() {
    let mut s = Supervisor::spawn(
        &std::env::temp_dir().join("worker.exe"),
        &[],
        7,
        Policy::default(),
        MonoTime(0),
    )
    .unwrap();
    super::super::hook_thread::with_callback_borrow(|| {
        assert_eq!(s.poll(MonoTime(0)).unwrap_err().cause, JobError::InCallback);
        assert_eq!(s.request_stop().unwrap_err().cause, JobError::InCallback);
        assert_eq!(s.close_channels().unwrap_err(), JobError::InCallback);
        assert!(
            Supervisor::spawn(
                &std::env::temp_dir().join("worker.exe"),
                &[],
                7,
                Policy::default(),
                MonoTime(0)
            )
            .is_err()
        );
    });
    assert_eq!(s.status(), SupervisorStatus::Running);
    assert_eq!(s.core.written, 0);
    assert_eq!(s.close_channels().unwrap_err(), JobError::State);
    assert_eq!(s.deadline(), MonoTime(10_000_000_000));
    // Model an already observed exit to exercise the public cleanup gate;
    // native fixture separately observes the real process handle.
    s.core.exit = Some(259);
    assert!(s.close_channels().unwrap().complete());
}

#[test]
fn queued_fast_reply_waits_for_delayed_write_ack_without_false_identity_failure() {
    let mut s = fixture();
    s.writer.blocked = true;
    response(&mut s, 1, 0, 1, ProgressPhase::Active);
    assert_eq!(s.poll(MonoTime(1)).unwrap(), SupervisorStatus::Running);
    assert_eq!(s.reader.calls, 0);
    assert_eq!(s.watchdog.epoch(), 0);
    assert_eq!(s.watchdog.deadline(), MonoTime(10));
    s.writer.blocked = false;
    assert_eq!(s.poll(MonoTime(2)).unwrap(), SupervisorStatus::Running);
    assert_eq!(s.writer.bytes, Challenge { run_id: 7, id: 1 }.encode());
    assert_eq!(s.watchdog.epoch(), 1);
    assert_eq!(s.watchdog.deadline(), MonoTime(7));
    assert_eq!(s.process.stops, 0);
}
