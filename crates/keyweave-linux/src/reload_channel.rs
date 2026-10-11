//! Owned independent reload pipes. Cold construction/admission only.
#![allow(unsafe_code)]
use std::{
    fs::File,
    io,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    process::Command,
};
pub(crate) const REQUEST_ENV: &str = "KEYWEAVE_RELOAD_REQUEST_FD";
pub(crate) const ACK_ENV: &str = "KEYWEAVE_RELOAD_ACK_FD";

/// Nonblocking owned ends; no Clone and no protocol/time/progress state.
pub struct ParentEndpoints {
    reader: File,
    writer: File,
}
pub struct WorkerEndpoints {
    reader: File,
    writer: File,
}
impl ParentEndpoints {
    pub fn into_parts(self) -> (File, File) {
        (self.reader, self.writer)
    }
}
impl WorkerEndpoints {
    pub fn into_parts(self) -> (File, File) {
        (self.reader, self.writer)
    }
    pub(crate) fn command_inherit(self, command: &mut Command) -> io::Result<(i32, i32)> {
        use std::os::unix::process::CommandExt;
        let reader = duplicate(self.reader.as_raw_fd())?;
        let writer = duplicate(self.writer.as_raw_fd())?;
        let raw = (reader.as_raw_fd(), writer.as_raw_fd());
        // SAFETY: closure retains its uniquely owned duplicates above stdio.
        // Child uses only live scalar FDs/direct fcntl; no allocation, locks,
        // environment access, IO, credentials, or Drop in this hook.
        unsafe {
            command.pre_exec(move || {
                for (file, direction) in [(&reader, libc::O_RDONLY), (&writer, libc::O_WRONLY)] {
                    let fd = file.as_raw_fd();
                    let flags = libc::fcntl(fd, libc::F_GETFL);
                    if flags < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if flags & libc::O_NONBLOCK == 0 || flags & libc::O_ACCMODE != direction {
                        return Err(io::Error::from_raw_os_error(libc::EPERM));
                    }
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Ok(raw)
    }
    pub(crate) fn from_inherited(read: i32, write: i32) -> io::Result<Self> {
        let reader = duplicate(read)?;
        let writer = duplicate(write)?;
        if crate::ioctl::pipe_identity(reader.as_fd(), false)?
            == crate::ioctl::pipe_identity(writer.as_fd(), true)?
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reload pipes must be distinct",
            ));
        }
        Ok(Self { reader, writer })
    }
    pub(crate) fn reject_reserved(
        &self,
        reserved: &[(libc::dev_t, libc::ino_t)],
    ) -> io::Result<()> {
        if self.identities()?.iter().any(|id| reserved.contains(id)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reload overlaps reserved protocol",
            ));
        }
        Ok(())
    }
    pub(crate) fn identities(&self) -> io::Result<[(libc::dev_t, libc::ino_t); 2]> {
        Ok([
            crate::ioctl::pipe_identity(self.reader.as_fd(), false)?,
            crate::ioctl::pipe_identity(self.writer.as_fd(), true)?,
        ])
    }
}
fn duplicate(raw: i32) -> io::Result<File> {
    if raw < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "reload descriptor overlaps stdio",
        ));
    }
    // SAFETY: scalar lookup; no BorrowedFd is constructed for an unchecked
    // inherited number. Failure never owns/closes it; success is fresh CLOEXEC.
    let copy = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 3) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fresh successful duplicate with one Rust owner.
    Ok(unsafe { File::from_raw_fd(copy) })
}
/// Cold parent only. std pipe creates CLOEXEC; all four ends are NONBLOCK.
/// Partial construction failure drops every owned end. These are not devices.
pub fn channel() -> io::Result<(ParentEndpoints, WorkerEndpoints)> {
    let (requests, request_writer) = io::pipe()?;
    let (acks, ack_writer) = io::pipe()?;
    for fd in [
        requests.as_fd(),
        request_writer.as_fd(),
        acks.as_fd(),
        ack_writer.as_fd(),
    ] {
        crate::ioctl::nonblocking(fd)?;
    }
    Ok((
        ParentEndpoints {
            reader: File::from(OwnedFd::from(acks)),
            writer: File::from(OwnedFd::from(request_writer)),
        },
        WorkerEndpoints {
            reader: File::from(OwnedFd::from(requests)),
            writer: File::from(OwnedFd::from(ack_writer)),
        },
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::{Read, Write};
    fn cloexec(file: &File) {
        // SAFETY: live owned FD, scalar no-argument lookup.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }
    #[test]
    fn owned_pair_is_nonblocking_cloexec_distinct_and_bidirectional() {
        let (parent, worker) = channel().unwrap();
        let (mut pr, mut pw) = parent.into_parts();
        let (mut wr, mut ww) = worker.into_parts();
        for file in [&pr, &pw, &wr, &ww] {
            cloexec(file);
        }
        let mut byte = [0];
        assert_eq!(
            wr.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            pr.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        pw.write_all(b"q").unwrap();
        assert_eq!(wr.read(&mut byte).unwrap(), 1);
        assert_eq!(byte, *b"q");
        ww.write_all(b"a").unwrap();
        assert_eq!(pr.read(&mut byte).unwrap(), 1);
        assert_eq!(byte, *b"a");
        drop(pw);
        assert_eq!(wr.read(&mut byte).unwrap(), 0);
        drop(ww);
        assert_eq!(pr.read(&mut byte).unwrap(), 0);
    }
    #[test]
    fn either_reserved_pipe_overlap_rejects_before_handoff() {
        let (_parent, worker) = channel().unwrap();
        let ids = worker.identities().unwrap();
        for reserved in [vec![ids[0]], vec![ids[1]], ids.to_vec()] {
            assert_eq!(
                worker.reject_reserved(&reserved).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        let (_other_parent, other) = channel().unwrap();
        worker
            .reject_reserved(&other.identities().unwrap())
            .unwrap();
    }
    #[test]
    fn admission_rejects_wrong_direction_blocking_nonpipe_and_shared_pipe() {
        let (_parent, worker) = channel().unwrap();
        let (read, write) = worker.into_parts();
        let copy = WorkerEndpoints::from_inherited(read.as_raw_fd(), write.as_raw_fd()).unwrap();
        assert_eq!(
            copy.identities().unwrap(),
            [
                crate::ioctl::pipe_identity(read.as_fd(), false).unwrap(),
                crate::ioctl::pipe_identity(write.as_fd(), true).unwrap()
            ]
        );
        let (a, b) = copy.into_parts();
        cloexec(&a);
        cloexec(&b);
        assert!(WorkerEndpoints::from_inherited(write.as_raw_fd(), read.as_raw_fd()).is_err());
        assert!(WorkerEndpoints::from_inherited(0, write.as_raw_fd()).is_err());
        assert!(WorkerEndpoints::from_inherited(i32::MAX, write.as_raw_fd()).is_err());
        let (r, w) = io::pipe().unwrap();
        assert!(WorkerEndpoints::from_inherited(r.as_raw_fd(), w.as_raw_fd()).is_err());
        crate::ioctl::nonblocking(r.as_fd()).unwrap();
        crate::ioctl::nonblocking(w.as_fd()).unwrap();
        assert!(WorkerEndpoints::from_inherited(r.as_raw_fd(), w.as_raw_fd()).is_err());
        let null = File::open("/dev/null").unwrap();
        assert!(WorkerEndpoints::from_inherited(null.as_raw_fd(), write.as_raw_fd()).is_err());
        // Originals remain owned/live after every failed duplicate admission.
        assert!(crate::ioctl::pipe_identity(read.as_fd(), false).is_ok());
    }
}
