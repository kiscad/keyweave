#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]
use keyweave_linux::x11_probe::*;
use keyweave_types::MonoTime;
use serde_json::json;
use std::{
    fs, io,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};
fn target() -> ProbeTarget {
    ProbeTarget::new(":0", std::path::Path::new("/dev/input/event42"), 13, 106).unwrap()
}
fn body() -> serde_json::Value {
    json!({"schema_version":1,"display":":0","node":"/dev/input/event42","major":13,"minor":106,"devices":[{"id":10,"attachment":2,"kind":"keyboard","keys":[38,39],"buttons":0}]})
}
struct Fixture {
    root: PathBuf,
    image: PathBuf,
}
impl Fixture {
    fn new(name: &str, script: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("keyweave-x11-job-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let image = root.join("probe");
        fs::write(&image, format!("#!/usr/bin/python3\n{script}\n")).unwrap();
        fs::set_permissions(&image, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, image }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn finish(job: &mut ProbeJob, at: u64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if job.poll(MonoTime(at)).unwrap() == ProbePhase::Exited {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "same child observation limit expired"
        );
        std::thread::yield_now();
    }
}
#[test]
fn protocol_is_exact_target_bound_and_rejects_ambiguous_or_oversized_fields() {
    assert_eq!(
        decode_report(&serde_json::to_vec(&body()).unwrap(), &target())
            .unwrap()
            .devices
            .len(),
        1
    );
    let mut cases = vec![];
    for (field, value) in [
        ("schema_version", json!(2)),
        ("display", json!(":1")),
        ("node", json!("/dev/input/event43")),
        ("minor", json!(107)),
        ("ready", json!(true)),
    ] {
        let mut bad = body();
        bad[field] = value;
        cases.push(bad);
    }
    for (field, value) in [
        ("id", json!(0)),
        ("attachment", json!(10)),
        ("kind", json!("master")),
        ("keys", json!([39, 38])),
        ("keys", json!([38, 38])),
        ("buttons", json!(257)),
    ] {
        let mut bad = body();
        bad["devices"][0][field] = value;
        cases.push(bad);
    }
    let mut bad = body();
    bad["devices"] = json!([bad["devices"][0].clone(), bad["devices"][0].clone()]);
    cases.push(bad);
    for bad in cases {
        assert!(decode_report(&serde_json::to_vec(&bad).unwrap(), &target()).is_err());
    }
    let duplicate=br#"{"schema_version":1,"schema_version":1,"display":":0","node":"/dev/input/event42","major":13,"minor":106,"devices":[]}"#;
    assert!(decode_report(duplicate, &target()).is_err());
    assert!(decode_report(&vec![b' '; MAX_RESPONSE_BYTES + 1], &target()).is_err());
}
#[test]
fn actual_owned_fixture_requires_eof_and_success_before_one_time_report() {
    let json = body().to_string();
    let fixture = Fixture::new(
        "success",
        &format!("import sys\nsys.stdout.write({json:?})\nsys.stdout.flush()"),
    );
    let mut job = ProbeJob::spawn(&fixture.image, target(), MonoTime(0), MAX_PROBE_NS).unwrap();
    assert!(job.take_report().is_none());
    finish(&mut job, 1);
    assert!(job.exit_status().unwrap().success());
    assert_eq!(job.failure(), None);
    assert_eq!(job.take_report().unwrap().devices.len(), 1);
    assert!(job.take_report().is_none());
    assert_eq!(job.next_deadline(), None);
}
#[test]
fn timeout_requests_stop_and_retains_same_child_until_actual_exit() {
    let fixture = Fixture::new("timeout", "import time\ntime.sleep(10)");
    let mut job = ProbeJob::spawn(&fixture.image, target(), MonoTime(100), 100).unwrap();
    job.poll(MonoTime(200)).unwrap();
    assert_eq!(job.failure(), Some(ProbeFailure::Timeout));
    finish(&mut job, 200);
    assert!(job.exit_status().is_some());
    assert!(job.take_report().is_none());
}
#[test]
fn cancel_and_clock_fault_never_publish_even_a_valid_partial_report() {
    for cancel in [true, false] {
        let name = if cancel { "cancel" } else { "clock" };
        let fixture = Fixture::new(name, "import time\ntime.sleep(10)");
        let mut job =
            ProbeJob::spawn(&fixture.image, target(), MonoTime(100), MAX_PROBE_NS).unwrap();
        if cancel {
            job.request_stop().unwrap();
        } else {
            job.poll(MonoTime(99)).unwrap();
        }
        finish(&mut job, 100);
        assert_eq!(
            job.failure(),
            Some(if cancel {
                ProbeFailure::Requested
            } else {
                ProbeFailure::Clock
            })
        );
        assert!(job.take_report().is_none());
    }
}
#[test]
fn bad_exit_or_protocol_remains_failure_after_observed_child_exit() {
    for (name, script, failure) in [
        ("exit", "import sys\nsys.exit(3)", ProbeFailure::Exit),
        ("malformed", "print('not metadata')", ProbeFailure::Protocol),
    ] {
        let fixture = Fixture::new(name, script);
        let mut job = ProbeJob::spawn(&fixture.image, target(), MonoTime(0), MAX_PROBE_NS).unwrap();
        finish(&mut job, 1);
        assert_eq!(job.failure(), Some(failure));
        assert!(job.take_report().is_none());
    }
}
#[test]
fn oversized_child_output_is_stopped_without_unbounded_buffer_or_report() {
    let fixture = Fixture::new(
        "capacity",
        "import sys\nsys.stdout.write('x'*300000)\nsys.stdout.flush()",
    );
    let mut job = ProbeJob::spawn(&fixture.image, target(), MonoTime(0), MAX_PROBE_NS).unwrap();
    finish(&mut job, 1);
    assert_eq!(job.failure(), Some(ProbeFailure::OutputLimit));
    assert!(job.exit_status().is_some());
    assert!(job.take_report().is_none());
}
#[test]
fn invalid_target_image_timeout_and_overflow_reject_before_spawn() {
    assert!(
        ProbeTarget::new(
            "remote:0",
            std::path::Path::new("/dev/input/event42"),
            13,
            106
        )
        .is_err()
    );
    assert!(ProbeTarget::new(":0", std::path::Path::new("/tmp/event42"), 13, 106).is_err());
    let fixture = Fixture::new("policy", "raise RuntimeError('must not execute')");
    for timeout in [0, MAX_PROBE_NS + 1] {
        assert_eq!(
            ProbeJob::spawn(&fixture.image, target(), MonoTime(0), timeout)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
    assert!(ProbeJob::spawn(&fixture.image, target(), MonoTime(u64::MAX), 1).is_err());
    assert!(ProbeJob::spawn(std::path::Path::new("relative"), target(), MonoTime(0), 1).is_err());
}
#[test]
fn empty_device_metadata_is_completed_probe_but_not_ready() {
    let mut body = body();
    body["devices"] = json!([]);
    let report = decode_report(&serde_json::to_vec(&body).unwrap(), &target()).unwrap();
    assert!(report.devices.is_empty());
}

#[test]
fn actual_owned_schema2_fixture_reaches_motion_coverage_after_exit() {
    let mut body = body();
    body["schema_version"] = json!(2);
    body["devices"] = json!([{"id":10,"attachment":2,"kind":"pointer","keys":[],"buttons":3,"relative_motion_axes":[0,1]}]);
    let json = body.to_string();
    let fixture = Fixture::new(
        "motion-v2",
        &format!("import sys\nsys.stdout.write({json:?})\nsys.stdout.flush()"),
    );
    let mut job = ProbeJob::spawn(&fixture.image, target(), MonoTime(0), MAX_PROBE_NS).unwrap();
    finish(&mut job, 1);
    assert!(job.exit_status().unwrap().success());
    let report = job.take_report().unwrap();
    let requirements = CoverageRequirements::with_relative_motion(&[], &[1, 3]).unwrap();
    assert_eq!(
        report
            .check_coverage(&target(), &requirements)
            .unwrap()
            .pointer
            .unwrap()
            .slave,
        10
    );
    assert!(job.take_report().is_none());
}
