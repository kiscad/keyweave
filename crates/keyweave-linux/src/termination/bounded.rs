//! Explicit-clock bounded finalization; native owners provide nonblocking IO.
use super::{ReadStatus, SendError, TerminalReader, TerminalWriter};
use crate::runtime::TerminationReport;
use keyweave_types::MonoTime;
use std::io::{self, Read, Write};
pub const FINALIZE_NS: u64 = 100_000_000;
pub const MAX_ATTEMPTS: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemFailure {
    pub kind: io::ErrorKind,
    pub code: Option<i32>,
}
impl From<io::Error> for SystemFailure {
    fn from(e: io::Error) -> Self {
        Self {
            kind: e.kind(),
            code: e.raw_os_error(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleFailure {
    Clock(SystemFailure),
    Wait(SystemFailure),
    Backwards,
    TimeOverflow,
    Deadline,
    AttemptBudget,
    Endpoint,
}
#[derive(Debug)]
pub enum DeliveryFailure {
    MissingReport,
    Admission(SystemFailure),
    Write(SendError),
    Schedule(ScheduleFailure),
}
#[derive(Debug)]
pub struct DeliveryReport {
    pub frame_sent: bool,
    pub attempts: usize,
    pub failure: Option<DeliveryFailure>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservationReport {
    pub status: ReadStatus,
    pub attempts: usize,
    pub failure: Option<ScheduleFailure>,
}
/// Must wait finitely on the terminal endpoint alone; readiness isn't a receipt.
pub trait SendSchedule<W: Write> {
    fn now(&mut self) -> io::Result<MonoTime>;
    fn wait(
        &mut self,
        writer: &TerminalWriter<W>,
        deadline: MonoTime,
    ) -> io::Result<(MonoTime, bool)>;
}
pub trait ReadSchedule<R: Read> {
    fn now(&mut self) -> io::Result<MonoTime>;
    fn wait(
        &mut self,
        reader: &TerminalReader<R>,
        deadline: MonoTime,
    ) -> io::Result<(MonoTime, bool)>;
}
fn check(now: MonoTime, last: &mut MonoTime, deadline: MonoTime) -> Result<(), ScheduleFailure> {
    if now < *last {
        return Err(ScheduleFailure::Backwards);
    }
    *last = now;
    if now >= deadline {
        return Err(ScheduleFailure::Deadline);
    }
    Ok(())
}
fn limits(now: io::Result<MonoTime>) -> Result<(MonoTime, MonoTime), ScheduleFailure> {
    let now = now.map_err(|e| ScheduleFailure::Clock(e.into()))?;
    Ok((
        now,
        now.checked_add(FINALIZE_NS)
            .ok_or(ScheduleFailure::TimeOverflow)?,
    ))
}
pub fn send_bounded<W: Write, S: SendSchedule<W>>(
    writer: &mut TerminalWriter<W>,
    report: Option<TerminationReport>,
    schedule: &mut S,
) -> DeliveryReport {
    let mut result = DeliveryReport {
        frame_sent: false,
        attempts: 0,
        failure: None,
    };
    let Some(report) = report else {
        result.failure = Some(DeliveryFailure::MissingReport);
        return result;
    };
    let (mut last, deadline) = match limits(schedule.now()) {
        Ok(v) => v,
        Err(e) => {
            result.failure = Some(DeliveryFailure::Schedule(e));
            return result;
        }
    };
    if let Err(e) = writer.prepare(report) {
        result.failure = Some(DeliveryFailure::Write(e));
        return result;
    }
    let outcome = (|| -> Result<(), DeliveryFailure> {
        for attempt in 0..MAX_ATTEMPTS {
            let now = schedule
                .now()
                .map_err(|e| DeliveryFailure::Schedule(ScheduleFailure::Clock(e.into())))?;
            check(now, &mut last, deadline).map_err(DeliveryFailure::Schedule)?;
            result.attempts = attempt + 1;
            result.frame_sent = writer.flush().map_err(DeliveryFailure::Write)?;
            let now = schedule
                .now()
                .map_err(|e| DeliveryFailure::Schedule(ScheduleFailure::Clock(e.into())))?;
            check(now, &mut last, deadline).map_err(DeliveryFailure::Schedule)?;
            if result.frame_sent {
                return Ok(());
            }
            if result.attempts == MAX_ATTEMPTS {
                return Err(DeliveryFailure::Schedule(ScheduleFailure::AttemptBudget));
            }
            let (now, fault) = schedule
                .wait(writer, deadline)
                .map_err(|e| DeliveryFailure::Schedule(ScheduleFailure::Wait(e.into())))?;
            check(now, &mut last, deadline).map_err(DeliveryFailure::Schedule)?;
            if fault {
                return Err(DeliveryFailure::Schedule(ScheduleFailure::Endpoint));
            }
        }
        Err(DeliveryFailure::Schedule(ScheduleFailure::AttemptBudget))
    })();
    result.failure = outcome.err();
    result
}
/// Owner calls only after actual child exit. Missing/invalid/IO status is
/// preserved, independently from any schedule failure observed after the read.
pub fn observe_bounded<R: Read, S: ReadSchedule<R>>(
    reader: &mut TerminalReader<R>,
    schedule: &mut S,
) -> ObservationReport {
    let mut result = ObservationReport {
        status: ReadStatus::Pending,
        attempts: 0,
        failure: None,
    };
    let (mut last, deadline) = match limits(schedule.now()) {
        Ok(v) => v,
        Err(e) => {
            result.failure = Some(e);
            return result;
        }
    };
    let outcome = (|| -> Result<(), ScheduleFailure> {
        for attempt in 0..MAX_ATTEMPTS {
            let now = schedule
                .now()
                .map_err(|e| ScheduleFailure::Clock(e.into()))?;
            check(now, &mut last, deadline)?;
            result.attempts = attempt + 1;
            result.status = reader.poll();
            let now = schedule
                .now()
                .map_err(|e| ScheduleFailure::Clock(e.into()))?;
            check(now, &mut last, deadline)?;
            if result.status != ReadStatus::Pending {
                return Ok(());
            }
            if result.attempts == MAX_ATTEMPTS {
                return Err(ScheduleFailure::AttemptBudget);
            }
            let (now, fault) = schedule
                .wait(reader, deadline)
                .map_err(|e| ScheduleFailure::Wait(e.into()))?;
            check(now, &mut last, deadline)?;
            if fault {
                return Err(ScheduleFailure::Endpoint);
            }
        }
        Err(ScheduleFailure::AttemptBudget)
    })();
    result.failure = outcome.err();
    result
}
#[cfg(test)]
mod tests;
