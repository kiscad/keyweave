#![allow(clippy::unwrap_used)]
use keyweave_linux::{
    events::{self, Event, FrameWrite, Layout},
    output::WriteError,
    relative::*,
};
use std::io::{self, Write};
struct Writer {
    reply: Option<io::Result<usize>>,
    calls: usize,
    bytes: Vec<u8>,
}
impl Writer {
    fn new(reply: io::Result<usize>) -> Self {
        Self {
            reply: Some(reply),
            calls: 0,
            bytes: vec![],
        }
    }
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls += 1;
        self.bytes.extend(bytes);
        self.reply.take().unwrap()
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("no implicit flush")
    }
}
fn axes() -> RelativeAxes {
    RelativeAxes::new(&SUPPORTED_AXES).unwrap()
}
fn records() -> Vec<Event> {
    vec![
        Event::output(EV_REL, REL_X, -3),
        Event::output(EV_REL, REL_WHEEL_HI_RES, 30),
        Event::output(EV_REL, REL_WHEEL, 1),
        Event::output(EV_REL, REL_WHEEL_HI_RES, 90),
    ]
}
#[test]
fn capability_whitelist_requires_explicit_unique_axes() {
    for bad in [vec![], vec![REL_X, REL_X], vec![2], vec![REL_X; 7]] {
        assert_eq!(RelativeAxes::new(&bad), Err(RelativeError::Axes));
    }
    assert_eq!(axes().codes().collect::<Vec<_>>(), SUPPORTED_AXES);
}
#[test]
fn exact_order_legacy_hires_and_signed_values_survive_both_layouts() {
    let mut original = records();
    original.extend([
        Event::output(EV_REL, REL_HWHEEL_HI_RES, i32::MIN),
        Event::output(EV_REL, REL_Y, i32::MAX),
        Event::output(EV_REL, REL_X, 0),
    ]);
    original[0].seconds = 10;
    original[0].microseconds = 20;
    let frame = RelativeFrame::prepare(&axes(), &original).unwrap();
    assert_eq!(frame.records(), original);
    assert_eq!(
        frame.activity(),
        RelativeActivity {
            moved: true,
            scrolled: true
        }
    );
    for layout in [Layout::Time32, Layout::Time64] {
        let decoded = events::decode_batch(layout, &frame.encode(layout).unwrap()).unwrap();
        assert_eq!(
            decoded[..original.len()],
            original
                .iter()
                .map(|record| Event::output(EV_REL, record.code, record.value))
                .collect::<Vec<_>>()
        );
        assert_eq!(decoded.last(), Some(&Event::output(0, 0, 0)));
    }
}
#[test]
fn known_prefixes_and_report_completion_have_distinct_receipts_without_retry() {
    let frame = RelativeFrame::prepare(&axes(), &records()).unwrap();
    for layout in [Layout::Time32, Layout::Time64] {
        for accepted in 0..=4 {
            let mut writer = Writer::new(Ok(accepted * layout.record_size()));
            assert_eq!(
                frame.write_once(&mut writer, layout).unwrap(),
                FrameWrite::Incomplete {
                    accepted,
                    report_pending: true
                }
            );
            assert_eq!(writer.calls, 1);
        }
        let mut writer = Writer::new(Ok(5 * layout.record_size()));
        assert_eq!(
            frame.write_once(&mut writer, layout).unwrap(),
            FrameWrite::Complete { accepted: 4 }
        );
        assert_eq!(writer.calls, 1);
    }
}
#[test]
fn errno_unaligned_and_oversized_writes_remain_unknown_acceptance() {
    let frame = RelativeFrame::prepare(&axes(), &records()).unwrap();
    for reply in [
        Err(io::ErrorKind::WouldBlock.into()),
        Err(io::ErrorKind::Interrupted.into()),
        Ok(1),
        Ok(121),
        Ok(144),
    ] {
        let mut writer = Writer::new(reply);
        assert!(matches!(
            frame.write_once(&mut writer, Layout::Time64),
            Err(WriteError::UnknownAcceptance(_))
        ));
        assert_eq!(writer.calls, 1);
    }
}
#[test]
fn empty_frame_never_calls_writer_or_fabricates_report() {
    let frame = RelativeFrame::prepare(&axes(), &[]).unwrap();
    let mut writer = Writer::new(Err(io::ErrorKind::Other.into()));
    assert_eq!(
        frame.write_once(&mut writer, Layout::Time64).unwrap(),
        FrameWrite::Complete { accepted: 0 }
    );
    assert_eq!(writer.calls, 0);
    assert!(frame.encode(Layout::Time32).unwrap().is_empty());
}
#[test]
fn unadvertised_or_non_relative_records_reject_before_output() {
    let limited = RelativeAxes::new(&[REL_X]).unwrap();
    for event in [
        Event::output(EV_REL, REL_WHEEL, 1),
        Event::output(1, 30, 1),
        Event::output(0, 0, 0),
        Event::output(EV_REL, 2, 1),
    ] {
        assert_eq!(
            RelativeFrame::prepare(&limited, &[event]).unwrap_err(),
            RelativeError::Record
        );
    }
}
#[test]
fn activity_uses_nonzero_records_without_dropping_opposite_or_small_hires_deltas() {
    for (input, expected) in [
        (
            vec![
                Event::output(EV_REL, REL_X, 0),
                Event::output(EV_REL, REL_WHEEL_HI_RES, 0),
            ],
            RelativeActivity::default(),
        ),
        (
            vec![
                Event::output(EV_REL, REL_X, 1),
                Event::output(EV_REL, REL_X, -1),
            ],
            RelativeActivity {
                moved: true,
                scrolled: false,
            },
        ),
        (
            vec![Event::output(EV_REL, REL_HWHEEL_HI_RES, 1)],
            RelativeActivity {
                moved: false,
                scrolled: true,
            },
        ),
    ] {
        let frame = RelativeFrame::prepare(&axes(), &input).unwrap();
        assert_eq!(frame.activity(), expected);
        assert_eq!(frame.records(), input);
    }
}
#[test]
fn frame_limit_includes_all_relative_records_and_one_report_only() {
    let input = vec![Event::output(EV_REL, REL_X, 1); events::MAX_FRAME_EVENTS];
    let frame = RelativeFrame::prepare(&axes(), &input).unwrap();
    assert_eq!(
        frame.encode(Layout::Time64).unwrap().len(),
        (events::MAX_FRAME_EVENTS + 1) * 24
    );
    assert_eq!(
        RelativeFrame::prepare(&axes(), &vec![input[0]; events::MAX_FRAME_EVENTS + 1]).unwrap_err(),
        RelativeError::Capacity
    );
}
