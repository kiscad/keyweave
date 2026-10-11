//! Owned configuration-directory hints, not content, authority or activation.
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read},
    os::unix::ffi::OsStrExt,
    path::Path,
};
const HEADER: usize = 16;
const BUFFER: usize = 4096;
const CHANGES: u32 = libc::IN_MODIFY
    | libc::IN_ATTRIB
    | libc::IN_CLOSE_WRITE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_CREATE
    | libc::IN_DELETE;
const LOST: u32 = libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_UNMOUNT | libc::IN_IGNORED;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hint {
    pub changed: bool,
    pub lost: bool,
}
pub struct DirectoryNotice {
    file: File,
    watch: i32,
    name: Box<[u8]>,
}
impl DirectoryNotice {
    /// Cold filesystem lookup only. Watch the selected name's parent directory,
    /// so replacing the file does not require re-registering its old inode.
    pub fn new(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("notice path must be absolute"));
        }
        let name = path
            .file_name()
            .ok_or_else(|| invalid("notice filename missing"))?
            .as_bytes();
        let parent = path
            .parent()
            .ok_or_else(|| invalid("notice directory missing"))?;
        let directory = CString::new(parent.as_os_str().as_bytes())
            .map_err(|_| invalid("notice directory invalid"))?;
        let (fd, watch) = crate::ioctl::config_directory_notice(&directory, CHANGES | LOST)?;
        Ok(Self {
            file: File::from(fd),
            watch,
            name: name.into(),
        })
    }
    /// At most one nonblocking read and 4096 bytes decoded. No file/path lookup.
    pub fn poll(&mut self) -> io::Result<Hint> {
        let mut buffer = [0; BUFFER];
        match self.file.read(&mut buffer) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "notice stream ended",
            )),
            Ok(n) => decode(&buffer[..n], self.watch, &self.name),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(Hint::default())
            }
            Err(e) => Err(e),
        }
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid directory notice")
}
fn decode(mut bytes: &[u8], watch: i32, selected: &[u8]) -> io::Result<Hint> {
    let mut hint = Hint::default();
    while !bytes.is_empty() {
        if bytes.len() < HEADER {
            return Err(malformed());
        }
        let wd = i32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let mask = u32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let len = u32::from_ne_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
        if len > bytes.len() - HEADER || !len.is_multiple_of(4) {
            return Err(malformed());
        }
        let padded = &bytes[HEADER..HEADER + len];
        let name = if padded.is_empty() {
            &[][..]
        } else {
            let end = padded.iter().position(|b| *b == 0).ok_or_else(malformed)?;
            if padded[end..].iter().any(|b| *b != 0) {
                return Err(malformed());
            }
            &padded[..end]
        };
        if mask & libc::IN_Q_OVERFLOW != 0 {
            if wd != -1 || !name.is_empty() {
                return Err(malformed());
            }
            hint.changed = true;
        } else {
            if wd != watch {
                return Err(malformed());
            }
            if mask & LOST != 0 {
                hint.lost = true;
                hint.changed = true;
            }
            if name == selected && mask & CHANGES != 0 {
                hint.changed = true;
            }
        }
        bytes = &bytes[HEADER + len..];
    }
    Ok(hint)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn record(wd: i32, mask: u32, name: &[u8]) -> Vec<u8> {
        let len = if name.is_empty() {
            0
        } else {
            (name.len() + 1).div_ceil(16) * 16
        };
        let mut bytes = vec![0; HEADER + len];
        bytes[..4].copy_from_slice(&wd.to_ne_bytes());
        bytes[4..8].copy_from_slice(&mask.to_ne_bytes());
        bytes[12..16].copy_from_slice(&(len as u32).to_ne_bytes());
        bytes[HEADER..HEADER + name.len()].copy_from_slice(name);
        bytes
    }
    #[test]
    fn mixed_names_overflow_and_watch_loss_are_hints_not_content() {
        let mut bytes = record(7, libc::IN_CLOSE_WRITE, b"other");
        assert_eq!(decode(&bytes, 7, b"selected").unwrap(), Hint::default());
        bytes.extend(record(7, libc::IN_MOVED_TO, b"selected"));
        assert_eq!(
            decode(&bytes, 7, b"selected").unwrap(),
            Hint {
                changed: true,
                lost: false
            }
        );
        assert_eq!(
            decode(&record(-1, libc::IN_Q_OVERFLOW, b""), 7, b"selected").unwrap(),
            Hint {
                changed: true,
                lost: false
            }
        );
        assert_eq!(
            decode(&record(7, libc::IN_IGNORED, b""), 7, b"selected").unwrap(),
            Hint {
                changed: true,
                lost: true
            }
        );
        assert!(decode(&record(8, libc::IN_MODIFY, b"selected"), 7, b"selected").is_err());
    }
    #[test]
    fn truncated_records_bad_lengths_names_and_overflow_are_rejected() {
        let good = record(7, libc::IN_MODIFY, b"selected");
        for n in 1..good.len() {
            assert!(decode(&good[..n], 7, b"selected").is_err());
        }
        let mut bad = good.clone();
        bad[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(decode(&bad, 7, b"selected").is_err());
        bad = good.clone();
        bad[HEADER..].fill(b'x');
        assert!(decode(&bad, 7, b"selected").is_err());
        bad = good;
        bad[HEADER + 10] = b'x';
        assert!(decode(&bad, 7, b"selected").is_err());
        assert!(decode(&record(7, libc::IN_Q_OVERFLOW, b""), 7, b"selected").is_err());
    }
    struct Dir(std::path::PathBuf);
    impl Dir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "keyweave-notice-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn changed(notice: &mut DirectoryNotice) -> Hint {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let hint = notice.poll().unwrap();
            if hint.changed {
                return hint;
            }
            assert!(std::time::Instant::now() < end);
            std::thread::yield_now();
        }
    }
    #[test]
    fn own_directory_reports_write_atomic_replace_nonunicode_and_delete() {
        use std::{
            fs,
            os::{fd::AsRawFd, unix::ffi::OsStringExt},
        };
        let dir = Dir::new();
        let path = dir.0.join(std::ffi::OsString::from_vec(vec![b'c', 0xff]));
        let mut notice = DirectoryNotice::new(&path).unwrap();
        let info =
            fs::read_to_string(format!("/proc/self/fdinfo/{}", notice.file.as_raw_fd())).unwrap();
        let flags = u32::from_str_radix(
            info.lines()
                .find(|l| l.starts_with("flags:"))
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap(),
            8,
        )
        .unwrap();
        assert_ne!(flags & libc::O_CLOEXEC as u32, 0);
        assert_ne!(flags & libc::O_NONBLOCK as u32, 0);
        assert_eq!(notice.poll().unwrap(), Hint::default());
        fs::write(dir.0.join("unrelated"), b"x").unwrap();
        assert_eq!(notice.poll().unwrap(), Hint::default());
        fs::write(&path, b"a").unwrap();
        assert!(changed(&mut notice).changed);
        let replacement = dir.0.join("replacement");
        fs::write(&replacement, b"b").unwrap();
        fs::rename(replacement, &path).unwrap();
        assert!(changed(&mut notice).changed);
        fs::remove_file(&path).unwrap();
        assert!(changed(&mut notice).changed);
    }
    #[test]
    fn parent_removal_is_lost_and_relative_registration_is_refused() {
        let dir = Dir::new();
        let mut notice = DirectoryNotice::new(&dir.0.join("absent")).unwrap();
        std::fs::remove_dir(&dir.0).unwrap();
        assert!(changed(&mut notice).lost);
        assert!(DirectoryNotice::new(Path::new("relative")).is_err());
    }
}
