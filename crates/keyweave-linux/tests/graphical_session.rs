#![allow(clippy::unwrap_used)]
use keyweave_linux::graphical_session::*;
use keyweave_types::MonoTime;
use std::{collections::VecDeque, io};
struct Probe(VecDeque<io::Result<SessionSnapshot>>);
impl SessionQuery for Probe {
    fn query(&mut self, _: SessionSubject) -> io::Result<SessionSnapshot> {
        self.0.pop_front().unwrap()
    }
}
fn subject() -> SessionSubject {
    SessionSubject {
        parent: 42,
        worker: 43,
        uid: 1000,
    }
}
fn snapshot() -> SessionSnapshot {
    SessionSnapshot {
        parent_session: "c1".into(),
        worker_session: "c1".into(),
        uid: 1000,
        active: true,
        remote: false,
        class: "user".into(),
        kind: "x11".into(),
        seat: "seat0".into(),
        display: Some(":0.0".into()),
        state: "active".into(),
    }
}
fn make_observer(values: Vec<io::Result<SessionSnapshot>>) -> SessionObserver<Probe> {
    SessionObserver::new(Probe(values.into()), subject()).unwrap()
}
fn clock(times: &[u64]) -> impl FnMut() -> io::Result<MonoTime> + '_ {
    let mut times = times.iter().copied();
    move || {
        times
            .next()
            .map(MonoTime)
            .ok_or_else(|| io::Error::other("fixture clock exhausted"))
    }
}
#[test]
fn two_stable_snapshots_create_subject_bound_evidence_from_query_start() {
    let mut observer = make_observer(vec![Ok(snapshot()), Ok(snapshot())]);
    let ready = observer.observe(&mut clock(&[100, 200])).unwrap().unwrap();
    assert_eq!(ready.observed_at(), MonoTime(100));
    assert_eq!(ready.valid_until(), MonoTime(100 + SESSION_LIFETIME_NS));
    assert!(ready.fresh_for(subject(), MonoTime(100)));
    assert!(!ready.fresh_for(subject(), MonoTime(99)));
    assert!(!ready.fresh_for(subject(), MonoTime(100 + SESSION_LIFETIME_NS)));
    assert!(!ready.fresh_for(
        SessionSubject {
            uid: 1001,
            ..subject()
        },
        MonoTime(200)
    ));
}
#[test]
fn wayland_metadata_is_supported_without_inventing_x11_display_or_consumer() {
    let mut snapshot = snapshot();
    snapshot.kind = "wayland".into();
    snapshot.display = None;
    let mut observer = make_observer(vec![Ok(snapshot.clone()), Ok(snapshot)]);
    let ready = observer.observe(&mut clock(&[0, 1])).unwrap().unwrap();
    assert_eq!(ready.snapshot().kind, "wayland");
    assert_eq!(ready.snapshot().display, None);
}
#[test]
fn inactive_remote_wrong_user_missing_seat_and_untrusted_display_never_grant() {
    let mut cases = vec![];
    let mut bad = snapshot();
    bad.active = false;
    cases.push(bad);
    let mut bad = snapshot();
    bad.remote = true;
    cases.push(bad);
    let mut bad = snapshot();
    bad.uid = 0;
    cases.push(bad);
    let mut bad = snapshot();
    bad.uid = 1001;
    cases.push(bad);
    let mut bad = snapshot();
    bad.class = "greeter".into();
    cases.push(bad);
    let mut bad = snapshot();
    bad.kind = "tty".into();
    cases.push(bad);
    let mut bad = snapshot();
    bad.seat.clear();
    cases.push(bad);
    let mut bad = snapshot();
    bad.parent_session = "../c1".into();
    bad.worker_session = bad.parent_session.clone();
    cases.push(bad);
    let mut bad = snapshot();
    bad.worker_session = "c2".into();
    cases.push(bad);
    let mut bad = snapshot();
    bad.state = "closing".into();
    cases.push(bad);
    for display in ["remote:0", ":", ":0..1", ":0.", ":0\n", ":0/", ":０"] {
        let mut bad = snapshot();
        bad.display = Some(display.into());
        cases.push(bad);
    }
    for bad in cases {
        let mut observer = make_observer(vec![Ok(bad.clone()), Ok(bad)]);
        assert!(observer.observe(&mut clock(&[0, 1])).unwrap().is_none());
        assert!(observer.current(MonoTime(1)).is_none());
    }
}
#[test]
fn mismatched_second_observation_revokes_previous_evidence() {
    let mut bad = snapshot();
    bad.active = false;
    let mut observer = make_observer(vec![
        Ok(snapshot()),
        Ok(snapshot()),
        Ok(snapshot()),
        Ok(bad),
    ]);
    assert!(observer.observe(&mut clock(&[0, 1])).unwrap().is_some());
    assert!(observer.observe(&mut clock(&[2, 3])).unwrap().is_none());
    assert!(observer.current(MonoTime(3)).is_none());
}
#[test]
fn stable_session_change_never_rebinds_an_existing_run_anchor() {
    for changed in ["session", "seat", "display", "kind"] {
        let mut next = snapshot();
        match changed {
            "session" => {
                next.parent_session = "c2".into();
                next.worker_session = "c2".into();
            }
            "seat" => next.seat = "seat1".into(),
            "display" => next.display = Some(":1".into()),
            _ => {
                next.kind = "wayland".into();
                next.display = None;
            }
        }
        let mut observer = make_observer(vec![
            Ok(snapshot()),
            Ok(snapshot()),
            Ok(next.clone()),
            Ok(next),
        ]);
        observer.observe(&mut clock(&[0, 1])).unwrap();
        assert!(matches!(
            observer.observe(&mut clock(&[2, 3])),
            Err(SessionError::Identity)
        ));
        assert!(observer.current(MonoTime(3)).is_none());
    }
}
#[test]
fn query_error_or_late_completion_retains_no_old_ready() {
    let mut observer = make_observer(vec![
        Ok(snapshot()),
        Ok(snapshot()),
        Err(io::ErrorKind::NotFound.into()),
    ]);
    observer.observe(&mut clock(&[0, 1])).unwrap();
    assert!(matches!(
        observer.observe(&mut clock(&[2])),
        Err(SessionError::Query(_))
    ));
    assert!(observer.current(MonoTime(3)).is_none());
    let mut observer = make_observer(vec![Ok(snapshot()), Ok(snapshot())]);
    assert!(
        observer
            .observe(&mut clock(&[0, SESSION_LIFETIME_NS]))
            .unwrap()
            .is_none()
    );
}
#[test]
fn clock_failure_backwards_and_overflow_never_publish_evidence() {
    let mut observer = make_observer(vec![
        Ok(snapshot()),
        Ok(snapshot()),
        Ok(snapshot()),
        Ok(snapshot()),
    ]);
    observer.observe(&mut clock(&[100, 200])).unwrap();
    assert!(matches!(
        observer.observe(&mut clock(&[199])),
        Err(SessionError::Clock)
    ));
    assert!(observer.current(MonoTime(200)).is_none());
    assert!(matches!(
        observer.observe(&mut clock(&[300, 299])),
        Err(SessionError::Clock)
    ));
    assert!(matches!(
        observer.observe(&mut clock(&[299])),
        Err(SessionError::Clock)
    ));
    let mut observer = make_observer(vec![]);
    assert!(matches!(
        observer.observe(&mut clock(&[u64::MAX])),
        Err(SessionError::Clock)
    ));
}
#[test]
fn invalid_subject_rejects_before_any_query() {
    for subject in [
        SessionSubject {
            parent: 0,
            ..subject()
        },
        SessionSubject {
            worker: 42,
            ..subject()
        },
        SessionSubject {
            uid: 0,
            ..subject()
        },
        SessionSubject {
            parent: u32::MAX,
            ..subject()
        },
    ] {
        assert!(matches!(
            SessionObserver::new(Probe(VecDeque::new()), subject),
            Err(SessionError::Subject)
        ));
    }
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn native_sd_login_export_check_fails_closed_for_untrusted_library_namespace() {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize("/usr/lib/x86_64-linux-gnu/libsystemd.so.0").unwrap();
    let trusted = path.ancestors().all(|node| {
        std::fs::metadata(node)
            .is_ok_and(|metadata| metadata.uid() == 0 && metadata.mode() & 0o022 == 0)
    });
    let result = NativeSessionProbe::check_exports();
    if trusted {
        result.unwrap();
    } else {
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }
}
