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
/// Linux sibling/explicit locator only, not image authentication or execution.
/// At least one execute bit is required; effective access/ACL/mount policy and
/// check-to-exec races are still checked by the eventual native spawn.
#[cfg(target_os = "linux")]
pub fn locate_linux_worker(host: &Path, explicit: Option<&Path>) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let host = host.canonicalize()?;
    let parent = host.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let candidate = if let Some(path) = explicit {
        if !path.is_absolute() {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        path.to_owned()
    } else {
        parent.join("keyweave-worker")
    };
    let resolved = candidate.canonicalize()?;
    let metadata = resolved.metadata()?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
        || (explicit.is_none() && resolved.parent() != Some(parent))
    {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    Ok(resolved)
}
#[cfg(all(test, target_os = "linux"))]
#[allow(clippy::unwrap_used)]
mod linux_locator_tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt, symlink},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(1);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "keyweave-linux-locator-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn file(&self, name: &str, mode: u32) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, b"owned non-executed fixture").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            path
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn native_sibling_name_and_explicit_absolute_image_are_resolved() {
        let dir = Directory::new();
        let host = dir.file("keyweave-host", 0o700);
        let worker = dir.file("keyweave-worker", 0o700);
        assert_eq!(
            locate_linux_worker(&host, None).unwrap(),
            worker.canonicalize().unwrap()
        );
        let developer = dir.file("developer image", 0o700);
        assert_eq!(
            locate_linux_worker(&host, Some(&developer)).unwrap(),
            developer.canonicalize().unwrap()
        );
    }
    #[test]
    fn relative_non_executable_directory_and_missing_image_are_rejected() {
        let dir = Directory::new();
        let host = dir.file("keyweave-host", 0o700);
        let worker = dir.file("keyweave-worker", 0o600);
        assert!(locate_linux_worker(&host, None).is_err());
        assert!(locate_linux_worker(&host, Some(Path::new("relative"))).is_err());
        assert!(locate_linux_worker(&host, Some(&dir.0)).is_err());
        fs::remove_file(worker).unwrap();
        assert!(locate_linux_worker(&host, None).is_err());
    }
    #[test]
    fn default_symlink_escape_is_rejected_but_explicit_trusted_target_is_allowed() {
        let dir = Directory::new();
        let other = Directory::new();
        let host = dir.file("keyweave-host", 0o700);
        let outside = other.file("worker", 0o700);
        symlink(&outside, dir.0.join("keyweave-worker")).unwrap();
        assert!(locate_linux_worker(&host, None).is_err());
        assert_eq!(
            locate_linux_worker(&host, Some(&outside)).unwrap(),
            outside.canonicalize().unwrap()
        );
    }
}
