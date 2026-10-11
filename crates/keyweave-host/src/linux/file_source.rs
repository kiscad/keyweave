//! Single background file-read owner; no parser, actor, progress or reload IO.
use std::{
    fs::{self, Metadata},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    thread::JoinHandle,
};
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceError {
    Busy,
    Closed,
    Path,
    Io {
        kind: io::ErrorKind,
        errno: Option<i32>,
    },
    Length,
    Changed,
    Spawn,
    Panicked,
    Comparison,
}
impl From<io::Error> for SourceError {
    fn from(error: io::Error) -> Self {
        Self::Io {
            kind: error.kind(),
            errno: error.raw_os_error(),
        }
    }
}
impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "configuration source failed: {self:?}")
    }
}
impl std::error::Error for SourceError {}
pub struct Snapshot {
    bytes: Box<[u8]>,
    comparison: Option<Match>,
}
type Baselines = (std::sync::Arc<[u8]>, std::sync::Arc<[u8]>);
enum Match {
    Seen,
    Active,
    Changed(std::sync::Arc<[u8]>),
}
pub enum Compared {
    Seen,
    Active,
    Changed {
        body: Box<[u8]>,
        retained: std::sync::Arc<[u8]>,
    },
}
impl Snapshot {
    pub fn into_comparison(self) -> Result<Compared, SourceError> {
        match self.comparison {
            Some(Match::Seen) => Ok(Compared::Seen),
            Some(Match::Active) => Ok(Compared::Active),
            Some(Match::Changed(retained)) => Ok(Compared::Changed {
                body: self.bytes,
                retained,
            }),
            None => Err(SourceError::Comparison),
        }
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn into_bytes(self) -> Box<[u8]> {
        self.bytes
    }
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceSnapshot")
            .field("byte_length", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceStep {
    Idle,
    Reading,
    Ready,
    Closed,
}
struct JobResult {
    snapshot: Result<Snapshot, SourceError>,
    notice: Option<std::io::Result<keyweave_linux::config_notify::DirectoryNotice>>,
}
pub struct FileSource {
    path: PathBuf,
    job: Option<JoinHandle<JobResult>>,
    notice_attempt: bool,
    notice_enabled: bool,
    notice: Option<keyweave_linux::config_notify::DirectoryNotice>,
    notice_error: Option<SourceError>,
    ready: Option<Result<Snapshot, SourceError>>,
    closed: bool,
}
impl FileSource {
    /// Cold construction only. Preserve absolute OS path units; no lookup/IO.
    pub fn new(path: PathBuf) -> Result<Self, SourceError> {
        if !path.is_absolute() {
            return Err(SourceError::Path);
        }
        Ok(Self {
            path,
            job: None,
            notice_attempt: false,
            notice_enabled: false,
            notice: None,
            notice_error: None,
            ready: None,
            closed: false,
        })
    }
    pub fn with_notifications(path: PathBuf) -> Result<Self, SourceError> {
        let mut source = Self::new(path)?;
        source.notice_enabled = true;
        Ok(source)
    }
    pub fn notification_error(&self) -> Option<SourceError> {
        self.notice_error
    }
    /// Poll original job nonblocking, then at most one bounded notice read.
    /// Any unavailable/lost stream becomes polling-only, without re-registering.
    pub fn poll_notice(&mut self) -> bool {
        self.poll();
        if self.closed {
            return false;
        }
        let Some(notice) = self.notice.as_mut() else {
            return false;
        };
        match notice.poll() {
            Ok(hint) => {
                if hint.lost {
                    self.notice = None;
                    self.notice_error = Some(SourceError::Changed);
                }
                hint.changed
            }
            Err(error) => {
                self.notice = None;
                self.notice_error = Some(error.into());
                true
            }
        }
    }
    pub fn request(&mut self) -> Result<(), SourceError> {
        self.start(None)
    }
    /// Compare and duplicate candidate only on the cold thread. Neither arc
    /// is advanced here; caller owns accepted baseline and last-seen policy.
    /// Consume via Snapshot::into_comparison; Seen/Active snapshots have no body.
    pub fn request_compare(
        &mut self,
        seen: std::sync::Arc<[u8]>,
        active: std::sync::Arc<[u8]>,
    ) -> Result<(), SourceError> {
        self.start(Some((seen, active)))
    }
    fn start(&mut self, compare: Option<Baselines>) -> Result<(), SourceError> {
        if self.closed {
            return Err(SourceError::Closed);
        }
        if self.job.is_some() || self.ready.is_some() {
            return Err(SourceError::Busy);
        }
        let path = self.path.clone();
        let register = self.notice_enabled && !self.notice_attempt;
        self.notice_attempt |= register;
        match std::thread::Builder::new()
            .name("keyweave-source".into())
            .spawn(move || {
                let notice =
                    register.then(|| keyweave_linux::config_notify::DirectoryNotice::new(&path));
                let snapshot = (|| {
                    let mut snapshot = read_snapshot(&path)?;
                    if let Some((seen, active)) = compare {
                        snapshot.comparison = Some(if snapshot.bytes.as_ref() == seen.as_ref() {
                            Match::Seen
                        } else if snapshot.bytes.as_ref() == active.as_ref() {
                            Match::Active
                        } else {
                            Match::Changed(std::sync::Arc::from(snapshot.bytes.as_ref()))
                        });
                        if !matches!(snapshot.comparison, Some(Match::Changed(_))) {
                            snapshot.bytes = Box::new([]);
                        }
                    }
                    Ok(snapshot)
                })();
                JobResult { snapshot, notice }
            }) {
            Ok(job) => {
                self.job = Some(job);
                Ok(())
            }
            Err(_) => {
                self.close();
                Err(SourceError::Spawn)
            }
        }
    }
    /// Control-thread only; never join a still-running filesystem job.
    pub fn poll(&mut self) -> SourceStep {
        if self.job.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(job) = self.job.take()
        {
            match job.join() {
                Ok(result) if !self.closed => {
                    if let Some(notice) = result.notice {
                        match notice {
                            Ok(notice) => self.notice = Some(notice),
                            Err(error) => self.notice_error = Some(error.into()),
                        }
                    }
                    self.ready = Some(result.snapshot);
                }
                Err(_) if !self.closed => self.ready = Some(Err(SourceError::Panicked)),
                _ => {}
            }
        }
        if self.closed {
            SourceStep::Closed
        } else if self.ready.is_some() {
            SourceStep::Ready
        } else if self.job.is_some() {
            SourceStep::Reading
        } else {
            SourceStep::Idle
        }
    }
    /// Consume once, even a read error. Neither snapshot nor read success is a
    /// config commit; caller advances baseline only after actual worker ack.
    pub fn take(&mut self) -> Option<Result<Snapshot, SourceError>> {
        if self.closed { None } else { self.ready.take() }
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.notice = None;
        self.ready = None;
    }
    pub fn job_pending(&self) -> bool {
        self.job.is_some()
    }
    /// Keep same closed owner to reap late work; no waiting/restart/activation.
    pub fn reap_closed(&mut self) -> Result<bool, SourceError> {
        if !self.closed {
            return Err(SourceError::Busy);
        }
        self.poll();
        Ok(self.job.is_none())
    }
}
fn stamp(m: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.mode(),
    )
}
fn bounded_read(reader: impl Read) -> Result<Vec<u8>, SourceError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > MAX_SOURCE_BYTES {
        return Err(SourceError::Length);
    }
    Ok(bytes)
}
fn read_snapshot(path: &Path) -> Result<Snapshot, SourceError> {
    let file = keyweave_linux::config_source::open(path)?;
    let before = file.metadata()?;
    if before.len() == 0 || before.len() > MAX_SOURCE_BYTES as u64 {
        return Err(SourceError::Length);
    }
    let bytes = bounded_read(&file)?;
    finish_snapshot(path, &file, before, bytes)
}
fn finish_snapshot(
    path: &Path,
    file: &std::fs::File,
    before: Metadata,
    bytes: Vec<u8>,
) -> Result<Snapshot, SourceError> {
    let after = file.metadata()?;
    let current = fs::metadata(path)?;
    if !current.is_file()
        || stamp(&before) != stamp(&after)
        || stamp(&after) != stamp(&current)
        || bytes.len() as u64 != after.len()
    {
        return Err(SourceError::Changed);
    }
    // This detects ordinary replace/write races, not atomic/ABA consistency or
    // authenticated content. Exact bytes still require worker cold compilation.
    Ok(Snapshot {
        bytes: bytes.into_boxed_slice(),
        comparison: None,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "keyweave-source-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("private config.json")
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn ready(source: &mut FileSource) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if source.poll() == SourceStep::Ready {
                break;
            }
            assert!(Instant::now() < end, "same file job did not complete");
            std::thread::yield_now();
        }
    }
    #[test]
    fn exact_jsonc_raw_bytes_and_ready_gate_require_single_consumer() {
        let dir = Dir::new();
        let path = dir.file();
        let bytes = b"// private comment\n{private invalid schema}\xff";
        fs::write(&path, bytes).unwrap();
        let mut source = FileSource::new(path).unwrap();
        source.request().unwrap();
        ready(&mut source);
        assert_eq!(source.request().unwrap_err(), SourceError::Busy);
        let snapshot = source.take().unwrap().unwrap();
        assert_eq!(snapshot.bytes(), bytes);
        assert!(!format!("{snapshot:?}").contains("private"));
        assert_eq!(snapshot.into_bytes().as_ref(), bytes);
        assert!(source.take().is_none());
        assert_eq!(source.poll(), SourceStep::Idle);
        source.close();
        assert_eq!(source.request().unwrap_err(), SourceError::Closed);
        assert!(source.reap_closed().unwrap());
    }
    #[test]
    fn length_cap_is_inclusive_and_nonregular_missing_errors_exclude_path() {
        let dir = Dir::new();
        let path = dir.file();
        for len in [0, MAX_SOURCE_BYTES, MAX_SOURCE_BYTES + 1] {
            fs::write(&path, vec![b'x'; len]).unwrap();
            let result = read_snapshot(&path);
            if len == MAX_SOURCE_BYTES {
                assert_eq!(result.unwrap().bytes().len(), len)
            } else {
                assert_eq!(result.unwrap_err(), SourceError::Length)
            }
        }
        assert!(matches!(
            read_snapshot(&dir.0),
            Err(SourceError::Io {
                kind: io::ErrorKind::InvalidInput,
                ..
            })
        ));
        fs::remove_file(&path).unwrap();
        let error = read_snapshot(&path).unwrap_err();
        assert!(matches!(
            error,
            SourceError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            }
        ));
        assert!(!format!("{error:?}").contains("private"));
        assert!(!error.to_string().contains("private"));
    }
    #[test]
    fn ordinary_symlink_and_nonunicode_names_preserve_exact_bytes() {
        use std::os::unix::{ffi::OsStringExt, fs::symlink};
        let dir = Dir::new();
        let path = dir.0.join(std::ffi::OsString::from_vec(vec![b'c', 0xff]));
        fs::write(&path, b"exact").unwrap();
        let link = dir.0.join("selected.json");
        symlink(&path, &link).unwrap();
        assert_eq!(read_snapshot(&link).unwrap().bytes(), b"exact");
    }
    #[test]
    fn replaced_path_and_changed_file_cannot_publish_observed_old_bytes() {
        let dir = Dir::new();
        let path = dir.file();
        fs::write(&path, b"old").unwrap();
        let file = keyweave_linux::config_source::open(&path).unwrap();
        let before = file.metadata().unwrap();
        let bytes = bounded_read(&file).unwrap();
        let replacement = dir.0.join("new.json");
        fs::write(&replacement, b"new").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert_eq!(
            finish_snapshot(&path, &file, before, bytes).unwrap_err(),
            SourceError::Changed
        );
        let file = keyweave_linux::config_source::open(&path).unwrap();
        let before = file.metadata().unwrap();
        let bytes = bounded_read(&file).unwrap();
        fs::write(&path, b"longer").unwrap();
        assert_eq!(
            finish_snapshot(&path, &file, before, bytes).unwrap_err(),
            SourceError::Changed
        );
    }
    #[test]
    fn fifo_without_writer_returns_type_failure_without_open_wait() {
        let dir = Dir::new();
        let path = dir.file();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let mut source = FileSource::new(path).unwrap();
        source.request().unwrap();
        ready(&mut source);
        assert!(matches!(
            source.take().unwrap(),
            Err(SourceError::Io {
                kind: io::ErrorKind::InvalidInput,
                ..
            })
        ));
    }
    #[test]
    fn stop_retains_original_blocked_job_and_discards_late_result() {
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        let dir = Dir::new();
        let path = dir.file();
        let mut source = FileSource::with_notifications(path.clone()).unwrap();
        source.job = Some(std::thread::spawn(move || {
            let notice = keyweave_linux::config_notify::DirectoryNotice::new(&path).unwrap();
            wait.recv().unwrap();
            JobResult {
                snapshot: Ok(Snapshot {
                    bytes: Box::new(*b"late private body"),
                    comparison: None,
                }),
                notice: Some(Ok(notice)),
            }
        }));
        assert_eq!(source.request().unwrap_err(), SourceError::Busy);
        source.close();
        assert_eq!(source.poll(), SourceStep::Closed);
        assert!(source.job_pending());
        assert!(!source.reap_closed().unwrap());
        release.send(()).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !source.reap_closed().unwrap() {
            assert!(Instant::now() < end);
            std::thread::yield_now()
        }
        assert!(source.take().is_none());
        assert!(source.notice.is_none());
        assert!(!source.poll_notice());
        assert_eq!(source.poll(), SourceStep::Closed);
    }
    #[test]
    fn relative_path_and_panicked_job_never_produce_snapshot() {
        assert!(matches!(
            FileSource::new(PathBuf::from("relative")),
            Err(SourceError::Path)
        ));
        let mut source = FileSource::new(PathBuf::from("/private-test-source")).unwrap();
        source.job = Some(std::thread::spawn(|| panic!("owned test source panic")));
        ready(&mut source);
        assert_eq!(source.take().unwrap().unwrap_err(), SourceError::Panicked);
        assert_eq!(source.poll(), SourceStep::Idle);
    }
    #[test]
    fn comparison_is_exact_and_retains_only_changed_body_off_thread() {
        use std::sync::Arc;
        let dir = Dir::new();
        let path = dir.file();
        let active: Arc<[u8]> = Arc::from(b"old\xff".as_slice());
        let seen: Arc<[u8]> = Arc::from(b"rejected".as_slice());
        let mut source = FileSource::new(path.clone()).unwrap();
        for (bytes, expected) in [
            (b"rejected".as_slice(), 0),
            (b"old\xff".as_slice(), 1),
            (b"new\xff".as_slice(), 2),
        ] {
            fs::write(&path, bytes).unwrap();
            source
                .request_compare(seen.clone(), active.clone())
                .unwrap();
            ready(&mut source);
            match source.take().unwrap().unwrap().into_comparison().unwrap() {
                Compared::Seen => assert_eq!(expected, 0),
                Compared::Active => assert_eq!(expected, 1),
                Compared::Changed { body, retained } => {
                    assert_eq!(expected, 2);
                    assert_eq!(body.as_ref(), bytes);
                    assert_eq!(retained.as_ref(), bytes);
                }
            }
        }
        source.request().unwrap();
        ready(&mut source);
        assert!(matches!(
            source.take().unwrap().unwrap().into_comparison(),
            Err(SourceError::Comparison)
        ));
    }
    #[test]
    fn unavailable_directory_registration_is_one_attempt_with_polling_fallback() {
        let dir = Dir::new();
        let parent = dir.0.join("missing");
        let path = parent.join("config");
        let mut source = FileSource::with_notifications(path.clone()).unwrap();
        assert!(source.notice.is_none());
        assert!(!source.job_pending());
        source.request().unwrap();
        ready(&mut source);
        assert!(source.take().unwrap().is_err());
        assert!(matches!(
            source.notification_error(),
            Some(SourceError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            })
        ));
        assert!(source.notice_attempt);
        fs::create_dir(&parent).unwrap();
        fs::write(path, b"usable via periodic read").unwrap();
        source.request().unwrap();
        ready(&mut source);
        assert_eq!(
            source.take().unwrap().unwrap().bytes(),
            b"usable via periodic read"
        );
        assert!(source.notice.is_none());
        assert!(!source.poll_notice());
        source.close();
        assert!(source.reap_closed().unwrap());
    }
}
