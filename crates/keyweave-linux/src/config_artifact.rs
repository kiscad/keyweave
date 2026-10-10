//! Kernel-sealed configuration image, not child admission or authentication.
#![allow(unsafe_code)]
use std::{
    fs::File,
    io::{self, Write},
    os::{
        fd::{AsFd, AsRawFd, FromRawFd},
        unix::fs::FileExt,
    },
};
pub const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
const REQUIRED_SEALS: i32 =
    libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn seals(file: &File) -> io::Result<i32> {
    // SAFETY: borrowed live descriptor, scalar fcntl command without arg.
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}
pub struct ConfigArtifact {
    file: File,
    len: usize,
}
impl std::fmt::Debug for ConfigArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxConfigArtifact")
            .field("byte_length", &self.len)
            .finish_non_exhaustive()
    }
}
impl ConfigArtifact {
    /// Copies exact bytes then seals against writes, resize and further seals.
    /// CLOEXEC is retained; inherited delivery must be selected by a supervisor.
    pub fn create(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(invalid("configuration image too large"));
        }
        // SAFETY: static NUL-terminated name, known scalar flags. Returned FD
        // has no Rust owner until the immediately following File construction.
        let fd = unsafe {
            libc::memfd_create(
                c"keyweave-config".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fresh memfd is owned exactly once, no aliases.
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes)?;
        // SAFETY: borrowed live memfd, scalar seal mask, no pointers/mapping.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, REQUIRED_SEALS) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Self::admit(file)
    }
    /// Takes an already owned descriptor. This verifies immutable bounded
    /// bytes, not sender identity, exclusivity or the validity of the config.
    pub fn admit(file: File) -> io::Result<Self> {
        // SAFETY: borrowed live descriptor; scalar no-argument fcntl command.
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if !matches!(flags & libc::O_ACCMODE, libc::O_RDONLY | libc::O_RDWR) {
            return Err(invalid("configuration image is not readable"));
        }
        // Check seals before size; once all required seals exist they cannot
        // be removed. No size-before-seal race is treated as verified content.
        if seals(&file)? & REQUIRED_SEALS != REQUIRED_SEALS {
            return Err(invalid("configuration image is not fully sealed"));
        }
        let metadata = file.metadata()?;
        let len = usize::try_from(metadata.len())
            .map_err(|_| invalid("configuration image too large"))?;
        if !metadata.is_file() || len > MAX_ARTIFACT_BYTES {
            return Err(invalid("invalid configuration image size/type"));
        }
        Ok(Self { file, len })
    }
    /// Positional reads don't share an offset with inherited/duplicated FDs.
    pub fn read_bytes(&self) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; self.len];
        self.file.read_exact_at(&mut bytes, 0)?;
        Ok(bytes)
    }
    pub fn byte_length(&self) -> usize {
        self.len
    }
    pub fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.file.as_fd()
    }
    pub fn into_file(self) -> File {
        self.file
    }
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::io::{Read, Seek, SeekFrom};
    #[test]
    fn real_seals_reject_write_resize_and_additional_seals() {
        let artifact = ConfigArtifact::create(b"private config").unwrap();
        assert_eq!(
            seals(&artifact.file).unwrap() & REQUIRED_SEALS,
            REQUIRED_SEALS
        );
        let mut alias = artifact.file.try_clone().unwrap();
        assert_eq!(
            alias.write(b"replacement").unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            alias.set_len(0).unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        assert_eq!(
            alias.set_len(100).unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        // SAFETY: own live memfd alias and scalar command/mask only.
        assert_eq!(
            unsafe { libc::fcntl(alias.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_WRITE) },
            -1
        );
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
        assert_eq!(artifact.read_bytes().unwrap(), b"private config");
        assert!(!format!("{artifact:?}").contains("private"));
        // SAFETY: borrowed live FD and scalar no-argument command.
        assert_ne!(
            unsafe { libc::fcntl(artifact.as_fd().as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
    #[test]
    fn positional_reader_survives_duplicate_offset_changes_and_parent_drop() {
        let artifact = ConfigArtifact::create(b"exact\0bytes").unwrap();
        let mut alias = artifact.file.try_clone().unwrap();
        let admitted = ConfigArtifact::admit(artifact.into_file()).unwrap();
        alias.seek(SeekFrom::End(0)).unwrap();
        assert_eq!(admitted.read_bytes().unwrap(), b"exact\0bytes");
        let mut byte = [0];
        assert_eq!(alias.read(&mut byte).unwrap(), 0);
        drop(alias);
        assert_eq!(admitted.read_bytes().unwrap(), b"exact\0bytes");
    }
    #[test]
    fn exact_limit_and_empty_images_are_bounded_without_truncation() {
        assert_eq!(
            ConfigArtifact::create(&[]).unwrap().read_bytes().unwrap(),
            b""
        );
        let bytes = vec![71; MAX_ARTIFACT_BYTES];
        assert_eq!(
            ConfigArtifact::create(&bytes)
                .unwrap()
                .read_bytes()
                .unwrap(),
            bytes
        );
        assert!(ConfigArtifact::create(&vec![0; MAX_ARTIFACT_BYTES + 1]).is_err());
    }
    #[test]
    fn unsealed_owned_memory_file_is_rejected() {
        // SAFETY: static name and known flags; ownership established once below.
        let fd = unsafe {
            libc::memfd_create(
                c"keyweave-unsealed-test".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        assert!(fd >= 0);
        // SAFETY: newly created unaliased descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        assert_eq!(
            ConfigArtifact::admit(file).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn seal_verification_does_not_accept_a_write_only_alias() {
        let artifact = ConfigArtifact::create(b"sealed").unwrap();
        let path = format!("/proc/self/fd/{}", artifact.file.as_raw_fd());
        let write_only = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert_eq!(
            ConfigArtifact::admit(write_only).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let read_only = File::open(path).unwrap();
        let admitted = ConfigArtifact::admit(read_only).unwrap();
        drop(artifact);
        assert_eq!(admitted.read_bytes().unwrap(), b"sealed");
    }
}
