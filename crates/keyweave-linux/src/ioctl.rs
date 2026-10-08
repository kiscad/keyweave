//! The private Linux ioctl/file-flag FFI boundary; no arbitrary public requests.
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
}
