//! Explicit no-input worker fixture for public host control integration tests.
//! Never constructs Runtime/devices or sends an Active/progress receipt.
#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]
#[cfg(target_os = "linux")]
fn main() {
    use keyweave_linux::{
        runtime::{StopReason, TerminationReport},
        worker::{ReportingStartup, WorkerReport, finalize_worker_report},
    };
    use keyweave_progress::{CHALLENGE_BYTES, Challenge, StopRequest};
    use std::{
        io::{self, Read},
        time::{Duration, Instant},
    };
    let args = keyweave_worker::linux_arguments::LinuxArguments::parse(std::env::args_os().skip(1))
        .unwrap();
    assert_eq!(
        args.session_provider,
        keyweave_worker::linux_arguments::SessionProvider::None
    );
    let mut startup = ReportingStartup::from_supervisor_environment().unwrap();
    startup.guard().verify_isolated_group().unwrap();
    assert_eq!(startup.guard().parent_pid(), args.parent);
    assert_eq!(startup.run_id(), args.run_id);
    assert_eq!(
        startup.claim_config().unwrap().read_bytes().unwrap(),
        br#"{"profiles":[{}]}"#
    );
    let (_guard, client, mut terminal) = startup.into_parts();
    drop(client);
    let until = Instant::now() + Duration::from_secs(4);
    let mut frame = [0; CHALLENGE_BYTES];
    let mut read = 0;
    let mut last = 0;
    loop {
        assert!(
            Instant::now() < until,
            "no-input fixture timed out without Stop"
        );
        match io::stdin().read(&mut frame[read..]) {
            Ok(0) => panic!("parent endpoint closed before Stop"),
            Ok(n) => read += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            Err(e) => panic!("{e}"),
        }
        if read < frame.len() {
            continue;
        }
        if let Ok(stop) = StopRequest::decode(&frame) {
            assert_eq!(stop.run_id, args.run_id);
            assert!(stop.id > last);
            break;
        }
        let challenge = Challenge::decode(&frame).unwrap();
        assert_eq!(challenge.run_id, args.run_id);
        assert!(challenge.id > last);
        last = challenge.id;
        // No Runtime poll occurred. Deliberately do not fake a progress response;
        // cooperative Stop must be deliverable while this challenge is outstanding.
        read = 0;
    }
    let report = TerminationReport {
        reason: StopReason::Pause,
        input_owners_released: true,
        engine_reset_confirmed: true,
        recovery_failed: false,
        recovery_budget_exhausted: false,
        submitted_remaining: false,
        report_pending: false,
        acceptance_unknown: false,
        output_recovery_confirmed: true,
        output_owner_dropped: true,
        backend_transition_confirmed: true,
    };
    let sent = finalize_worker_report(
        WorkerReport {
            outcome: Ok(StopReason::Pause),
            termination: Some(report),
        },
        &mut terminal,
    );
    assert!(sent.delivery.frame_sent && sent.delivery.failure.is_none());
}
#[cfg(not(target_os = "linux"))]
fn main() {
    std::process::exit(78);
}
