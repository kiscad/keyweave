//! Private SDK scalar/opaque calls; no device-control or authority-changing API.
#![allow(unsafe_code)]
use super::*;
use crate::worker::WorkerParentGuard;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs,
    marker::PhantomData,
    os::unix::fs::MetadataExt,
    ptr,
    rc::Rc,
};
type PidString = unsafe extern "C" fn(libc::pid_t, *mut *mut c_char) -> c_int;
type StringQuery = unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> c_int;
type BoolQuery = unsafe extern "C" fn(*const c_char) -> c_int;
type UidQuery = unsafe extern "C" fn(*const c_char, *mut libc::uid_t) -> c_int;
struct Library(*mut c_void);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: one owned dlopen handle; no function call outlives its owner.
        unsafe {
            libc::dlclose(self.0);
        }
    }
}
struct Functions {
    pid: PidString,
    active: BoolQuery,
    remote: BoolQuery,
    uid: UidQuery,
    kind: StringQuery,
    class: StringQuery,
    seat: StringQuery,
    display: StringQuery,
    state: StringQuery,
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid sd-login metadata")
}
fn status(value: c_int) -> io::Result<c_int> {
    if value < 0 {
        Err(io::Error::from_raw_os_error(
            value.checked_neg().unwrap_or(libc::EINVAL),
        ))
    } else {
        Ok(value)
    }
}
fn library_path() -> io::Result<&'static str> {
    #[cfg(target_arch = "x86_64")]
    const PATH: &str = "/usr/lib/x86_64-linux-gnu/libsystemd.so.0";
    #[cfg(target_arch = "aarch64")]
    const PATH: &str = "/usr/lib/aarch64-linux-gnu/libsystemd.so.0";
    #[cfg(target_arch = "riscv64")]
    const PATH: &str = "/usr/lib/riscv64-linux-gnu/libsystemd.so.0";
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    return Err(io::ErrorKind::Unsupported.into());
    #[cfg(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    ))]
    Ok(PATH)
}
fn load() -> io::Result<(Library, Functions)> {
    let path = fs::canonicalize(library_path()?)?;
    for ancestor in path.ancestors() {
        let metadata = fs::metadata(ancestor)?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "untrusted system library path",
            ));
        }
    }
    if !fs::metadata(&path)?.is_file() {
        return Err(invalid());
    }
    let path = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| invalid())?;
    // SAFETY: fixed canonical root-owned library, nul-terminated path, owned handle.
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "sd-login library unavailable",
        ));
    }
    let library = Library(handle);
    macro_rules! symbol {
        ($name:literal,$ty:ty) => {{
            // SAFETY: audited sd-login symbol and exact public SDK signature; library
            // stays owned while this function table is in use. No pointer escapes.
            let pointer = unsafe { libc::dlsym(library.0, cstring!($name).as_ptr()) };
            if pointer.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "required sd-login symbol unavailable",
                ));
            }
            // SAFETY: dlsym yielded this public function symbol with the exact SDK
            // signature; only the owned library's lifetime can use the resulting pointer.
            unsafe { std::mem::transmute::<*mut c_void, $ty>(pointer) }
        }};
    }
    // Literal C strings only, without runtime allocation or user-selected symbol names.
    macro_rules! cstring {
        ($name:literal) => {
            CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()).map_err(|_| invalid())?
        };
    }
    let functions = Functions {
        pid: symbol!("sd_pid_get_session", PidString),
        active: symbol!("sd_session_is_active", BoolQuery),
        remote: symbol!("sd_session_is_remote", BoolQuery),
        uid: symbol!("sd_session_get_uid", UidQuery),
        kind: symbol!("sd_session_get_type", StringQuery),
        class: symbol!("sd_session_get_class", StringQuery),
        seat: symbol!("sd_session_get_seat", StringQuery),
        display: symbol!("sd_session_get_display", StringQuery),
        state: symbol!("sd_session_get_state", StringQuery),
    };
    Ok((library, functions))
}
struct OwnedString(*mut c_char);
impl Drop for OwnedString {
    fn drop(&mut self) {
        // SAFETY: sd-login returned malloc-owned string or NULL; exactly one free.
        unsafe {
            libc::free(self.0.cast());
        }
    }
}
impl OwnedString {
    fn copy(&self) -> io::Result<String> {
        if self.0.is_null() {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        for index in 0..=256 {
            // SAFETY: SDK promises a nul-terminated allocated string. Stop before
            // touching beyond its terminator; read at most 257 bytes.
            let value = unsafe { *self.0.add(index) } as u8;
            if value == 0 {
                return String::from_utf8(bytes).map_err(|_| invalid());
            }
            if index == 256 {
                return Err(invalid());
            }
            bytes.push(value);
        }
        Err(invalid())
    }
}
pub struct NativeSessionProbe {
    library: Library,
    functions: Functions,
    subject: SessionSubject,
    affinity: PhantomData<Rc<()>>,
}
impl NativeSessionProbe {
    pub fn for_parent(guard: &WorkerParentGuard) -> io::Result<Self> {
        WorkerParentGuard::verify(guard.parent_pid())?.verify_isolated_group()?;
        // SAFETY: read-only effective uid query with no pointers.
        let subject = SessionSubject {
            parent: guard.parent_pid(),
            worker: std::process::id(),
            uid: unsafe { libc::geteuid() },
        };
        if !subject.valid() {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let (library, functions) = load()?;
        Ok(Self {
            library,
            functions,
            subject,
            affinity: PhantomData,
        })
    }
    pub fn subject(&self) -> SessionSubject {
        self.subject
    }
    /// Actual library symbol presence only, not logind/session availability proof.
    pub fn check_exports() -> io::Result<()> {
        load().map(drop)
    }
    fn verify(&self) -> io::Result<()> {
        WorkerParentGuard::verify(self.subject.parent)?.verify_isolated_group()?;
        // SAFETY: read-only current uid; reject changed credentials.
        if unsafe { libc::geteuid() } != self.subject.uid {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(())
    }
    fn pid_session(&self, pid: u32) -> io::Result<String> {
        let mut value = OwnedString(ptr::null_mut());
        // SAFETY: validated positive pid, exclusive initialized char* output;
        // returned allocation freed by the owner even if status indicates error.
        status(unsafe { (self.functions.pid)(pid as libc::pid_t, &mut value.0) })?;
        value.copy()
    }
    fn text(&self, query: StringQuery, session: &CStr) -> io::Result<String> {
        let mut value = OwnedString(ptr::null_mut());
        // SAFETY: live nul-terminated session and exclusive initialized output.
        status(unsafe { query(session.as_ptr(), &mut value.0) })?;
        value.copy()
    }
}
impl SessionQuery for NativeSessionProbe {
    fn query(&mut self, subject: SessionSubject) -> io::Result<SessionSnapshot> {
        if subject != self.subject {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        self.verify()?;
        let _keep_library = &self.library;
        let parent_session = self.pid_session(subject.parent)?;
        let worker_session = self.pid_session(subject.worker)?;
        if !identifier(&parent_session) || parent_session != worker_session {
            return Err(invalid());
        }
        let session = CString::new(parent_session.as_str()).map_err(|_| invalid())?;
        let mut uid = 0;
        // SAFETY: stable nul-terminated session, initialized aligned uid_t output.
        status(unsafe { (self.functions.uid)(session.as_ptr(), &mut uid) })?;
        // SAFETY: read-only SDK predicate of live session string; no retained pointer.
        let active = status(unsafe { (self.functions.active)(session.as_ptr()) })? > 0;
        // SAFETY: same SDK predicate contract.
        let remote = status(unsafe { (self.functions.remote)(session.as_ptr()) })? > 0;
        let kind = self.text(self.functions.kind, &session)?;
        let class = self.text(self.functions.class, &session)?;
        let seat = self.text(self.functions.seat, &session)?;
        let state = self.text(self.functions.state, &session)?;
        let display = if kind == "x11" {
            Some(self.text(self.functions.display, &session)?)
        } else {
            None
        };
        self.verify()?;
        Ok(SessionSnapshot {
            parent_session,
            worker_session,
            uid,
            active,
            remote,
            class,
            kind,
            seat,
            display,
            state,
        })
    }
}
