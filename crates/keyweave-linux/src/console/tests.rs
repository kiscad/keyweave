use super::*;

fn path() -> &'static Path {
    Path::new("/sys/devices/virtual/input/input42")
}
fn snapshot() -> Snapshot {
    Snapshot {
        active: 2,
        display: 0,
        keyboard: 3,
        tty_session: 91,
        foreground: 92,
        parent_session: 91,
        parent_group: 92,
        worker_session: 91,
    }
}
fn observation() -> Observation {
    Observation {
        before: MonoTime(10),
        after: MonoTime(11),
        first: snapshot(),
        last: snapshot(),
        handler: true,
    }
}
fn inventory(sys: &str, handlers: &str) -> Vec<u8> {
    format!("I: Bus=0006\nS: Sysfs={sys}\nH: Handlers={handlers}\n\n").into_bytes()
}
#[test]
fn handler_must_belong_to_exact_owned_output() {
    let mut bytes = inventory("/devices/virtual/input/input7", "kbd event1 ");
    bytes.extend(inventory("/devices/virtual/input/input42", "event2 "));
    assert!(!has_handler(&bytes, path()).unwrap());
    for handlers in ["kbd", "event2 kbd ", "kbd event2"] {
        assert!(
            has_handler(
                &inventory("/devices/virtual/input/input42", handlers),
                path()
            )
            .unwrap()
        );
    }
    for handlers in ["", "evdev", "kbd2", "prefixkbd", "kbd-suffix"] {
        assert!(
            !has_handler(
                &inventory("/devices/virtual/input/input42", handlers),
                path()
            )
            .unwrap()
        );
    }
    assert!(!has_handler(&[], path()).unwrap());
    assert!(!has_handler(&inventory("/devices/virtual/input/input420", "kbd"), path()).unwrap());
}
#[test]
fn malformed_inventory_and_output_identity_do_not_grant() {
    let bytes = inventory("/devices/virtual/input/input42", "kbd");
    let mut duplicate = bytes.clone();
    duplicate.extend(&bytes);
    let mut nul = bytes.clone();
    nul[0] = 0;
    for bad in [
        duplicate,
        nul,
        bytes[..bytes.len() - 1].to_vec(),
        b"S: Sysfs=/devices/virtual/input/input42\n\n".to_vec(),
        b"S: Sysfs=/devices/virtual/input/input42\nH: Handlers=kbd\nH: Handlers=kbd\n\n".to_vec(),
        b"S: Sysfs=x\nS: Sysfs=x\n\n".to_vec(),
        vec![b'x'; MAX_INVENTORY + 1],
    ] {
        assert!(has_handler(&bad, path()).is_err());
    }
    for bad in [
        "/sys/devices/virtual/input/input",
        "/sys/devices/virtual/input/input042",
        "/sys/devices/virtual/input/input42/event1",
        "/sys/devices/virtual/input/input42\n",
        "/sys/devices/virtual/input/inputx",
        "/devices/virtual/input/input42",
    ] {
        assert!(has_handler(&bytes, Path::new(bad)).is_err());
    }
}
#[test]
fn selected_vt_is_explicit_canonical_kernel_console() {
    assert_eq!(vt_number(Path::new("/dev/tty1")).unwrap(), 1);
    assert_eq!(vt_number(Path::new("/dev/tty63")).unwrap(), 63);
    for bad in [
        "/dev/tty0",
        "/dev/tty64",
        "/dev/tty01",
        "/dev/tty",
        "/dev/tty-1",
        "/dev/tty1/",
        "/dev/console",
        "/dev/pts/1",
        "dev/tty1",
        "/dev/tty1\n",
    ] {
        assert!(vt_number(Path::new(bad)).is_err());
    }
    let file = crate::config_artifact::ConfigArtifact::create(b"not a VT")
        .unwrap()
        .into_file();
    assert!(validate_file(&file, 1).is_err()); // own regular FD; no VT ioctl
}
#[test]
fn ready_is_short_lived_output_scoped_and_anchored_before_query() {
    let ready = grant(path(), 2, (91, 92), observation()).unwrap().unwrap();
    assert!(!ready.fresh_for(path(), MonoTime(9)));
    assert!(ready.fresh_for(path(), MonoTime(10)));
    assert!(ready.fresh_for(path(), MonoTime(10 + CONSOLE_LIFETIME_NS - 1)));
    assert!(!ready.fresh_for(path(), MonoTime(10 + CONSOLE_LIFETIME_NS)));
    assert!(!ready.fresh_for(
        Path::new("/sys/devices/virtual/input/input43"),
        MonoTime(11)
    ));
    assert_eq!(ready.valid_until(), MonoTime(10 + CONSOLE_LIFETIME_NS));
    assert!(!format!("{ready:?}").contains("input42"));
}
#[test]
fn inactive_graphics_raw_other_session_or_group_never_grant() {
    let good = snapshot();
    for bad in [
        Snapshot { active: 1, ..good },
        Snapshot { display: 1, ..good },
        Snapshot {
            keyboard: 0,
            ..good
        },
        Snapshot {
            keyboard: 2,
            ..good
        },
        Snapshot {
            keyboard: 4,
            ..good
        },
        Snapshot {
            tty_session: 0,
            ..good
        },
        Snapshot {
            foreground: 0,
            ..good
        },
        Snapshot {
            parent_session: 90,
            ..good
        },
        Snapshot {
            parent_group: 90,
            ..good
        },
        Snapshot {
            worker_session: 90,
            ..good
        },
    ] {
        let mut obs = observation();
        obs.first = bad;
        obs.last = bad;
        assert!(grant(path(), 2, (91, 92), obs).unwrap().is_none());
    }
    let mut obs = observation();
    obs.first.keyboard = 1;
    obs.last = obs.first;
    assert!(grant(path(), 2, (91, 92), obs).unwrap().is_some());
    assert!(grant(path(), 2, (91, 90), observation()).unwrap().is_none());
    let mut obs = observation();
    obs.handler = false;
    assert!(grant(path(), 2, (91, 92), obs).unwrap().is_none());
    let mut obs = observation();
    obs.last.keyboard = 1; // both eligible, changed sample
    assert!(grant(path(), 2, (91, 92), obs).unwrap().is_none());
}
#[test]
fn slow_queries_do_not_extend_lifetime_and_clock_faults_are_errors() {
    let mut obs = observation();
    obs.after = MonoTime(10 + CONSOLE_LIFETIME_NS);
    assert!(grant(path(), 2, (91, 92), obs).unwrap().is_none());
    let mut obs = observation();
    obs.after = MonoTime(9);
    assert!(grant(path(), 2, (91, 92), obs).is_err());
    let mut obs = observation();
    obs.before = MonoTime(u64::MAX);
    obs.after = obs.before;
    assert!(grant(path(), 2, (91, 92), obs).is_err());
}
