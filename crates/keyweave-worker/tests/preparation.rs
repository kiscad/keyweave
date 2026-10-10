//! Owned synthetic file only; never starts a worker or calls input APIs.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]
use keyweave_worker::{load_config, preparation::PreparedConfig};
#[test]
fn actual_system_rng_identity_fits_fixed_worker_arguments_on_both_architectures() {
    use keyweave_windows::{RunIdentity, keys::IdentityPolicy};
    use keyweave_worker::{Arguments, WORKER_ARGUMENT, preparation::worker_arguments};
    let identity = RunIdentity::generate().unwrap();
    assert_ne!(identity.run_id(), 0);
    assert!((1..=u32::MAX as usize).contains(&identity.tag().value()));
    let path = std::env::temp_dir().join("synthetic diagnostic path.json");
    let mut args = vec![
        WORKER_ARGUMENT.into(),
        "42".into(),
        identity.run_id().to_string().into(),
    ];
    args.extend(worker_arguments(&path, identity.tag(), IdentityPolicy::StrictUnique).unwrap());
    args.extend(["--keyweave-config-handle".into(), "512".into()]);
    let parsed = Arguments::parse(args).unwrap();
    assert_eq!(parsed.run_id, identity.run_id());
    assert_eq!(parsed.tag, identity.tag());
    // No probabilistic uniqueness assertion: markers are finite identifiers.
}
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
