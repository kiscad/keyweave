//! User host preparation and owned worker lifecycle. No automatic capture.
use std::path::{Path, PathBuf};
/// Resolve a packaged sibling or an explicitly chosen absolute developer image.
/// No PATH/shell search. Filesystem permissions/trust remain deployment premises.
pub fn locate_worker(host: &Path, explicit: Option<&Path>) -> std::io::Result<PathBuf> {
    let host = host.canonicalize()?;
    let parent = host.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let candidate = if let Some(path) = explicit {
        if !path.is_absolute() {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        path.to_owned()
    } else {
        parent.join("keyweave-worker.exe")
    };
    let resolved = candidate.canonicalize()?;
    if !resolved.is_file() || (explicit.is_none() && resolved.parent() != Some(parent)) {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(resolved)
}
#[cfg(windows)]
pub mod windows;
