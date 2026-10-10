//! Owned synthetic file only; never starts a worker or calls input APIs.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]
use keyweave_worker::{load_config, preparation::PreparedConfig};
use std::{
    fs,
    io::Read,
    time::{Duration, Instant},
};

#[test]
fn prepared_original_image_seals_reads_compiles_and_deletes_without_input() {
    let bytes = br#"// exact synthetic comment
{"profiles":[{"name":"synthetic","selected":true,"simple_modifications":[{"from":{"key_code":"a"},"to":[{"key_code":"b"}]}]}]}
"#;
    let prepared = PreparedConfig::from_bytes(bytes.to_vec()).unwrap();
    let mut artifact = prepared.seal(&std::env::temp_dir()).unwrap();
    let path = artifact.path().unwrap().to_path_buf();
    let mut reader = artifact.prepare_reader().unwrap();
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes);
    let child = load_config(actual.as_slice()).unwrap();
    assert_eq!(child.generation, prepared.snapshot().generation);
    assert_eq!(child.simple[0].from, prepared.snapshot().simple[0].from);
    assert_eq!(child.simple[0].to, prepared.snapshot().simple[0].to);
    reader.close().unwrap();
    assert!(artifact.close().unwrap().complete());
    let start = Instant::now();
    loop {
        match fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            _ => assert!(start.elapsed() < Duration::from_secs(2)),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
