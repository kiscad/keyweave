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
#[repr(C)]
struct ValuatorClass {
    kind: c_int,
    source: c_int,
    number: c_int,
    label: c_ulong,
    min: f64,
    max: f64,
    value: f64,
    resolution: c_int,
    mode: c_int,
}
fn motion_axis(class: &ValuatorClass, labels: [c_ulong; 2]) -> io::Result<Option<u8>> {
    if !(0..=255).contains(&class.number) || !(0..=1).contains(&class.mode) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    if class.mode != 0 || class.label == 0 {
        return Ok(None);
    }
    Ok(labels
        .iter()
        .position(|label| *label != 0 && *label == class.label)
        .map(|axis| axis as u8))
}
#[repr(C)]
struct ScrollClass {
    kind: c_int,
    source: c_int,
    number: c_int,
    scroll_type: c_int,
    increment: f64,
    flags: c_int,
}
fn scroll_axis(class: &ScrollClass) -> io::Result<u8> {
    if !(0..=255).contains(&class.number)
        || !class.increment.is_finite()
        || class.increment == 0.0
        || class.flags & !3 != 0
    {
        return Err(io::ErrorKind::InvalidData.into());
    }
    match class.scroll_type {
        1 => Ok(1), // XIScrollTypeVertical
        2 => Ok(0), // XIScrollTypeHorizontal
        _ => Err(io::ErrorKind::InvalidData.into()),
    }
}
fn joined_scroll_axes(relative: &[i32], scroll: &[(i32, u8)]) -> io::Result<Vec<u8>> {
    let mut axes = Vec::new();
    let mut numbers = Vec::new();
    for &(number, axis) in scroll {
        if !relative.contains(&number) || numbers.contains(&number) || axes.contains(&axis) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        numbers.push(number);
        axes.push(axis);
    }
    axes.sort_unstable();
    Ok(axes)
}
fn semantic_button_labels(labels: &[c_ulong], atoms: [c_ulong; 8]) -> io::Result<Vec<u16>> {
    let mut result = Vec::new();
    for label in labels {
        let code = atoms
            .iter()
            .position(|atom| *atom != 0 && atom == label)
            .map_or(0, |index| 0x110 + index as u16);
        if code != 0 && result.contains(&code) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        result.push(code);
    }
    Ok(result)
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
    let mut minor = 1;
    // SAFETY: live connection, exclusive aligned integer outputs; read-only XI negotiation.
    if unsafe { version(display.value, &mut major, &mut minor) } != 0 || major < 2 {
        return Err(unavailable());
    }
    // SAFETY: literal names, read-only only-if-exists atom lookup on live display.
    let node_atom = unsafe { atom(display.value, c"Device Node".as_ptr(), 1) };
    if node_atom == 0 {
        return Err(unavailable());
    }
    // SAFETY: fixed label names, only-if-exists; never creates an atom.
    let motion_labels = unsafe {
        [
            atom(display.value, c"Rel X".as_ptr(), 1),
            atom(display.value, c"Rel Y".as_ptr(), 1),
        ]
    };
    // SAFETY: fixed literals, only-if-exists lookup on the live connection.
    let button_atoms = unsafe {
        [
            atom(display.value, c"Button Left".as_ptr(), 1),
            atom(display.value, c"Button Right".as_ptr(), 1),
            atom(display.value, c"Button Middle".as_ptr(), 1),
            atom(display.value, c"Button Side".as_ptr(), 1),
            atom(display.value, c"Button Extra".as_ptr(), 1),
            atom(display.value, c"Button Forward".as_ptr(), 1),
            atom(display.value, c"Button Back".as_ptr(), 1),
            atom(display.value, c"Button Task".as_ptr(), 1),
        ]
    };
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
        let mut button_labels = None;
        let mut relative_motion_axes = vec![];
        let mut relative_numbers = vec![];
        let mut scroll = vec![];
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
                if !(0..=256).contains(&class.count)
                    || class.source != row.id
                    || button_labels.is_some()
                    || (class.count > 0 && class.labels.is_null())
                {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                let labels = if class.count == 0 {
                    &[][..]
                } else {
                    // SAFETY: XIButtonClass owns exactly count Atom labels; pressed state unused.
                    unsafe { slice::from_raw_parts(class.labels, class.count as usize) }
                };
                button_labels = Some(semantic_button_labels(labels, button_atoms)?);
                buttons = class.count as u32;
            } else if header.kind == 2 {
                // SAFETY: XIValuatorClass tag chooses the SDK layout; current value is never read.
                let class = unsafe { &*pointer.cast::<ValuatorClass>() };
                let axis = motion_axis(class, motion_labels)?;
                if class.mode == 0 {
                    if class.source != row.id || relative_numbers.contains(&class.number) {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    relative_numbers.push(class.number);
                }
                if let Some(axis) = axis {
                    if row.use_kind != 3 || relative_motion_axes.contains(&axis) {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    relative_motion_axes.push(axis);
                }
            } else if header.kind == 3 {
                // SAFETY: XIScrollClass tag chooses the public SDK layout, metadata only.
                let class = unsafe { &*pointer.cast::<ScrollClass>() };
                if row.use_kind != 3 || class.source != row.id || scroll.len() >= 2 {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                scroll.push((class.number, scroll_axis(class)?));
            }
        }
        let relative_scroll_axes = joined_scroll_axes(&relative_numbers, &scroll)?;
        relative_motion_axes.sort_unstable();
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
            relative_motion_axes,
            relative_scroll_axes,
            button_labels: button_labels.unwrap_or_default(),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod motion_tests {
    use super::*;
    fn class(label: c_ulong, mode: i32) -> ValuatorClass {
        ValuatorClass {
            kind: 2,
            source: 8,
            number: 0,
            label,
            min: f64::NAN,
            max: f64::NAN,
            value: f64::NAN,
            resolution: 0,
            mode,
        }
    }
    #[test]
    fn labels_and_relative_mode_required() {
        assert_eq!(motion_axis(&class(10, 0), [10, 11]).ok(), Some(Some(0)));
        assert_eq!(motion_axis(&class(11, 0), [10, 11]).ok(), Some(Some(1)));
        for c in [class(10, 1), class(0, 0), class(12, 0)] {
            assert_eq!(motion_axis(&c, [10, 11]).ok(), Some(None));
        }
    }
    #[test]
    fn invalid_mode_and_axis_number_rejected() {
        assert!(motion_axis(&class(10, 2), [10, 11]).is_err());
        let mut c = class(10, 0);
        c.number = 256;
        assert!(motion_axis(&c, [10, 11]).is_err());
        c.number = -1;
        assert!(motion_axis(&c, [10, 11]).is_err());
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    fn class(direction: i32, increment: f64) -> ScrollClass {
        ScrollClass {
            kind: 3,
            source: 8,
            number: 2,
            scroll_type: direction,
            increment,
            flags: 0,
        }
    }
    #[test]
    fn scroll_directions_accept_signed_finite_nonzero_increments() {
        assert_eq!(scroll_axis(&class(1, 15.0)).ok(), Some(1));
        assert_eq!(scroll_axis(&class(2, -15.0)).ok(), Some(0));
    }
    #[test]
    fn scroll_rejects_invalid_scalar_metadata() {
        for value in [0.0, -0.0, f64::NAN, f64::INFINITY] {
            assert!(scroll_axis(&class(1, value)).is_err());
        }
        assert!(scroll_axis(&class(3, 1.0)).is_err());
        let mut c = class(1, 1.0);
        c.flags = 4;
        assert!(scroll_axis(&c).is_err());
        c.flags = 0;
        c.number = -1;
        assert!(scroll_axis(&c).is_err());
    }
    #[test]
    fn scroll_requires_unique_relative_axis_join() {
        assert_eq!(
            joined_scroll_axes(&[2, 3], &[(2, 1), (3, 0)]).ok(),
            Some(vec![0, 1])
        );
        for scroll in [vec![(4, 1)], vec![(2, 1), (2, 0)], vec![(2, 1), (3, 1)]] {
            assert!(joined_scroll_axes(&[2, 3], &scroll).is_err());
        }
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;
    #[test]
    fn button_semantics_do_not_assume_hid_ordinal() {
        assert_eq!(
            semantic_button_labels(&[10, 12, 11, 0, 13], [10, 11, 12, 13, 14, 15, 16, 17]).ok(),
            Some(vec![0x110, 0x112, 0x111, 0, 0x113])
        );
    }
    #[test]
    fn button_labels_reject_duplicate_known_semantics() {
        assert!(semantic_button_labels(&[10, 10], [10, 11, 12, 13, 14, 15, 16, 17]).is_err());
        assert_eq!(
            semantic_button_labels(&[0, 0, 20], [0; 8]).ok(),
            Some(vec![0, 0, 0])
        );
    }
}

#[cfg(test)]
mod sdk_layout_tests {
    use super::*;
    use std::mem::{offset_of, size_of};
    #[test]
    fn repr_c_layouts_match_linux64_sdk_oracle() {
        assert_eq!(size_of::<DeviceInfo>(), 40);
        assert_eq!(offset_of!(DeviceInfo, id), 0);
        assert_eq!(offset_of!(DeviceInfo, use_kind), 16);
        assert_eq!(offset_of!(DeviceInfo, attachment), 20);
        assert_eq!(offset_of!(DeviceInfo, enabled), 24);
        assert_eq!(offset_of!(DeviceInfo, num_classes), 28);
        assert_eq!(offset_of!(DeviceInfo, classes), 32);
        assert_eq!(size_of::<AnyClass>(), 8);
        assert_eq!(offset_of!(AnyClass, source), 4);
        assert_eq!(size_of::<KeyClass>(), 24);
        assert_eq!(offset_of!(KeyClass, count), 8);
        assert_eq!(offset_of!(KeyClass, codes), 16);
        assert_eq!(size_of::<ButtonClass>(), 40);
        assert_eq!(offset_of!(ButtonClass, count), 8);
        assert_eq!(offset_of!(ButtonClass, labels), 16);
        assert_eq!(offset_of!(ButtonClass, state), 24);
        assert_eq!(size_of::<ValuatorClass>(), 56);
        assert_eq!(offset_of!(ValuatorClass, number), 8);
        assert_eq!(offset_of!(ValuatorClass, label), 16);
        assert_eq!(offset_of!(ValuatorClass, mode), 52);
        assert_eq!(size_of::<ScrollClass>(), 32);
        assert_eq!(offset_of!(ScrollClass, number), 8);
        assert_eq!(offset_of!(ScrollClass, scroll_type), 12);
        assert_eq!(offset_of!(ScrollClass, increment), 16);
        assert_eq!(offset_of!(ScrollClass, flags), 24);
    }
}
