//! Bounded native waiting. Readiness is a hint, never an I/O receipt.
use keyweave_types::MonoTime;
use std::{io, os::fd::BorrowedFd};
pub const MAX_WAIT_FDS: usize = 67;
pub const MAX_WAIT_NS: u64 = 1_000_000_000;
pub const HOUSEKEEPING_NS: u64 = 20_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interest {
    Errors,
    Read,
    Write,
}
#[derive(Clone, Copy)]
pub struct WaitRequest<'a> {
    pub fd: BorrowedFd<'a>,
    pub interest: Interest,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Readiness {
    pub readable: bool,
    pub writable: bool,
    pub hangup: bool,
    pub error: bool,
    pub invalid: bool,
}
impl Readiness {
    pub fn fault(self) -> bool {
        self.hangup || self.error || self.invalid
    }
    fn decode(bits: i16) -> io::Result<Self> {
        if bits & !(libc::POLLIN | libc::POLLOUT | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)
            != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected poll event",
            ));
        }
        Ok(Self {
            readable: bits & libc::POLLIN != 0,
            writable: bits & libc::POLLOUT != 0,
            hangup: bits & libc::POLLHUP != 0,
            error: bits & libc::POLLERR != 0,
            invalid: bits & libc::POLLNVAL != 0,
        })
    }
}
#[derive(Debug)]
pub struct Wake {
    pub at_ns: MonoTime,
    pub interrupted: bool,
    readiness: [Readiness; MAX_WAIT_FDS],
    len: usize,
}
impl Wake {
    pub fn readiness(&self, index: usize) -> Option<Readiness> {
        (index < self.len).then(|| self.readiness[index])
    }
    pub fn any_fault(&self) -> bool {
        self.readiness[..self.len].iter().any(|ready| ready.fault())
    }
}
#[derive(Debug, Default)]
pub struct MonotonicClock {
    last: Option<MonoTime>,
}
impl MonotonicClock {
    pub fn new() -> Self {
        Self::default()
    }
    fn observe(&mut self, nanos: u64) -> io::Result<MonoTime> {
        let now = MonoTime(nanos);
        if self.last.is_some_and(|last| now < last) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "monotonic clock moved backwards",
            ));
        }
        self.last = Some(now);
        Ok(now)
    }
    pub fn now(&mut self) -> io::Result<MonoTime> {
        self.observe(crate::ioctl::monotonic_now()?)
    }
    pub fn last(&self) -> Option<MonoTime> {
        self.last
    }
}
/// No retry or infinite timeout. EINTR returns a wake with a fresh clock.
pub fn wait(
    clock: &mut MonotonicClock,
    requests: &[WaitRequest<'_>],
    deadline: Option<MonoTime>,
    maximum_ns: u64,
) -> io::Result<Wake> {
    if requests.len() > MAX_WAIT_FDS || maximum_ns == 0 || maximum_ns > MAX_WAIT_NS {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let now = clock.now()?;
    let timeout = deadline.map_or(maximum_ns, |deadline| {
        deadline.0.saturating_sub(now.0).min(maximum_ns)
    });
    let bits = crate::ioctl::wait_ready(requests, timeout)?;
    let mut readiness = [Readiness::default(); MAX_WAIT_FDS];
    if let Some(bits) = bits {
        for (ready, bits) in readiness.iter_mut().zip(bits).take(requests.len()) {
            *ready = Readiness::decode(bits)?;
        }
    }
    Ok(Wake {
        at_ns: clock.now()?,
        interrupted: bits.is_none(),
        readiness,
        len: requests.len(),
    })
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::fd::AsFd,
    };
    #[test]
    fn pipe_readiness_preserves_data_and_hangup_without_consuming_it() {
        let (mut reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"x").unwrap();
        drop(writer);
        let wake = wait(
            &mut MonotonicClock::new(),
            &[WaitRequest {
                fd: reader.as_fd(),
                interest: Interest::Read,
            }],
            None,
            HOUSEKEEPING_NS,
        )
        .unwrap();
        let ready = wake.readiness(0).unwrap();
        assert!(ready.readable && ready.hangup && ready.fault());
        assert_eq!(wake.readiness(1), None);
        let mut byte = [0];
        assert_eq!(reader.read(&mut byte).unwrap(), 1);
        assert_eq!(byte, *b"x");
    }
    #[test]
    fn errors_only_does_not_report_permanently_writable_endpoint() {
        let (reader, writer) = io::pipe().unwrap();
        let mut clock = MonotonicClock::new();
        let now = clock.now().unwrap();
        let wake = wait(
            &mut clock,
            &[WaitRequest {
                fd: writer.as_fd(),
                interest: Interest::Errors,
            }],
            Some(now),
            HOUSEKEEPING_NS,
        )
        .unwrap();
        assert_eq!(wake.readiness(0), Some(Readiness::default()));
        let wake = wait(
            &mut clock,
            &[WaitRequest {
                fd: writer.as_fd(),
                interest: Interest::Write,
            }],
            Some(now),
            HOUSEKEEPING_NS,
        )
        .unwrap();
        assert!(wake.readiness(0).unwrap().writable);
        drop(reader);
        let wake = wait(
            &mut clock,
            &[WaitRequest {
                fd: writer.as_fd(),
                interest: Interest::Errors,
            }],
            Some(now),
            HOUSEKEEPING_NS,
        )
        .unwrap();
        assert!(wake.readiness(0).unwrap().error);
    }
    #[test]
    fn real_clock_empty_wait_and_expired_deadline_use_actual_time() {
        let mut clock = MonotonicClock::new();
        let before = clock.now().unwrap();
        let wake = wait(&mut clock, &[], Some(MonoTime(0)), HOUSEKEEPING_NS).unwrap();
        assert!(wake.at_ns >= before && !wake.interrupted);
        assert_eq!(clock.last(), Some(wake.at_ns));
        assert!(!wake.any_fault());
        assert!(wait(&mut clock, &[], None, 0).is_err());
        assert!(wait(&mut clock, &[], None, MAX_WAIT_NS + 1).is_err());
    }
    #[test]
    fn future_deadline_uses_finite_native_timeout() {
        let mut clock = MonotonicClock::new();
        let before = clock.now().unwrap();
        let deadline = MonoTime(before.0 + 1_000_000);
        let wake = wait(&mut clock, &[], Some(deadline), HOUSEKEEPING_NS).unwrap();
        assert!(wake.at_ns >= before);
        if !wake.interrupted {
            assert!(wake.at_ns >= deadline);
        }
        assert!(!wake.any_fault());
    }
    #[test]
    fn capacity_and_backwards_clock_fail_without_rewinding_state() {
        let (reader, _writer) = io::pipe().unwrap();
        let request = WaitRequest {
            fd: reader.as_fd(),
            interest: Interest::Read,
        };
        let mut clock = MonotonicClock::new();
        assert!(wait(&mut clock, &[request; MAX_WAIT_FDS + 1], None, 1).is_err());
        assert_eq!(clock.last(), None);
        assert_eq!(clock.observe(5).unwrap(), MonoTime(5));
        assert!(clock.observe(4).is_err());
        assert_eq!(clock.last(), Some(MonoTime(5)));
        assert!(Readiness::decode(libc::POLLNVAL).unwrap().invalid);
        assert!(Readiness::decode(libc::POLLPRI).is_err());
    }
}
