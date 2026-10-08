//! The only Linux ioctl FFI boundary; no public arbitrary request interface.
#![allow(unsafe_code)]
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd},
};
const MAX_BUFFER: usize = 4096;
pub(crate) const fn request(direction: u32, number: u8, size: usize) -> u32 {
    (direction << 30) | ((size as u32) << 16) | (0x45 << 8) | number as u32
}
fn call(fd: BorrowedFd<'_>, command: u32, buffer: &mut [u8]) -> io::Result<usize> {
    if buffer.is_empty()
        || buffer.len() > MAX_BUFFER
        || ((command >> 16) & 0x3fff) as usize != buffer.len()
        || ((command >> 8) & 0xff) != 0x45
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid ioctl buffer",
        ));
    }
    // SAFETY: fd is borrowed for the full synchronous call. All callers use
    // audited evdev requests encoding exactly buffer.len() bytes, initialized
    // and exclusively borrowed here. Byte-oriented queries copy within the
    // encoded range; scalar/version and clock requests use aligned storage.
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
