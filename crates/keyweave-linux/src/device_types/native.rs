//! libudev v257 SDK scalar/opaque ABI. Cold metadata only; never grabs input.
#![allow(unsafe_code)]
use super::{BOOL_TYPES, DeviceTypes, TYPE_NAMES, classify};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs, io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
    ptr::NonNull,
};
type New = unsafe extern "C" fn() -> *mut c_void;
type Unref = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
type FromDevnum = unsafe extern "C" fn(*mut c_void, c_char, libc::dev_t) -> *mut c_void;
type StringQuery = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type Property = unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char;
type Parent = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> *mut c_void;
type Initialized = unsafe extern "C" fn(*mut c_void) -> c_int;
type Devnum = unsafe extern "C" fn(*mut c_void) -> libc::dev_t;
struct Library(NonNull<c_void>);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: owned dlopen handle, all function calls/opaque owners end before drop.
        unsafe {
            libc::dlclose(self.0.as_ptr());
        }
    }
}
struct Functions {
    new: New,
    unref: Unref,
    device_unref: Unref,
    from_devnum: FromDevnum,
    parent: Parent,
    subsystem: StringQuery,
    syspath: StringQuery,
    sysname: StringQuery,
    initialized: Initialized,
    devnum: Devnum,
    property: Property,
}
struct Owner {
    pointer: NonNull<c_void>,
    unref: Unref,
}
impl Drop for Owner {
    fn drop(&mut self) {
        // SAFETY: one reference returned by udev_new/new_from_devnum; library remains
        // live in caller's outer scope. Borrowed parent devices never become Owners.
        unsafe {
            (self.unref)(self.pointer.as_ptr());
        }
    }
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "udev device metadata mismatch or unavailable",
    )
}
fn library_path() -> io::Result<&'static str> {
    #[cfg(target_arch = "x86_64")]
    const PATH: &str = "/usr/lib/x86_64-linux-gnu/libudev.so.1";
    #[cfg(target_arch = "aarch64")]
    const PATH: &str = "/usr/lib/aarch64-linux-gnu/libudev.so.1";
    #[cfg(target_arch = "riscv64")]
    const PATH: &str = "/usr/lib/riscv64-linux-gnu/libudev.so.1";
    #[cfg(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    ))]
    {
        Ok(PATH)
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    {
        Err(io::ErrorKind::Unsupported.into())
    }
}
fn load() -> io::Result<(Library, Functions)> {
    let path = fs::canonicalize(library_path()?)?;
    for ancestor in path.ancestors() {
        let meta = fs::metadata(ancestor)?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "untrusted libudev system library path",
            ));
        }
    }
    if !fs::metadata(&path)?.is_file() {
        return Err(invalid());
    }
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid())?;
    // SAFETY: canonical fixed root-owned path with trusted ancestors. No symbols
    // or alternative search paths are taken from environment/configuration.
    let library = Library(
        NonNull::new(unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) })
            .ok_or_else(invalid)?,
    );
    macro_rules! symbol {
        ($name:literal,$ty:ty) => {{
            // SAFETY: literal SDK symbol, nul-terminated name, library is owned above.
            let ptr = unsafe {
                libc::dlsym(
                    library.0.as_ptr(),
                    CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes())
                        .map_err(|_| invalid())?
                        .as_ptr(),
                )
            };
            if ptr.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "required libudev symbol unavailable",
                ));
            }
            // SAFETY: exact public libudev.h signature; table never outlives library.
            unsafe { std::mem::transmute::<*mut c_void, $ty>(ptr) }
        }};
    }
    let functions = Functions {
        new: symbol!("udev_new", New),
        unref: symbol!("udev_unref", Unref),
        device_unref: symbol!("udev_device_unref", Unref),
        from_devnum: symbol!("udev_device_new_from_devnum", FromDevnum),
        parent: symbol!("udev_device_get_parent_with_subsystem_devtype", Parent),
        subsystem: symbol!("udev_device_get_subsystem", StringQuery),
        syspath: symbol!("udev_device_get_syspath", StringQuery),
        sysname: symbol!("udev_device_get_sysname", StringQuery),
        initialized: symbol!("udev_device_get_is_initialized", Initialized),
        devnum: symbol!("udev_device_get_devnum", Devnum),
        property: symbol!("udev_device_get_property_value", Property),
    };
    Ok((library, functions))
}
// SAFETY caller: pointer is NULL or a live libudev nul-terminated string. Read
// only through the first nul, with an explicit bound and no unbounded scanning.
unsafe fn bytes(ptr: *const c_char, limit: usize) -> io::Result<Option<Vec<u8>>> {
    if ptr.is_null() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    for i in 0..=limit {
        // SAFETY: SDK guarantees a readable string including its nul; after nul
        // return immediately so no offset past the actual allocation is read.
        let byte = unsafe { *ptr.add(i) } as u8;
        if byte == 0 {
            return Ok(Some(bytes));
        }
        if i == limit {
            return Err(invalid());
        }
        bytes.push(byte);
    }
    Err(invalid())
}
unsafe fn flag(ptr: *const c_char) -> io::Result<Option<bool>> {
    // SAFETY: same live SDK string premise; at most two non-nul bytes examined.
    match unsafe { bytes(ptr, 1) }?.as_deref() {
        None => Ok(None),
        Some(b"0") => Ok(Some(false)),
        Some(b"1") => Ok(Some(true)),
        _ => Err(invalid()),
    }
}
fn query(f: &Functions, number: (u32, u32), input_syspath: &Path) -> io::Result<DeviceTypes> {
    if number.0 != 13 || !input_syspath.is_absolute() {
        return Err(invalid());
    }
    // SAFETY: public allocator, no args, owned single reference on success.
    let context = Owner {
        pointer: NonNull::new(unsafe { (f.new)() }).ok_or_else(invalid)?,
        unref: f.unref,
    };
    let devnum = libc::makedev(number.0, number.1);
    // SAFETY: live context, 'c' denotes char device, native dev_t from actual FD.
    let device = Owner {
        pointer: NonNull::new(unsafe {
            (f.from_devnum)(context.pointer.as_ptr(), b'c' as c_char, devnum)
        })
        .ok_or_else(invalid)?,
        unref: f.device_unref,
    };
    let ptr = device.pointer.as_ptr();
    // SAFETY: all borrowed strings/parent remain valid under owned device/context.
    unsafe {
        if (f.devnum)(ptr) != devnum || bytes((f.subsystem)(ptr), 16)?.as_deref() != Some(b"input")
        {
            return Err(invalid());
        }
        let name = bytes((f.sysname)(ptr), 32)?.ok_or_else(invalid)?;
        if !name
            .strip_prefix(b"event")
            .is_some_and(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit))
        {
            return Err(invalid());
        }
        let parent = (f.parent)(ptr, c"input".as_ptr(), std::ptr::null());
        if parent.is_null() {
            return Err(invalid());
        }
        let path = bytes((f.syspath)(parent), 4096)?.ok_or_else(invalid)?;
        if Path::new(std::ffi::OsStr::from_bytes(&path)) != input_syspath {
            return Err(invalid());
        }
        if (f.initialized)(ptr) != 1 {
            return Err(invalid());
        }
        let input = flag((f.property)(ptr, c"ID_INPUT".as_ptr()))?;
        let mut tags = [None; BOOL_TYPES];
        for (i, name) in TYPE_NAMES.iter().enumerate() {
            let name = CString::new(*name).map_err(|_| invalid())?;
            tags[i] = flag((f.property)(ptr, name.as_ptr()))?;
        }
        // Readback ensures we never accept an object whose own identity changed
        // across property access, in addition to caller's FD/sysfs revalidation.
        if (f.devnum)(ptr) != devnum || (f.initialized)(ptr) != 1 {
            return Err(invalid());
        }
        classify(true, input, &tags).ok_or_else(invalid)
    }
}
/// Cold read only, scoped to the opened FD's device number and input syspath.
/// Any error remains None at the EvdevDevice boundary, never false metadata.
pub(crate) fn observe(number: (u32, u32), input_syspath: &Path) -> io::Result<DeviceTypes> {
    // libudev's persistent classification record must also be administratively
    // trusted. A changed database layout is unsupported rather than interpreted.
    let database = std::path::PathBuf::from(format!("/run/udev/data/c{}:{}", number.0, number.1));
    for ancestor in database.ancestors() {
        let meta = fs::symlink_metadata(ancestor)?;
        if meta.file_type().is_symlink() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "untrusted udev classification database path",
            ));
        }
    }
    let meta = fs::metadata(&database)?;
    if !meta.is_file() || meta.len() > 256 * 1024 {
        return Err(invalid());
    }
    let (_library, functions) = load()?;
    query(&functions, number, input_syspath)
}
/// Read-only loader diagnostic. Does not enumerate/open/query input devices.
pub fn provider_available() -> io::Result<()> {
    load().map(drop)
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct State {
        fail: u8,
        number_calls: usize,
        drops: Vec<&'static str>,
    }
    thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
    fn failed(code: u8) -> bool {
        STATE.with(|s| s.borrow().fail == code)
    }
    unsafe extern "C" fn new() -> *mut c_void {
        if failed(1) {
            std::ptr::null_mut()
        } else {
            Box::into_raw(Box::new(1u8)).cast()
        }
    }
    unsafe extern "C" fn unref(p: *mut c_void) -> *mut c_void {
        // SAFETY: mock new allocated exactly one u8, native query releases once.
        unsafe {
            drop(Box::from_raw(p.cast::<u8>()));
        }
        STATE.with(|s| s.borrow_mut().drops.push("context"));
        std::ptr::null_mut()
    }
    unsafe extern "C" fn device_unref(p: *mut c_void) -> *mut c_void {
        // SAFETY: mock from_devnum allocated exactly one u8, one release.
        unsafe {
            drop(Box::from_raw(p.cast::<u8>()));
        }
        STATE.with(|s| s.borrow_mut().drops.push("device"));
        std::ptr::null_mut()
    }
    unsafe extern "C" fn from(_: *mut c_void, _: c_char, _: libc::dev_t) -> *mut c_void {
        if failed(2) {
            std::ptr::null_mut()
        } else {
            Box::into_raw(Box::new(2u8)).cast()
        }
    }
    unsafe extern "C" fn parent(p: *mut c_void, _: *const c_char, _: *const c_char) -> *mut c_void {
        if failed(3) { std::ptr::null_mut() } else { p }
    }
    unsafe extern "C" fn subsystem(_: *mut c_void) -> *const c_char {
        if failed(4) {
            c"usb".as_ptr()
        } else {
            c"input".as_ptr()
        }
    }
    unsafe extern "C" fn path(_: *mut c_void) -> *const c_char {
        if failed(5) {
            c"/sys/devices/other".as_ptr()
        } else {
            c"/sys/devices/mock/input/input1".as_ptr()
        }
    }
    unsafe extern "C" fn name(_: *mut c_void) -> *const c_char {
        if failed(6) {
            c"mouse0".as_ptr()
        } else {
            c"event64".as_ptr()
        }
    }
    unsafe extern "C" fn initialized(_: *mut c_void) -> c_int {
        if failed(7) { 0 } else { 1 }
    }
    unsafe extern "C" fn number(_: *mut c_void) -> libc::dev_t {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.number_calls += 1;
            if s.fail == 8 || s.fail == 9 && s.number_calls > 1 {
                libc::makedev(13, 65)
            } else {
                libc::makedev(13, 64)
            }
        })
    }
    unsafe extern "C" fn property(_: *mut c_void, key: *const c_char) -> *const c_char {
        // SAFETY: query uses only literal/CString keys valid for this synchronous call.
        let key = unsafe { CStr::from_ptr(key) }.to_bytes();
        if failed(10) {
            return c"invalid".as_ptr();
        }
        if failed(11) {
            return std::ptr::null();
        }
        if key == b"ID_INPUT" || key == b"ID_INPUT_KEYBOARD" {
            c"1".as_ptr()
        } else {
            std::ptr::null()
        }
    }
    fn functions() -> Functions {
        Functions {
            new,
            unref,
            device_unref,
            from_devnum: from,
            parent,
            subsystem,
            syspath: path,
            sysname: name,
            initialized,
            devnum: number,
            property,
        }
    }
    #[test]
    fn actual_query_fences_identity_and_releases_all_owned_opaque_references() {
        for fail in 0..=11 {
            STATE.with(|s| {
                *s.borrow_mut() = State {
                    fail,
                    ..Default::default()
                }
            });
            let result = query(
                &functions(),
                (13, 64),
                Path::new("/sys/devices/mock/input/input1"),
            );
            if fail == 0 {
                assert_eq!(
                    result.unwrap(),
                    DeviceTypes {
                        is_keyboard: true,
                        is_pointing_device: false
                    }
                );
            } else {
                assert!(result.is_err(), "failure {fail}");
            }
            let expected = match fail {
                1 => vec![],
                2 => vec!["context"],
                _ => vec!["device", "context"],
            };
            STATE.with(|s| assert_eq!(s.borrow().drops, expected));
        }
    }
    #[test]
    fn bounded_property_parser_rejects_invalid_values_and_preserves_missing() {
        // SAFETY: all pointers are NULL or statically valid nul-terminated strings.
        unsafe {
            assert_eq!(flag(std::ptr::null()).unwrap(), None);
            assert_eq!(flag(c"0".as_ptr()).unwrap(), Some(false));
            assert_eq!(flag(c"1".as_ptr()).unwrap(), Some(true));
            for p in [
                c"".as_ptr(),
                c"true".as_ptr(),
                c"01".as_ptr(),
                c"2".as_ptr(),
            ] {
                assert!(flag(p).is_err());
            }
        }
    }
}
