//! Ordinary-user documents only. No overwrite, execution or runtime activation.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
pub fn read_document(path: &Path) -> Result<String, String> {
    if !path.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Choose a regular configuration file".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(keyweave_config::MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > keyweave_config::MAX_CONFIG_BYTES {
        return Err("Configuration exceeds 8 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "Configuration must be UTF-8".into())
}
pub struct CopyReport {
    pub path: PathBuf,
    pub temporary_cleanup: Option<String>,
}
pub fn save_copy(path: &Path, text: &str) -> Result<CopyReport, String> {
    if text.len() > keyweave_config::MAX_CONFIG_BYTES {
        return Err("Configuration exceeds 8 MiB".into());
    }
    let name = path.file_name().ok_or("Choose a file name")?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize().map_err(|e| e.to_string())?;
    let destination = parent.join(name);
    let mut temporary = None;
    for _ in 0..64 {
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "Temporary name space exhausted")?;
        let path = parent.join(format!(".keyweave-ui-{}-{id}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => {
                temporary = Some((path, file));
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    let (temporary, mut file) = temporary.ok_or("Cannot create a unique temporary file")?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    // Same-directory hard link publishes complete bytes only if name is absent.
    // Existing files/symlinks are never replaced. Trusted user directory premise;
    // this is not a privileged artifact or a full ancestor/race-proof file API.
    let published = written.and_then(|()| fs::hard_link(&temporary, &destination));
    let cleanup = fs::remove_file(&temporary)
        .err()
        .map(|e| format!("{}: {e}", temporary.display()));
    match published {
        Ok(()) => Ok(CopyReport {
            path: destination,
            temporary_cleanup: cleanup,
        }),
        Err(e) => Err(match cleanup {
            Some(cleanup) => format!("{e}; temporary cleanup failed: {cleanup}"),
            None => e.to_string(),
        }),
    }
}
