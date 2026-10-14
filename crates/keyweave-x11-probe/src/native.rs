//! Disposable process only. SDK/XIO failures must never execute inside a capturer.
#![allow(unsafe_code)]
use super::*;
use std::{
    ffi::{CString, c_char, c_int, c_long, c_ulong, c_void},
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    ptr, slice,
};
#[repr(C)]
struct DeviceInfo {
    id: c_int,
    name: *mut c_char,
    use_kind: c_int,
    attachment: c_int,
    enabled: c_int,
    num_classes: c_int,
    classes: *mut *mut AnyClass,
}
#[repr(C)]
struct AnyClass {
    kind: c_int,
    source: c_int,
}
#[repr(C)]
struct KeyClass {
    kind: c_int,
    source: c_int,
    count: c_int,
    codes: *mut c_int,
}
#[repr(C)]
struct ButtonState {
    length: c_int,
    mask: *mut u8,
}
#[repr(C)]
struct ButtonClass {
    kind: c_int,
    source: c_int,
    count: c_int,
    labels: *mut c_ulong,
    state: ButtonState,
}
type Open = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
type Free = unsafe extern "C" fn(*mut c_void) -> c_int;
type Atom = unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong;
type Version = unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_int;
type Devices = unsafe extern "C" fn(*mut c_void, c_int, *mut c_int) -> *mut DeviceInfo;
type FreeDevices = unsafe extern "C" fn(*mut DeviceInfo);
type Property = unsafe extern "C" fn(
    *mut c_void,
    c_int,
    c_ulong,
    c_long,
    c_long,
    c_int,
    c_ulong,
    *mut c_ulong,
    *mut c_int,
    *mut c_ulong,
    *mut c_ulong,
    *mut *mut u8,
) -> c_int;
struct Library(*mut c_void);
impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: unique loader owner, all native handles already closed by inner RAII.
        unsafe {
            libc::dlclose(self.0);
        }
    }
}
fn unavailable() -> io::Error {
    io::ErrorKind::Unsupported.into()
}
fn library(name: &str) -> io::Result<Library> {
    if !cfg!(all(
        target_pointer_width = "64",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    )) {
        return Err(unavailable());
    }
    #[cfg(target_arch = "x86_64")]
    let directory = "/usr/lib/x86_64-linux-gnu";
    #[cfg(target_arch = "aarch64")]
    let directory = "/usr/lib/aarch64-linux-gnu";
    #[cfg(target_arch = "riscv64")]
    let directory = "/usr/lib/riscv64-linux-gnu";
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )))]
    let directory = "/unsupported";
    let path = fs::canonicalize(std::path::Path::new(directory).join(name))?;
    for ancestor in path.ancestors() {
        let metadata = fs::metadata(ancestor)?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    let path = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| unavailable())?;
    // SAFETY: audited canonical system library path, owned loader result.
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return Err(unavailable());
    }
    Ok(Library(handle))
}
struct Display {
    value: *mut c_void,
    close: Close,
}
impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: unique successful XOpenDisplay result; process may fail on XIO error.
        unsafe {
            (self.close)(self.value);
        }
    }
}
struct Info {
    value: *mut DeviceInfo,
    free: FreeDevices,
}
impl Drop for Info {
    fn drop(&mut self) {
        // SAFETY: allocation returned by XIQueryDevice, matching SDK free exactly once.
        unsafe {
            (self.free)(self.value);
        }
    }
}
struct Data {
    value: *mut u8,
    free: Free,
}
impl Drop for Data {
    fn drop(&mut self) {
        if !self.value.is_null() {
            // SAFETY: XIGetProperty allocation, matching XFree exactly once.
            unsafe {
                (self.free)(self.value.cast());
            }
        }
    }
}
pub(super) fn query(args: &Arguments) -> io::Result<Vec<Device>> {
    let x11 = library("libX11.so.6")?;
    let xi = library("libXi.so.6")?;
    macro_rules! symbol {
        ($library:expr,$name:literal,$ty:ty) => {{
            // SAFETY: literal public SDK function symbol in an owned audited library.
            let value = unsafe { libc::dlsym($library.0, concat!($name, "\0").as_ptr().cast()) };
            if value.is_null() {
                return Err(unavailable());
            }
            // SAFETY: exact Xlib/XI2 public signature; libraries outlive every call/owner.
            unsafe { std::mem::transmute::<*mut c_void, $ty>(value) }
        }};
    }
    let open = symbol!(x11, "XOpenDisplay", Open);
    let close = symbol!(x11, "XCloseDisplay", Close);
    let free = symbol!(x11, "XFree", Free);
    let atom = symbol!(x11, "XInternAtom", Atom);
    let version = symbol!(xi, "XIQueryVersion", Version);
    let devices = symbol!(xi, "XIQueryDevice", Devices);
    let free_devices = symbol!(xi, "XIFreeDeviceInfo", FreeDevices);
    let property = symbol!(xi, "XIGetProperty", Property);
    let metadata = fs::symlink_metadata(&args.node)?;
    if !metadata.file_type().is_char_device()
        || libc::major(metadata.rdev()) != args.major
        || libc::minor(metadata.rdev()) != args.minor
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let name = CString::new(args.display.as_str()).map_err(|_| unavailable())?;
    // SAFETY: bounded local display text, SDK owns the returned connection.
    let pointer = unsafe { open(name.as_ptr()) };
    if pointer.is_null() {
        return Err(unavailable());
    }
    let display = Display {
        value: pointer,
        close,
    };
    let mut major = 2;
    let mut minor = 0;
    // SAFETY: live connection, exclusive aligned integer outputs; read-only XI negotiation.
    if unsafe { version(display.value, &mut major, &mut minor) } != 0 || major < 2 {
        return Err(unavailable());
    }
    // SAFETY: literal names, read-only only-if-exists atom lookup on live display.
    let node_atom = unsafe { atom(display.value, c"Device Node".as_ptr(), 1) };
    if node_atom == 0 {
        return Err(unavailable());
    }
    let mut count = 0;
    // SAFETY: live display and aligned count output; XIAllDevices=0, returned owner managed below.
    let pointer = unsafe { devices(display.value, 0, &mut count) };
    if pointer.is_null() {
        return Err(unavailable());
    }
    let info = Info {
        value: pointer,
        free: free_devices,
    };
    if !(0..=256).contains(&count) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    // SAFETY: XIQueryDevice allocation contains exactly the reported bounded count.
    let infos = unsafe { slice::from_raw_parts(info.value, count as usize) };
    let mut result = vec![];
    for row in infos {
        if row.id <= 0 || row.enabled == 0 || row.attachment <= 0 || !matches!(row.use_kind, 3 | 4)
        {
            continue;
        }
        let master_kind = if row.use_kind == 3 { 1 } else { 2 };
        if !infos.iter().any(|master| {
            master.id == row.attachment && master.enabled != 0 && master.use_kind == master_kind
        }) {
            continue;
        }
        let mut actual_type = 0;
        let mut format = 0;
        let mut items = 0;
        let mut after = 0;
        let mut data = Data {
            value: ptr::null_mut(),
            free,
        };
        // SAFETY: live device ID/display, bounded read-only property request, initialized
        // scalar/pointer outputs; owned data is XFree'd even on a rejected reply.
        let status = unsafe {
            property(
                display.value,
                row.id,
                node_atom,
                0,
                64,
                0,
                0,
                &mut actual_type,
                &mut format,
                &mut items,
                &mut after,
                &mut data.value,
            )
        };
        if status != 0
            || actual_type != 31
            || format != 8
            || after != 0
            || items > 256
            || data.value.is_null()
        {
            continue;
        }
        // SAFETY: property returned exactly items format-8 bytes; no nul scan needed.
        let bytes = unsafe { slice::from_raw_parts(data.value, items as usize) };
        let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        if bytes != args.node.as_os_str().as_encoded_bytes() {
            continue;
        }
        if !(0..=64).contains(&row.num_classes) || (row.num_classes > 0 && row.classes.is_null()) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let classes = if row.num_classes == 0 {
            &[][..]
        } else {
            // SAFETY: SDK allocation contains its bounded reported class pointers.
            unsafe { slice::from_raw_parts(row.classes, row.num_classes as usize) }
        };
        let mut keys = vec![];
        let mut buttons = 0;
        for pointer in classes {
            if pointer.is_null() {
                return Err(io::ErrorKind::InvalidData.into());
            }
            // SAFETY: every SDK class begins with the common public AnyClass header.
            let header = unsafe { &**pointer };
            if header.kind == 0 {
                // SAFETY: XIKeyClass tag chooses this public class layout.
                let class = unsafe { &*pointer.cast::<KeyClass>() };
                if !(0..=768).contains(&class.count) || (class.count > 0 && class.codes.is_null()) {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                if class.count > 0 {
                    if class.count as usize > 768usize.saturating_sub(keys.len()) {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    // SAFETY: SDK class owns exactly count int keycodes.
                    keys.extend_from_slice(unsafe {
                        slice::from_raw_parts(class.codes, class.count as usize)
                    });
                }
            } else if header.kind == 1 {
                // SAFETY: XIButtonClass tag chooses this public class layout; pressed mask unused.
                let class = unsafe { &*pointer.cast::<ButtonClass>() };
                if !(0..=256).contains(&class.count) {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                buttons = buttons.max(class.count as u32);
            }
        }
        keys.sort_unstable();
        keys.dedup();
        if keys.len() > 768 || keys.iter().any(|code| *code < 0) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if result.len() >= 16 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        result.push(Device {
            id: row.id,
            attachment: row.attachment,
            kind: if row.use_kind == 3 {
                "pointer"
            } else {
                "keyboard"
            },
            keys,
            buttons,
        });
    }
    Ok(result)
}
