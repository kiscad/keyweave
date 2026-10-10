//! Private Linux device/file-flag/process/clock/wait FFI; no arbitrary requests.
#![allow(unsafe_code)]
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd},
};
const MAX_BUFFER: usize = 4096;
pub(crate) const fn request(direction: u32, number: u8, size: usize) -> u32 {
    request_for(0x45, direction, number, size)
}
const fn request_for(tag: u8, direction: u32, number: u8, size: usize) -> u32 {
    (direction << 30) | ((size as u32) << 16) | ((tag as u32) << 8) | number as u32
}
fn call(fd: BorrowedFd<'_>, command: u32, buffer: &mut [u8]) -> io::Result<usize> {
    if buffer.is_empty()
        || buffer.len() > MAX_BUFFER
        || ((command >> 16) & 0x3fff) as usize != buffer.len()
        || ![0x45, 0x55].contains(&((command >> 8) & 0xff))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid ioctl buffer",
        ));
    }
    // SAFETY: fd is borrowed for the full synchronous call. All callers use
    // audited evdev/uinput requests encoding exactly buffer.len() bytes, initialized
    // and exclusively borrowed here. Byte-oriented queries copy within the
    // encoded range; scalar/version, clock and setup requests use aligned storage.
    // No pointer or reference is retained by these synchronous requests.
    let result =
        unsafe { libc::ioctl(fd.as_raw_fd(), command as libc::Ioctl, buffer.as_mut_ptr()) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(result as usize)
}
pub(crate) fn fixed(fd: BorrowedFd<'_>, number: u8, size: usize) -> io::Result<Vec<u8>> {
    if ![4, 8].contains(&size) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid scalar query size",
        ));
    }
    #[repr(C, align(8))]
    struct AlignedScalar([u8; 8]);
    let mut buffer = AlignedScalar([0; 8]);
    if call(fd, request(2, number, size), &mut buffer.0[..size])? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected scalar ioctl result",
        ));
    }
    Ok(buffer.0[..size].to_vec())
}
pub(crate) fn mask(fd: BorrowedFd<'_>, number: u8, size: usize) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0; size];
    let count = call(fd, request(2, number, size), &mut buffer)?;
    if count != size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete capability or state query",
        ));
    }
    Ok(buffer)
}
pub(crate) fn name(fd: BorrowedFd<'_>) -> io::Result<Option<Vec<u8>>> {
    let mut buffer = vec![0; 256];
    let count = match call(fd, request(2, 6, 256), &mut buffer) {
        Ok(count) => count,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
        Err(error) => return Err(error),
    };
    if count == 0 || count > buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid name query length",
        ));
    }
    let end = buffer[..count]
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unterminated device name"))?;
    buffer.truncate(end);
    Ok(Some(buffer))
}
pub(crate) fn monotonic_clock(fd: BorrowedFd<'_>) -> io::Result<()> {
    #[repr(C, align(4))]
    struct AlignedClock([u8; 4]);
    let mut clock = AlignedClock(libc::CLOCK_MONOTONIC.to_ne_bytes());
    if call(fd, request(1, 0xa0, 4), &mut clock.0)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected clock ioctl result",
        ));
    }
    Ok(())
}
// These UAPI requests take an integer argument despite _IOW(int) encoding.
fn scalar(fd: BorrowedFd<'_>, command: u32, argument: u32) -> io::Result<()> {
    // SAFETY: private callers select only audited scalar/no-data evdev/uinput requests;
    // the kernel uses the numeric argument, never dereferences it. FD is borrowed
    // for this synchronous call; no memory or descriptor is retained.
    let result = unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            command as libc::Ioctl,
            argument as libc::c_ulong,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if result != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected scalar ioctl result",
        ));
    }
    Ok(())
}
pub(crate) fn nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: borrowed fd stays open throughout both synchronous calls. F_GETFL
    // has no pointer argument; F_SETFL receives only audited scalar flags. Existing
    // file status flags are preserved, and no FD or memory escapes this boundary.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same live borrowed fd; preserve queried flags and add only O_NONBLOCK.
    let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
pub(crate) fn pipe_identity(
    fd: BorrowedFd<'_>,
    writer: bool,
) -> io::Result<(libc::dev_t, libc::ino_t)> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: aligned stack storage has exactly libc stat's ABI size. fstat
    // synchronously initializes its fields, retains no pointer, and the borrowed
    // descriptor stays live. We read no fields on syscall failure.
    if unsafe { libc::fstat(fd.as_raw_fd(), metadata.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat populated the initialized zero-backed scalar
    // stat structure. Padding is never read and the result contains no pointers.
    let metadata = unsafe { metadata.assume_init() };
    // SAFETY: synchronous scalar flag query on the live borrowed FD; no pointer.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let access = if writer {
        libc::O_WRONLY
    } else {
        libc::O_RDONLY
    };
    if metadata.st_mode & libc::S_IFMT != libc::S_IFIFO
        || flags & libc::O_ACCMODE != access
        || flags & libc::O_NONBLOCK == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected nonblocking pipe with the required direction",
        ));
    }
    Ok((metadata.st_dev, metadata.st_ino))
}

