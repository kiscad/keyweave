//! Owned temporary config image. No parser, input APIs, raw handle exports or Clone.
use std::{
    cell::Cell,
    ffi::c_void,
    io::{self, Read},
    marker::PhantomData,
    path::{Path, PathBuf},
    ptr::null_mut,
    rc::Rc,
};
type Handle = *mut c_void;
pub const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
const PATH_CAP: usize = 32768;
const CHUNK: usize = 65536;
const WRITE_BUDGET: usize = 256;
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}
#[repr(C)]
#[derive(Clone, Copy)]
union LargeInteger {
    quad: i64,
}
#[cfg(not(test))]
mod api;
#[cfg(test)]
mod api_mock;
use api::*;
#[cfg(test)]
use api_mock as api;
thread_local! {static POISONED:Cell<bool>=const {Cell::new(false)};}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArtifactError {
    #[error("config artifact forbidden in keyboard callback")]
    InCallback,
    #[error("config artifact cleanup unresolved on this thread")]
    Poisoned,
    #[error("config artifact exceeds 8MiB")]
    Size,
    #[error("config artifact directory/path is invalid or too long")]
    Path,
    #[error("config artifact native metadata is invalid")]
    Metadata,
    #[error("config artifact requires local fixed volume with persistent ACLs")]
    Volume,
    #[error("config artifact write made invalid progress")]
    WriteProgress,
    #[error("config artifact write operation budget exhausted")]
    WriteBudget,
    #[error("config artifact is not sealed or has closed")]
    NotSealed,
    #[error("config artifact reader was already attempted")]
    ReaderAttempted,
    #[error("config artifact RNG status {0}")]
    Random(i32),
    #[error("{call} failed: {code}")]
    Api { call: &'static str, code: u32 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtifactCleanup {
    pub file_closed: bool,
    pub security_released: bool,
    pub file_error: Option<ArtifactError>,
    pub security_error: Option<ArtifactError>,
}
impl ArtifactCleanup {
    pub fn complete(self) -> bool {
        self.file_closed
            && self.security_released
            && self.file_error.is_none()
            && self.security_error.is_none()
    }
}
#[derive(Debug, thiserror::Error)]
#[error("config artifact creation failed: {cause}; rollback: {rollback:?}")]
pub struct ArtifactCreateError {
    pub cause: ArtifactError,
    pub rollback: Option<ArtifactCleanup>,
    pub owner: Option<Box<ConfigArtifact>>,
}
#[derive(Debug, thiserror::Error)]
#[error("config reader preparation failed: {cause}; rollback: {rollback:?}")]
pub struct ArtifactReaderError {
    pub cause: ArtifactError,
    pub rollback: Option<ArtifactError>,
    pub owner: Option<Box<ConfigReader>>,
}
fn reader_early(cause: ArtifactError) -> ArtifactReaderError {
    ArtifactReaderError {
        cause,
        rollback: None,
        owner: None,
    }
}
fn reader_reject(mut reader: ConfigReader, cause: ArtifactError) -> ArtifactReaderError {
    reader.failed = true;
    let rollback = reader.close().err();
    ArtifactReaderError {
        cause,
        rollback,
        owner: Some(Box::new(reader)),
    }
}
fn context() -> Result<(), ArtifactError> {
    if super::outside_callback() {
        Ok(())
    } else {
        Err(ArtifactError::InCallback)
    }
}
fn error(call: &'static str) -> ArtifactError {
    // SAFETY: immediate same-thread last-error scalar.
    ArtifactError::Api {
        call,
        code: unsafe { GetLastError() },
    }
}
fn early(cause: ArtifactError) -> ArtifactCreateError {
    ArtifactCreateError {
        cause,
        rollback: None,
        owner: None,
    }
}
#[cfg(windows)]
fn units(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().take(PATH_CAP).collect()
}
#[cfg(not(windows))]
fn units(path: &Path) -> Vec<u16> {
    path.to_string_lossy()
        .encode_utf16()
        .take(PATH_CAP)
        .collect()
} // mock only
#[cfg(windows)]
fn from_units(units: &[u16]) -> Result<PathBuf, ArtifactError> {
    use std::os::windows::ffi::OsStringExt;
    Ok(std::ffi::OsString::from_wide(units).into())
}
#[cfg(not(windows))]
fn from_units(units: &[u16]) -> Result<PathBuf, ArtifactError> {
    String::from_utf16(units)
        .map(PathBuf::from)
        .map_err(|_| ArtifactError::Metadata)
}
fn local_path(units: &[u16]) -> bool {
    units.len() > 7
        && units[..4] == [92, 92, 63, 92]
        && units[4] <= 127
        && (units[4] as u8).is_ascii_alphabetic()
        && units[5..7] == [58, 92]
        && !units.contains(&0)
}
fn directory(path: &Path) -> Result<PathBuf, ArtifactError> {
    if !path.is_absolute() || units(path).len() > PATH_CAP - 128 {
        return Err(ArtifactError::Path);
    }
    #[cfg(test)]
    {
        Ok(path.to_path_buf())
    }
    #[cfg(not(test))]
    {
        let resolved = std::fs::canonicalize(path).map_err(|e| ArtifactError::Api {
            call: "canonicalize config directory",
            code: e.raw_os_error().unwrap_or(0) as u32,
        })?;
        if !resolved.is_dir() || !local_path(&units(&resolved)) {
            return Err(ArtifactError::Path);
        }
        Ok(resolved)
    }
}
/// Keep this original owner until the child has actually exited. Path alone is
/// not an immutable handoff; future host must transfer its unique reader object.
pub struct ConfigArtifact {
    handle: Handle,
    descriptor: *mut c_void,
    path: PathBuf,
    sealed: bool,
    reader_attempted: bool,
    _thread: PhantomData<Rc<()>>,
}
impl std::fmt::Debug for ConfigArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigArtifact")
            .field("sealed", &self.sealed)
            .field("closed", &self.handle.is_null())
            .finish_non_exhaustive()
    }
}
impl ConfigArtifact {
    pub fn create(directory_path: &Path, bytes: &[u8]) -> Result<Self, ArtifactCreateError> {
        context().map_err(early)?;
        if POISONED.get() {
            return Err(early(ArtifactError::Poisoned));
        }
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(early(ArtifactError::Size));
        }
        let directory = directory(directory_path).map_err(early)?;
        let mut random = [0u8; 16];
        // SAFETY: system preferred RNG, fixed live initialized output, no handle.
        let status = unsafe { BCryptGenRandom(null_mut(), random.as_mut_ptr(), 16, 2) };
        if status != 0 {
            return Err(early(ArtifactError::Random(status)));
        }
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let path = directory.join(format!("keyweave-config-{suffix}.jsonc"));
        let mut owner = Self {
            handle: null_mut(),
            descriptor: null_mut(),
            path,
            sealed: false,
            reader_attempted: false,
            _thread: PhantomData,
        };
        if let Err(cause) = owner.prepare(bytes) {
            let rollback = owner.close().ok();
            return Err(ArtifactCreateError {
                cause,
                rollback,
                owner: Some(Box::new(owner)),
            });
        }
        owner.sealed = true;
        Ok(owner)
    }
    fn prepare(&mut self, bytes: &[u8]) -> Result<(), ArtifactError> {
        let sddl: Vec<u16> = "D:P(A;;GA;;;OW)(A;;GA;;;SY)\0".encode_utf16().collect();
        // SAFETY: terminated bounded SDDL, unique LocalAlloc output, revision1.
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(error("config security"));
        }
        if descriptor.is_null() {
            return Err(ArtifactError::Metadata);
        }
        self.descriptor = descriptor;
        let attributes = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor: self.descriptor,
            inherit: 0,
        };
        let mut path = units(&self.path);
        if path.len() >= PATH_CAP || path.contains(&0) {
            return Err(ArtifactError::Path);
        }
        path.push(0);
        // SAFETY: stable terminated name/descriptor, CREATE_NEW only; owned sync
        // disk file, read/write/DELETE, share READ only, delete-on-last-close.
        let file = unsafe {
            CreateFileW(
                path.as_ptr(),
                0xc0010000,
                1,
                &attributes,
                1,
                0x04000100,
                null_mut(),
            )
        };
        if file.is_null() || file as usize == usize::MAX {
            return Err(error("create config file"));
        }
        self.handle = file;
        self.release_security()?;
        self.validate()?;
        let mut written = 0;
        for _ in 0..WRITE_BUDGET {
            if written == bytes.len() {
                return Ok(());
            }
            let part = &bytes[written..bytes.len().min(written + CHUNK)];
            let mut count = 0;
            // SAFETY: synchronous handle from our factory, live immutable slice
            // and count, no OVERLAPPED; no buffer is retained after return.
            if unsafe {
                WriteFile(
                    self.handle,
                    part.as_ptr().cast(),
                    part.len() as u32,
                    &mut count,
                    null_mut(),
                )
            } == 0
            {
                return Err(error("write config file"));
            }
            if count == 0 || count as usize > part.len() {
                return Err(ArtifactError::WriteProgress);
            }
            written += count as usize;
        }
        if written == bytes.len() {
            Ok(())
        } else {
            Err(ArtifactError::WriteBudget)
        }
    }
    fn validate(&mut self) -> Result<(), ArtifactError> {
        // SAFETY: this factory's owned file handle, immediate scalar query.
        if unsafe { GetFileType(self.handle) } != 1 {
            return Err(ArtifactError::Metadata);
        }
        let mut flags = 0;
        // SAFETY: unique live out DWORD; no handle inheritance permitted.
        if unsafe { GetHandleInformation(self.handle, &mut flags) } == 0 {
            return Err(error("config handle flags"));
        }
        if flags != 0 {
            return Err(ArtifactError::Metadata);
        }
        let mut path = vec![0u16; PATH_CAP];
        // SAFETY: fixed bounded writable UTF16 buffer, normalized DOS path0.
        let n = unsafe {
            GetFinalPathNameByHandleW(self.handle, path.as_mut_ptr(), PATH_CAP as u32, 0)
        } as usize;
        if n == 0 {
            return Err(error("config final path"));
        }
        if n >= PATH_CAP || path[n] != 0 || !local_path(&path[..n]) {
            return Err(ArtifactError::Volume);
        }
        let mut volume = vec![0u16; PATH_CAP];
        // SAFETY: live terminated normalized input and bounded output buffer.
        if unsafe { GetVolumePathNameW(path.as_ptr(), volume.as_mut_ptr(), PATH_CAP as u32) } == 0 {
            return Err(error("config volume path"));
        }
        if !volume.contains(&0) || volume[0] == 0 {
            return Err(ArtifactError::Metadata);
        }
        // SAFETY: terminated volume root obtained from actual file path.
        if unsafe { GetDriveTypeW(volume.as_ptr()) } != 3 {
            return Err(ArtifactError::Volume);
        }
        let mut fs_flags = 0;
        // SAFETY: optional outputs NULL, live flags; actual nearest volume root.
        if unsafe {
            GetVolumeInformationW(
                volume.as_ptr(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                &mut fs_flags,
                null_mut(),
                0,
            )
        } == 0
        {
            return Err(error("config volume information"));
        }
        if fs_flags & 8 == 0 {
            return Err(ArtifactError::Volume);
        }
        self.path = from_units(&path[..n])?;
        Ok(())
    }
    fn release_security(&mut self) -> Result<(), ArtifactError> {
        if self.descriptor.is_null() {
            return Ok(());
        }
        // SAFETY: unique LocalAlloc conversion result; synchronous create has
        // returned and no API keeps this descriptor input.
        if !unsafe { LocalFree(self.descriptor) }.is_null() {
            return Err(error("free config security"));
        }
        self.descriptor = null_mut();
        Ok(())
    }
    pub fn path(&self) -> Result<&Path, ArtifactError> {
        if self.sealed && !self.handle.is_null() {
            Ok(&self.path)
        } else {
            Err(ArtifactError::NotSealed)
        }
    }
    pub fn prepare_reader(&mut self) -> Result<ConfigReader, ArtifactReaderError> {
        context().map_err(reader_early)?;
        if !self.sealed || self.handle.is_null() {
            return Err(reader_early(ArtifactError::NotSealed));
        }
        if self.reader_attempted {
            return Err(reader_early(ArtifactError::ReaderAttempted));
        }
        self.reader_attempted = true;
        let mut handle = null_mut();
        // SAFETY: unique sync source, own current process pseudo handles; request
        // READ explicitly (not SAME_ACCESS), no inheritance and unique output.
        if unsafe {
            DuplicateHandle(
                (-1isize) as Handle,
                self.handle,
                (-1isize) as Handle,
                &mut handle,
                0x80000000,
                0,
                0,
            )
        } == 0
        {
            return Err(reader_early(error("duplicate config reader")));
        }
        if handle.is_null() || handle as usize == usize::MAX {
            return Err(reader_early(ArtifactError::Metadata));
        }
        let reader = ConfigReader {
            handle,
            failed: false,
            _thread: PhantomData,
        };
        let mut flags = 0;
        // SAFETY: live unique duplicated handle/out DWORD, no inheritance.
        if unsafe { GetHandleInformation(handle, &mut flags) } == 0 {
            return Err(reader_reject(reader, error("reader handle flags")));
        }
        if flags != 0 {
            return Err(reader_reject(reader, ArtifactError::Metadata));
        }
        // SAFETY: sync file position shared with now-sealed owner (no further
        // writes/reads), no second reader allowed, FILE_BEGIN and distance0.
        if unsafe { SetFilePointerEx(handle, LargeInteger { quad: 0 }, null_mut(), 0) } == 0 {
            return Err(reader_reject(reader, error("config reader rewind")));
        }
        Ok(reader)
    }
    pub fn close(&mut self) -> Result<ArtifactCleanup, ArtifactError> {
        context()?;
        let mut file_error = None;
        if !self.handle.is_null() {
            // SAFETY: unique owned handle, no file operation outstanding.
            if unsafe { CloseHandle(self.handle) } != 0 {
                self.handle = null_mut();
                self.sealed = false;
            } else {
                file_error = Some(error("close config file"));
            }
        }
        let security_error = self.release_security().err();
        Ok(ArtifactCleanup {
            file_closed: self.handle.is_null(),
            security_released: self.descriptor.is_null(),
            file_error,
            security_error,
        })
    }
}
impl Drop for ConfigArtifact {
    fn drop(&mut self) {
        if self.handle.is_null() && self.descriptor.is_null() {
            return;
        }
        if !self.close().is_ok_and(ArtifactCleanup::complete) {
            POISONED.set(true);
        }
        // Failure retains OS resources until process exit; not deletion proof.
    }
}
/// Unique synchronous read-only file object. Future Job transfer must consume
/// this owner; sharing the original sealed writer cursor is otherwise forbidden.
pub struct ConfigReader {
    handle: Handle,
    failed: bool,
    _thread: PhantomData<Rc<()>>,
}
impl std::fmt::Debug for ConfigReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigReader")
            .field("closed", &self.handle.is_null())
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}
impl ConfigReader {
    pub fn close(&mut self) -> Result<(), ArtifactError> {
        context()?;
        if self.handle.is_null() {
            return Ok(());
        }
        // SAFETY: unique own duplicate, synchronous IO already returned.
        if unsafe { CloseHandle(self.handle) } == 0 {
            return Err(error("close config reader"));
        }
        self.handle = null_mut();
        Ok(())
    }
}
impl Read for ConfigReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        context().map_err(io::Error::other)?;
        if self.failed || self.handle.is_null() {
            return Err(io::Error::other(ArtifactError::NotSealed));
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        let n = bytes.len().min(CHUNK);
        let mut count = 0;
        // SAFETY: factory's unique sync read-only duplicate, live bounded output
        // and count, NULL OVERLAPPED; no buffer survives a synchronous return.
        if unsafe {
            ReadFile(
                self.handle,
                bytes.as_mut_ptr().cast(),
                n as u32,
                &mut count,
                null_mut(),
            )
        } == 0
        {
            self.failed = true;
            return Err(io::Error::other(error("read config file")));
        }
        if count as usize > n {
            self.failed = true;
            return Err(io::Error::other(ArtifactError::Metadata));
        }
        Ok(count as usize)
    }
}
impl Drop for ConfigReader {
    fn drop(&mut self) {
        if !self.handle.is_null() && self.close().is_err() {
            POISONED.set(true);
        }
    }
}
#[cfg(test)]
mod tests;
