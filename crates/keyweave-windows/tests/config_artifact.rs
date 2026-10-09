//! Real owned temporary files only. No input APIs, user configuration or services.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]
use keyweave_windows::{ArtifactError, ConfigArtifact, MAX_ARTIFACT_BYTES};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::Path,
    time::{Duration, Instant},
};
fn assert_deleted(path: &Path) {
    let start = Instant::now();
    loop {
        match fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            _ => assert!(
                start.elapsed() < Duration::from_secs(2),
                "owned config file was not observed deleted"
            ),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn production_file_allows_std_read_but_denies_write_delete_and_rename_until_closed() {
    let data = b"synthetic config image, no real keyboard input";
    let mut artifact = ConfigArtifact::create(&std::env::temp_dir(), data).unwrap();
    let path = artifact.path().unwrap().to_path_buf();
    let mut peek = File::open(&path).unwrap();
    let mut bytes = vec![];
    peek.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, data);
    drop(peek);
    assert_eq!(
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap_err()
            .raw_os_error(),
        Some(32)
    );
    assert_eq!(fs::remove_file(&path).unwrap_err().raw_os_error(), Some(32));
    let destination = path.with_extension("renamed");
    assert!(!destination.exists());
    assert_eq!(
        fs::rename(&path, &destination).unwrap_err().raw_os_error(),
        Some(32)
    );
    let mut reader = artifact.prepare_reader().unwrap();
    bytes.clear();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, data);
    assert_eq!(
        artifact.prepare_reader().unwrap_err().cause,
        ArtifactError::ReaderAttempted
    );
    reader.close().unwrap();
    assert!(artifact.close().unwrap().complete());
    assert_deleted(&path);
}
#[test]
fn original_file_object_remains_readable_after_owner_close_then_last_reader_deletes() {
    let data = b"owned file object persists only while its existing reader is open";
    let mut artifact = ConfigArtifact::create(&std::env::temp_dir(), data).unwrap();
    let path = artifact.path().unwrap().to_path_buf();
    let mut reader = artifact.prepare_reader().unwrap();
    assert!(artifact.close().unwrap().complete());
    let mut bytes = vec![];
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, data);
    reader.close().unwrap();
    assert_deleted(&path);
}
#[test]
fn maximum_bounded_image_is_exact_and_larger_input_is_rejected_without_owner() {
    let bytes = vec![0x5a; MAX_ARTIFACT_BYTES];
    let mut artifact = ConfigArtifact::create(&std::env::temp_dir(), &bytes).unwrap();
    let path = artifact.path().unwrap().to_path_buf();
    let mut reader = artifact.prepare_reader().unwrap();
    let mut actual = vec![];
    reader.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes);
    reader.close().unwrap();
    assert!(artifact.close().unwrap().complete());
    assert_deleted(&path);
    let error = ConfigArtifact::create(&std::env::temp_dir(), &vec![0; MAX_ARTIFACT_BYTES + 1])
        .unwrap_err();
    assert_eq!(error.cause, ArtifactError::Size);
    assert!(error.owner.is_none());
    assert!(error.rollback.is_none());
}