pub(crate) fn duplicate_progress_pipes(
    reader: BorrowedFd<'_>,
    writer: BorrowedFd<'_>,
) -> io::Result<(std::fs::File, std::fs::File)> {
    if pipe_identity(reader, false)? == pipe_identity(writer, true)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "progress pipes must be distinct",
        ));
    }
    // Only validated pipe endpoints are copied. Standard-library OwnedFd makes
    // both duplicates CLOEXEC and handles partial-construction cleanup.
    let reader = std::fs::File::from(reader.try_clone_to_owned()?);
    let writer = std::fs::File::from(writer.try_clone_to_owned()?);
    Ok((reader, writer))
}
// Keep this path allocation-free: it also runs after fork, before exec.
fn parent_matches(expected: libc::pid_t) -> io::Result<()> {
    // SAFETY: getppid takes no pointers, performs a synchronous PID query, and
    // is async-signal-safe. A parent outside this PID namespace (0) is rejected.
    if unsafe { libc::getppid() } != expected {
        return Err(io::Error::from_raw_os_error(libc::ECANCELED));
    }
    Ok(())
}
pub(crate) fn command_parent_death(
    command: &mut std::process::Command,
    expected: u32,
) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let expected = libc::pid_t::try_from(expected)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: capture only a PID scalar. The hook queries getppid, executes a
    // direct Linux syscall, and constructs only inline raw-errno io::Errors.
    // It neither allocates, locks, accesses environment, nor touches inherited
    // library objects. Registration is in the parent, after all caller hooks.
    unsafe {
        command.pre_exec(move || {
            parent_matches(expected)?;
            // SAFETY: PR_SET_PDEATHSIG accepts a signal scalar and unused zero
            // arguments, with no pointers. Supply native-word variadic values
            // and all six syscall argument slots. The libc syscall trampoline
            // enters the kernel directly without allocation or library locks.
            let result = libc::syscall(
                libc::SYS_prctl,
                libc::PR_SET_PDEATHSIG as libc::c_ulong,
                libc::SIGKILL as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            );
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            parent_matches(expected)
        });
    }
    Ok(())
}
pub(crate) fn verify_parent_death(expected: u32) -> io::Result<()> {
    let expected = libc::pid_t::try_from(expected)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: synchronous scalar identity queries; no pointers or mutation.
    let main_thread = unsafe { libc::gettid() == libc::getpid() };
    if !main_thread {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    parent_matches(expected)?;
    let mut signal: libc::c_int = 0;
    // SAFETY: GET writes exactly one aligned, exclusively borrowed initialized
    // int synchronously, and retains no pointer. Other arguments are zero.
    let result = unsafe {
        libc::prctl(
            libc::PR_GET_PDEATHSIG,
            &mut signal as *mut libc::c_int,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    if signal != libc::SIGKILL {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    parent_matches(expected)
}
fn group_matches(
    pid: libc::pid_t,
    own_group: libc::pid_t,
    parent_group: libc::pid_t,
    own_session: libc::pid_t,
    parent_session: libc::pid_t,
) -> bool {
    pid > 0
        && own_group == pid
        && parent_group > 0
        && parent_group != pid
        && own_session > 0
        && own_session == parent_session
}
fn positive_pid(value: libc::pid_t) -> io::Result<libc::pid_t> {
    if value < 0 {
        return Err(io::Error::last_os_error());
    }
    if value == 0 {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    Ok(value)
}
pub(crate) fn verify_isolated_group(expected: u32) -> io::Result<()> {
    let expected =
        libc::pid_t::try_from(expected).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    if expected <= 0 {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    // SAFETY: synchronous read-only scalar process identity; no pointers or mutation.
    let pid = positive_pid(unsafe { libc::getpid() })?;
    // SAFETY: synchronous read-only current process group query.
    let own_group = positive_pid(unsafe { libc::getpgrp() })?;
    // SAFETY: read-only query for the previously verified positive parent PID.
    let parent_group = positive_pid(unsafe { libc::getpgid(expected) })?;
    // SAFETY: zero selects the current process's session, no mutation.
    let own_session = positive_pid(unsafe { libc::getsid(0) })?;
    // SAFETY: read-only session query for verified parent, no retained resource.
    let parent_session = positive_pid(unsafe { libc::getsid(expected) })?;
    if !group_matches(pid, own_group, parent_group, own_session, parent_session) {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    parent_matches(expected)
}
pub(crate) fn monotonic_now() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: exclusively borrowed, aligned initialized timespec lives for the
    // synchronous CLOCK_MONOTONIC query. Kernel/libc retains no pointer.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let seconds = u64::try_from(time.tv_sec).ok();
    let nanos = u64::try_from(time.tv_nsec)
        .ok()
        .filter(|value| *value < 1_000_000_000);
    seconds
        .zip(nanos)
        .and_then(|(seconds, nanos)| seconds.checked_mul(1_000_000_000)?.checked_add(nanos))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid monotonic clock"))
}
pub(crate) fn wait_ready(
    requests: &[crate::reactor::WaitRequest<'_>],
    timeout_ns: u64,
) -> io::Result<Option<[i16; crate::reactor::MAX_WAIT_FDS]>> {
    use crate::reactor::{Interest, MAX_WAIT_FDS, MAX_WAIT_NS};
    if requests.len() > MAX_WAIT_FDS || timeout_ns > MAX_WAIT_NS {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let mut descriptors = [libc::pollfd {
        fd: -1,
        events: 0,
        revents: 0,
    }; MAX_WAIT_FDS];
    for (slot, request) in descriptors.iter_mut().zip(requests) {
        slot.fd = request.fd.as_raw_fd();
        slot.events = match request.interest {
            Interest::Errors => 0,
            Interest::Read => libc::POLLIN,
            Interest::Write => libc::POLLOUT,
        };
    }
    let timeout = libc::timespec {
        tv_sec: (timeout_ns / 1_000_000_000) as libc::time_t,
        tv_nsec: (timeout_ns % 1_000_000_000) as libc::c_long,
    };
    // SAFETY: descriptors is initialized aligned stack storage with exactly
    // requests.len() live borrowed FDs; owners cannot be mutated/closed through
    // these borrows during this call. libc ppoll synchronously updates only
    // that prefix, borrows the valid finite timespec, and retains no pointers.
    // Null sigmask leaves the thread signal mask unchanged.
    let result = unsafe {
        libc::ppoll(
            descriptors.as_mut_ptr(),
            requests.len() as libc::nfds_t,
            &timeout,
            std::ptr::null(),
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted {
            Ok(None)
        } else {
            Err(error)
        };
    }
    if result as usize
        != descriptors[..requests.len()]
            .iter()
            .filter(|slot| slot.revents != 0)
            .count()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid ppoll readiness count",
        ));
    }
    Ok(Some(descriptors.map(|slot| slot.revents)))
}
pub(crate) fn evdev_grab(fd: BorrowedFd<'_>, grabbed: bool) -> io::Result<()> {
    scalar(fd, request(1, 0x90, 4), u32::from(grabbed))
}
pub(crate) fn uinput_bit(fd: BorrowedFd<'_>, number: u8, bit: u16) -> io::Result<()> {
    let valid = match number {
        100 => matches!(bit, 1 | 17 | 20),
        101 => (1..=0x2ff).contains(&bit),
        105 => bit <= 2,
        _ => false,
    };
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid uinput capability",
        ));
    }
    scalar(fd, request_for(0x55, 1, number, 4), u32::from(bit))
}
pub(crate) fn uinput_lifecycle(fd: BorrowedFd<'_>, create: bool) -> io::Result<()> {
    scalar(fd, request_for(0x55, 0, if create { 1 } else { 2 }, 0), 0)
}
pub(crate) fn uinput_version(fd: BorrowedFd<'_>) -> io::Result<u32> {
    #[repr(C, align(4))]
    struct Buffer([u8; 4]);
    let mut bytes = Buffer([0; 4]);
    if call(fd, request_for(0x55, 2, 45, 4), &mut bytes.0)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected uinput version result",
        ));
    }
    Ok(u32::from_ne_bytes(bytes.0))
}
pub(crate) fn uinput_setup(fd: BorrowedFd<'_>, setup: [u8; 92]) -> io::Result<()> {
    #[repr(C, align(4))]
    struct Buffer([u8; 92]);
    let mut bytes = Buffer(setup);
    if call(fd, request_for(0x55, 1, 3, 92), &mut bytes.0)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected uinput setup result",
        ));
    }
    Ok(())
}
pub(crate) fn uinput_sysname(fd: BorrowedFd<'_>) -> io::Result<String> {
    let mut bytes = [0; 128];
    let count = call(fd, request_for(0x55, 2, 44, 128), &mut bytes)?;
    if count < 2 || count > bytes.len() || bytes[count - 1] != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid uinput sysname length",
        ));
    }
    let name = std::str::from_utf8(&bytes[..count - 1])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if !name.strip_prefix("input").is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid uinput sysname",
        ));
    }
    Ok(name.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, os::fd::AsFd};
    #[test]
    fn isolated_group_requires_child_leader_distinct_parent_and_same_session() {
        assert!(group_matches(12, 12, 10, 9, 9));
        for args in [
            (12, 10, 10, 9, 9),
            (12, 12, 12, 9, 9),
            (12, 12, 10, 12, 9),
            (0, 0, 10, 9, 9),
            (12, 12, 0, 9, 9),
            (12, 12, 10, 0, 0),
        ] {
            assert!(!group_matches(args.0, args.1, args.2, args.3, args.4));
        }
    }
    #[test]
    fn uapi_command_encoding_matches_audited_64_bit_linux() {
        assert_eq!(request(2, 1, 4), 0x80044501);
        assert_eq!(request(2, 2, 8), 0x80084502);
        assert_eq!(request(2, 0x18, 96), 0x80604518);
        assert_eq!(request(1, 0xa0, 4), 0x400445a0);
        assert_eq!(request(1, 0x90, 4), 0x40044590);
    }
    #[test]
    fn non_evdev_fd_returns_real_errno_instead_of_empty_state() {
        let file =
            File::open("/dev/null").unwrap_or_else(|error| panic!("cannot open null: {error}"));
        let error = fixed(file.as_fd(), 2, 8)
            .err()
            .unwrap_or_else(|| panic!("unexpected ioctl success"));
        assert_eq!(error.raw_os_error(), Some(libc::ENOTTY));
    }
    #[test]
    fn progress_pipe_copies_are_cloexec_nonblocking_and_owned_without_taking_stdio() {
        use std::io::{Read, Write};
        let (reader, mut peer_writer) = io::pipe().unwrap_or_else(|error| panic!("{error}"));
        let (mut peer_reader, writer) = io::pipe().unwrap_or_else(|error| panic!("{error}"));
        nonblocking(reader.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        nonblocking(writer.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        let (mut input, mut output) = duplicate_progress_pipes(reader.as_fd(), writer.as_fd())
            .unwrap_or_else(|error| panic!("{error}"));
        for fd in [input.as_fd(), output.as_fd()] {
            // SAFETY: synchronous scalar FD_CLOEXEC query of a live owned File;
            // this test neither closes, changes nor exports its raw descriptor.
            let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
            assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
        }
        assert!(pipe_identity(input.as_fd(), false).is_ok());
        assert!(pipe_identity(output.as_fd(), true).is_ok());
        assert_eq!(
            input.read(&mut [0]).err().map(|error| error.kind()),
            Some(io::ErrorKind::WouldBlock)
        );
        peer_writer
            .write_all(b"c")
            .unwrap_or_else(|error| panic!("{error}"));
        let mut bytes = [0];
        input
            .read_exact(&mut bytes)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bytes, *b"c");
        output
            .write_all(b"r")
            .unwrap_or_else(|error| panic!("{error}"));
        peer_reader
            .read_exact(&mut bytes)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bytes, *b"r");
        drop((input, output));
        assert!(pipe_identity(reader.as_fd(), false).is_ok());
        assert!(pipe_identity(writer.as_fd(), true).is_ok());
    }
    #[test]
    fn progress_channel_rejects_blocking_reversed_and_non_pipe_descriptors() {
        let (reader, writer) = io::pipe().unwrap_or_else(|error| panic!("{error}"));
        let (other_reader, other_writer) = io::pipe().unwrap_or_else(|error| panic!("{error}"));
        assert!(duplicate_progress_pipes(reader.as_fd(), other_writer.as_fd()).is_err());
        nonblocking(reader.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        nonblocking(other_writer.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        assert!(duplicate_progress_pipes(reader.as_fd(), other_writer.as_fd()).is_ok());
        nonblocking(writer.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        nonblocking(other_reader.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        assert!(duplicate_progress_pipes(writer.as_fd(), other_reader.as_fd()).is_err());
        let null = File::open("/dev/null").unwrap_or_else(|error| panic!("{error}"));
        assert!(duplicate_progress_pipes(null.as_fd(), other_writer.as_fd()).is_err());
    }
    #[test]
    fn progress_channel_rejects_same_pipe_loopback_before_copying() {
        let (reader, writer) = io::pipe().unwrap_or_else(|error| panic!("{error}"));
        nonblocking(reader.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        nonblocking(writer.as_fd()).unwrap_or_else(|error| panic!("{error}"));
        let error = duplicate_progress_pipes(reader.as_fd(), writer.as_fd())
            .err()
            .unwrap_or_else(|| panic!("loopback accepted"));
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(pipe_identity(reader.as_fd(), false).is_ok());
        assert!(pipe_identity(writer.as_fd(), true).is_ok());
    }
}
