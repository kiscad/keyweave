//! Main-thread host stop signals. Install before creating threads or workers.
#![allow(unsafe_code)]
use std::{
    fs::File,
    io::{self, Read},
    marker::PhantomData,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::Command,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
const SIGNALS: [i32; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
static INSTALLED: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopSignal {
    Interrupt,
    Terminate,
    Hangup,
}
fn decode(record: &[u8]) -> io::Result<StopSignal> {
    if record.len() != 128 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid signal record length",
        ));
    }
    let number = u32::from_ne_bytes([record[0], record[1], record[2], record[3]]);
    match number as i32 {
        libc::SIGINT => Ok(StopSignal::Interrupt),
        libc::SIGTERM => Ok(StopSignal::Terminate),
        libc::SIGHUP => Ok(StopSignal::Hangup),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected stop signal",
        )),
    }
}
fn errno_result(value: i32) -> io::Result<()> {
    if value == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
fn mask(
    how: i32,
    set: Option<&libc::sigset_t>,
    old: Option<&mut libc::sigset_t>,
) -> io::Result<()> {
    // SAFETY: optional pointers refer to aligned live signal sets for this
    // synchronous call. pthread_sigmask changes only the calling thread.
    let result = unsafe {
        libc::pthread_sigmask(
            how,
            set.map_or(std::ptr::null(), |s| s),
            old.map_or(std::ptr::null_mut(), |s| s),
        )
    };
    // pthread APIs return the error number directly, not through errno.
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}
fn empty_set() -> io::Result<libc::sigset_t> {
    // SAFETY: sigset_t is integer storage; initialized before any query.
    let mut set = unsafe { std::mem::zeroed::<libc::sigset_t>() };
    // SAFETY: exclusive live aligned output set, no retained pointer.
    errno_result(unsafe { libc::sigemptyset(&mut set) })?;
    Ok(set)
}
fn member(set: &libc::sigset_t, signal: i32) -> io::Result<bool> {
    // SAFETY: immutable initialized set and one supported signal number.
    let value = unsafe { libc::sigismember(set, signal) };
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(io::Error::last_os_error()),
    }
}
/// Executable worker contract: selected host stop signals must not remain
/// masked after exec. This query neither changes masks nor installs handlers.
pub(crate) fn verify_worker_mask() -> io::Result<()> {
    let mut set = empty_set()?;
    mask(libc::SIG_BLOCK, None, Some(&mut set))?;
    for signal in SIGNALS {
        if member(&set, signal)? {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
    }
    Ok(())
}
/// Not Send/Sync. Sole main thread at installation, same thread throughout.
/// Additional threads must inherit this mask and never unblock these signals.
/// No handler or background heartbeat; each poll reads at most one record.
pub struct HostSignals {
    fd: Option<File>,
    previous: libc::sigset_t,
    restored: bool,
    _thread: PhantomData<Rc<()>>,
}
impl HostSignals {
    pub fn install() -> io::Result<Self> {
        // SAFETY: read-only scalar process/thread identity queries.
        if unsafe { libc::getpid() } != unsafe { libc::gettid() } {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        // Reject an already multithreaded embedding. Read at most two entries;
        // caller still owns the no-concurrent-thread-creation premise.
        let mut tasks = std::fs::read_dir("/proc/self/task")?;
        if tasks.next().transpose()?.is_none() || tasks.next().transpose()?.is_some() {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        if INSTALLED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let result = Self::install_inner();
        if result.is_err() {
            INSTALLED.store(false, Ordering::Release);
        }
        result
    }
    fn install_inner() -> io::Result<Self> {
        let mut selected = empty_set()?;
        let mut previous = empty_set()?;
        mask(libc::SIG_BLOCK, None, Some(&mut previous))?;
        for signal in SIGNALS {
            if member(&previous, signal)? {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
            // SAFETY: exclusive initialized set and valid signal number.
            errno_result(unsafe { libc::sigaddset(&mut selected, signal) })?;
            // SAFETY: sigaction is integer/pointer storage; null act performs a
            // read-only query into exclusively borrowed initialized storage.
            let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
            // SAFETY: live aligned output, null input leaves disposition intact.
            errno_result(unsafe { libc::sigaction(signal, std::ptr::null(), &mut action) })?;
            if action.sa_sigaction != libc::SIG_DFL {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
        }
        mask(libc::SIG_BLOCK, Some(&selected), None)?;
        let mut owner = Self {
            fd: None,
            previous,
            restored: false,
            _thread: PhantomData,
        };
        // SAFETY: -1 creates a new owned descriptor. Initialized mask lives for
        // call; NONBLOCK/CLOEXEC are applied atomically. No signal/device send.
        let fd = unsafe { libc::signalfd(-1, &selected, libc::SFD_NONBLOCK | libc::SFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful signalfd returned one new exclusively owned FD.
        owner.fd = Some(unsafe { File::from_raw_fd(fd) });
        Ok(owner)
    }
    fn verify_mask(&self) -> io::Result<()> {
        let mut current = empty_set()?;
        mask(libc::SIG_BLOCK, None, Some(&mut current))?;
        for signal in SIGNALS {
            if !member(&current, signal)? {
                return Err(io::Error::from_raw_os_error(libc::EPERM));
            }
        }
        Ok(())
    }
    /// Trusted Command only. Must spawn while this owner is alive. Restores
    /// exactly the pre-install mask in child; host remains blocked. Caller
    /// pre_exec hooks must not subsequently mutate masks/groups/session.
    pub fn prepare_child(&self, command: &mut Command) -> io::Result<()> {
        self.verify_mask()?;
        let previous = self.previous;
        // SAFETY: pre_exec performs only async-signal-safe sigprocmask and
        // scalar errno conversion; captured sigset_t is by value, no parent
        // locks, allocation, IO or retained pointer. No handler installation.
        unsafe {
            command.pre_exec(move || {
                if libc::sigprocmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        Ok(())
    }
    pub fn poll(&mut self) -> io::Result<Option<StopSignal>> {
        self.verify_mask()?;
        let mut record = [0u8; 128];
        let fd = self.fd.as_mut().ok_or(io::ErrorKind::NotConnected)?;
        match fd.read(&mut record) {
            Ok(n) => decode(&record[..n]).map(Some),
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
    /// At most 20ms; readiness is not signal consumption or worker progress.
    pub fn wait(&self, duration: Duration) -> io::Result<()> {
        self.verify_mask()?;
        let fd = self.fd.as_ref().ok_or(io::ErrorKind::NotConnected)?;
        let mut query = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = i32::try_from(duration.min(Duration::from_millis(20)).as_millis())
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: one initialized exclusively borrowed pollfd lives until poll
        // returns; valid owned FD and a bounded nonnegative timeout.
        let result = unsafe { libc::poll(&mut query, 1, ms) };
        if result < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(());
            }
            return Err(e);
        }
        if query.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err(io::Error::other("stop signal endpoint failed"));
        }
        Ok(())
    }
    /// Call only after owned worker actual exit/cleanup. Restoring the original
    /// mask may deliver a signal arriving during closure with its old default
    /// disposition. Drop is a fallback, not an exit/cleanup guarantee.
    pub fn close(mut self) -> io::Result<()> {
        drop(self.fd.take());
        mask(libc::SIG_SETMASK, Some(&self.previous), None)?;
        self.restored = true;
        Ok(())
    }
}
impl Drop for HostSignals {
    fn drop(&mut self) {
        drop(self.fd.take());
        if !self.restored {
            let _ = mask(libc::SIG_SETMASK, Some(&self.previous), None);
        }
        INSTALLED.store(false, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signal_records_are_exact_and_only_selected_stop_signals() {
        for (number, want) in [
            (libc::SIGINT, StopSignal::Interrupt),
            (libc::SIGTERM, StopSignal::Terminate),
            (libc::SIGHUP, StopSignal::Hangup),
        ] {
            let mut bytes = [0; 128];
            bytes[..4].copy_from_slice(&(number as u32).to_ne_bytes());
            assert_eq!(decode(&bytes).ok(), Some(want));
            assert!(decode(&bytes[..127]).is_err());
        }
        assert!(decode(&[0; 128]).is_err());
        assert!(decode(&[0; 129]).is_err());
    }
    #[test]
    fn libtest_thread_cannot_install_or_change_host_mask() {
        let mut before = empty_set().unwrap_or_else(|e| panic!("{e}"));
        mask(libc::SIG_BLOCK, None, Some(&mut before)).unwrap_or_else(|e| panic!("{e}"));
        assert!(HostSignals::install().is_err());
        let mut after = empty_set().unwrap_or_else(|e| panic!("{e}"));
        mask(libc::SIG_BLOCK, None, Some(&mut after)).unwrap_or_else(|e| panic!("{e}"));
        for signal in SIGNALS {
            assert_eq!(member(&before, signal).ok(), member(&after, signal).ok());
        }
    }
}
