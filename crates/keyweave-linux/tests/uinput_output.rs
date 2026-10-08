use keyweave_linux::{
    events::{self, FrameWrite, Layout, NativeKey},
    output::{self, DeviceSpec, RepeatPolicy, WriteError},
};
use std::io::{self, Write};
struct Writer {
    result: Option<io::Result<usize>>,
    calls: usize,
    bytes: Vec<u8>,
}
impl Writer {
    fn new(result: io::Result<usize>) -> Self {
        Self {
            result: Some(result),
            calls: 0,
            bytes: Vec::new(),
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        self.bytes = bytes.to_vec();
        self.result
            .take()
            .unwrap_or_else(|| panic!("unexpected retry"))
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("no stdio flush")
    }
}
const EDGES: [NativeKey; 2] = [
    NativeKey { code: 30, value: 1 },
    NativeKey { code: 30, value: 0 },
];
#[test]
fn complete_frame_has_one_final_report() {
    let mut writer = Writer::new(Ok(72));
    assert!(matches!(
        output::write_frame(&mut writer, Layout::Time64, &EDGES),
        Ok(FrameWrite::Complete { accepted: 2 })
    ));
    let records = events::decode_batch(Layout::Time64, &writer.bytes)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(records.len(), 3);
    assert_eq!(records[2], events::Event::output(0, 0, 0));
    assert_eq!(writer.calls, 1);
}
#[test]
fn every_known_prefix_is_distinct_from_full_frame() {
    for (bytes, accepted) in [(0, 0), (24, 1), (48, 2)] {
        let mut writer = Writer::new(Ok(bytes));
        assert!(
            matches!(output::write_frame(&mut writer, Layout::Time64, &EDGES), Ok(FrameWrite::Incomplete { accepted: count, report_pending: true }) if count == accepted)
        );
        assert_eq!(writer.calls, 1);
    }
}
#[test]
fn errno_and_misaligned_success_do_not_invent_zero_acceptance() {
    for result in [
        Err(io::Error::from_raw_os_error(14)),
        Err(io::Error::from(io::ErrorKind::WouldBlock)),
        Ok(25),
        Ok(96),
    ] {
        let mut writer = Writer::new(result);
        assert!(matches!(
            output::write_frame(&mut writer, Layout::Time64, &EDGES),
            Err(WriteError::UnknownAcceptance(_))
        ));
        assert_eq!(writer.calls, 1);
    }
}
#[test]
fn invalid_or_empty_frames_do_not_call_writer() {
    let mut writer = Writer::new(Ok(0));
    assert!(matches!(
        output::write_frame(&mut writer, Layout::Time64, &[]),
        Ok(FrameWrite::Complete { accepted: 0 })
    ));
    assert!(matches!(
        output::write_frame(
            &mut writer,
            Layout::Time64,
            &[NativeKey { code: 0, value: 1 }]
        ),
        Err(WriteError::InvalidFrame(_))
    ));
    assert_eq!(writer.calls, 0);
}
#[test]
fn report_flush_is_single_record_and_never_blindly_retried() {
    for (count, expected) in [(0, Some(false)), (24, Some(true)), (1, None), (48, None)] {
        let mut writer = Writer::new(Ok(count));
        match expected {
            Some(value) => assert_eq!(
                output::flush_report(&mut writer, Layout::Time64).ok(),
                Some(value)
            ),
            None => assert!(matches!(
                output::flush_report(&mut writer, Layout::Time64),
                Err(WriteError::UnknownAcceptance(_))
            )),
        }
        assert_eq!(writer.calls, 1);
        assert_eq!(writer.bytes.len(), 24);
    }
}
#[test]
fn setup_and_advertised_keys_are_bounded() {
    let spec = DeviceSpec::new("KeyWeave", &[30, 30, 48], RepeatPolicy::Kernel, true)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(spec.codes().collect::<Vec<_>>(), vec![30, 48]);
    let setup = spec.setup_bytes();
    assert_eq!(setup.len(), 92);
    assert_eq!(&setup[..2], &6u16.to_ne_bytes());
    assert_eq!(&setup[8..17], b"KeyWeave\0");
    assert_eq!(&setup[88..], &[0; 4]);
    for (name, codes) in [
        ("", vec![30]),
        ("a\0b", vec![30]),
        ("ok", vec![]),
        ("ok", vec![0]),
        ("ok", vec![767]),
    ] {
        assert!(DeviceSpec::new(name, &codes, RepeatPolicy::Kernel, false).is_err());
    }
    assert!(DeviceSpec::new(&"a".repeat(80), &[30], RepeatPolicy::Kernel, false).is_err());
}
#[test]
fn repeat_sources_cannot_be_combined() {
    let kernel = DeviceSpec::new("test", &[30], RepeatPolicy::Kernel, false)
        .unwrap_or_else(|error| panic!("{error}"));
    let external = DeviceSpec::new("test", &[30], RepeatPolicy::External, false)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        kernel
            .validate(&[NativeKey { code: 30, value: 2 }])
            .is_err()
    );
    assert!(
        external
            .validate(&[NativeKey { code: 30, value: 2 }])
            .is_ok()
    );
    assert!(
        external
            .validate(&[NativeKey { code: 48, value: 1 }])
            .is_err()
    );
}
#[cfg(target_os = "linux")]
#[test]
fn construction_rejects_non_uinput_without_creating_device() {
    let spec = DeviceSpec::new("test", &[30], RepeatPolicy::Kernel, false)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        keyweave_linux::uinput::UinputDevice::create(std::path::Path::new("/dev/null"), spec)
            .is_err()
    );
}
