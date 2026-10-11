//! Cold configuration source opening. No input discovery or protocol IO.
use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};
/// NONBLOCK prevents a FIFO-open wait; regular filesystem IO can still stall.
/// Follow ordinary file symlinks, then validate the actually owned object.
/// std opens CLOEXEC. No descriptor is inherited or mapped as a device here.
pub fn open(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration source must be regular",
        ));
    }
    Ok(file)
}
