#![allow(clippy::unwrap_used)]
use keyweave_linux::{
    events::{self, Event, FrameWrite, Layout, NativeKey},
    output::{DeviceSpec, OutputChannel, OutputState, RepeatPolicy, WriteError},
    relative::*,
};
use std::{
    cell::RefCell,
    collections::VecDeque,
    io::{self, Write},
    rc::Rc,
};
type Calls = Rc<RefCell<Vec<Vec<u8>>>>;
struct Writer {
    replies: VecDeque<io::Result<usize>>,
    calls: Calls,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.calls.borrow_mut().push(bytes.to_vec());
        self.replies.pop_front().unwrap_or(Ok(bytes.len()))
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("no hidden flush")
    }
}
fn spec() -> DeviceSpec {
    DeviceSpec::new("channel", &[30], RepeatPolicy::Kernel, false).unwrap()
}
fn axes() -> RelativeAxes {
    RelativeAxes::new(&[REL_X, REL_Y]).unwrap()
}
fn frame() -> RelativeFrame {
    RelativeFrame::prepare(
        &axes(),
        &[
            Event::output(EV_REL, REL_X, 1),
            Event::output(EV_REL, REL_Y, -1),
        ],
    )
    .unwrap()
}
fn channel(spec: DeviceSpec, replies: Vec<io::Result<usize>>) -> (OutputChannel<Writer>, Calls) {
    let calls = Rc::new(RefCell::new(vec![]));
    (
        OutputChannel::new(
            Writer {
                replies: replies.into(),
                calls: calls.clone(),
            },
            Layout::Time64,
            spec,
        ),
        calls,
    )
}
#[test]
fn explicit_relative_spec_preserves_keyboard_metadata_and_full_frame_values() {
    let base = spec();
    let mixed = base.clone().with_relative_axes(axes());
    assert!(base.relative_axes().is_none());
    assert_eq!(base.setup_bytes(), mixed.setup_bytes());
    assert_eq!(
        base.codes().collect::<Vec<_>>(),
        mixed.codes().collect::<Vec<_>>()
    );
    let (mut channel, calls) = channel(mixed, vec![]);
    assert_eq!(
        channel.emit_relative(&frame()).unwrap().unwrap(),
        FrameWrite::Complete { accepted: 2 }
    );
    assert_eq!(channel.state(), OutputState::Ready);
    let events = events::decode_batch(Layout::Time64, &calls.borrow()[0]).unwrap();
    assert_eq!(
        events,
        [
            Event::output(EV_REL, REL_X, 1),
            Event::output(EV_REL, REL_Y, -1),
            Event::output(0, 0, 0)
        ]
    );
}
#[test]
fn cold_relative_capability_rejection_never_calls_writer_or_changes_state() {
    for spec in [
        spec(),
        spec().with_relative_axes(RelativeAxes::new(&[REL_X]).unwrap()),
    ] {
        let (mut channel, calls) = channel(spec, vec![]);
        assert!(channel.emit_relative(&frame()).is_err());
        assert_eq!(channel.state(), OutputState::Ready);
        assert!(calls.borrow().is_empty());
    }
}
#[test]
fn real_channel_short_relative_requires_report_then_allows_only_key_recovery() {
    let (mut channel, calls) = channel(
        spec().with_relative_axes(axes()),
        vec![Ok(24), Ok(0), Ok(24), Ok(48)],
    );
    assert_eq!(
        channel.emit_relative(&frame()).unwrap().unwrap(),
        FrameWrite::Incomplete {
            accepted: 1,
            report_pending: true
        }
    );
    assert_eq!(channel.state(), OutputState::ReportPending);
    assert!(channel.emit(&[NativeKey { code: 30, value: 1 }]).is_err());
    assert!(
        channel
            .release_submitted(&[NativeKey { code: 30, value: 0 }])
            .is_err()
    );
    assert!(channel.emit_relative(&frame()).is_err());
    assert!(!channel.flush_report().unwrap().unwrap());
    assert_eq!(channel.state(), OutputState::ReportPending);
    assert!(channel.flush_report().unwrap().unwrap());
    assert_eq!(channel.state(), OutputState::Recovery);
    assert!(channel.emit_relative(&frame()).is_err());
    assert!(
        channel
            .release_submitted(&[NativeKey { code: 30, value: 1 }])
            .is_err()
    );
    channel
        .release_submitted(&[NativeKey { code: 30, value: 0 }])
        .unwrap()
        .unwrap();
    assert_eq!(calls.borrow().len(), 4);
}
#[test]
fn real_channel_unknown_relative_write_freezes_all_later_output_without_retry() {
    let (mut channel, calls) = channel(
        spec().with_relative_axes(axes()),
        vec![Err(io::ErrorKind::WouldBlock.into())],
    );
    assert!(matches!(
        channel.emit_relative(&frame()).unwrap(),
        Err(WriteError::UnknownAcceptance(_))
    ));
    assert_eq!(channel.state(), OutputState::Quarantined);
    assert!(channel.emit_relative(&frame()).is_err());
    assert!(channel.emit(&[NativeKey { code: 30, value: 1 }]).is_err());
    assert!(
        channel
            .release_submitted(&[NativeKey { code: 30, value: 0 }])
            .is_err()
    );
    assert!(channel.flush_report().is_err());
    assert_eq!(calls.borrow().len(), 1);
}
#[test]
fn real_channel_keyboard_short_and_relative_share_one_report_state() {
    let (mut channel, calls) = channel(
        spec().with_relative_axes(axes()),
        vec![Ok(24), Ok(24), Ok(48)],
    );
    channel
        .emit(&[NativeKey { code: 30, value: 1 }])
        .unwrap()
        .unwrap();
    assert_eq!(channel.state(), OutputState::ReportPending);
    assert!(channel.emit_relative(&frame()).is_err());
    channel.flush_report().unwrap().unwrap();
    channel
        .release_submitted(&[NativeKey { code: 30, value: 0 }])
        .unwrap()
        .unwrap();
    assert_eq!(calls.borrow().len(), 3);
}
#[test]
fn empty_relative_frame_has_no_write_and_does_not_unlock_recovery() {
    let (mut channel, calls) = channel(spec(), vec![]);
    let empty = RelativeFrame::prepare(&axes(), &[]).unwrap();
    assert_eq!(
        channel.emit_relative(&empty).unwrap().unwrap(),
        FrameWrite::Complete { accepted: 0 }
    );
    assert!(calls.borrow().is_empty());
    channel.release_submitted(&[]).unwrap().unwrap();
    assert_eq!(channel.state(), OutputState::Recovery);
    assert!(channel.emit_relative(&empty).is_err());
    assert!(calls.borrow().is_empty());
}
#[test]
fn real_channel_misaligned_or_oversized_relative_success_is_unknown() {
    for count in [1, 73, 96] {
        let (mut channel, calls) = channel(spec().with_relative_axes(axes()), vec![Ok(count)]);
        assert!(matches!(
            channel.emit_relative(&frame()).unwrap(),
            Err(WriteError::UnknownAcceptance(_))
        ));
        assert_eq!(channel.state(), OutputState::Quarantined);
        assert_eq!(calls.borrow().len(), 1);
    }
}
struct Backend(OutputChannel<Writer>);
impl keyweave_linux::pump::FrameBackend for Backend {
    fn spec(&self) -> &DeviceSpec {
        self.0.spec()
    }
    fn relative_axes(&self) -> Option<&RelativeAxes> {
        self.0.spec().relative_axes()
    }
    fn submit(
        &mut self,
        edges: &[NativeKey],
        recovery: bool,
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        if recovery {
            self.0.release_submitted(edges)
        } else {
            self.0.emit(edges)
        }
    }
    fn submit_relative(
        &mut self,
        frame: &RelativeFrame,
    ) -> io::Result<Result<FrameWrite, WriteError>> {
        self.0.emit_relative(frame)
    }
    fn flush_report(&mut self) -> io::Result<Result<bool, WriteError>> {
        self.0.flush_report()
    }
}
#[test]
fn actual_pump_and_production_channel_recover_after_relative_short_in_same_order() {
    use keyweave_linux::pump::{OutputPump, Phase};
    use keyweave_types::*;
    let (channel, calls) = channel(
        spec().with_relative_axes(axes()),
        vec![Ok(48), Ok(24), Ok(24), Ok(48)],
    );
    let mut pump = OutputPump::new(Backend(channel));
    pump.resume().unwrap();
    pump.enqueue_mixed(
        &[OutputIntent {
            at_ns: MonoTime(0),
            key: "a".parse().unwrap(),
            edge: KeyEdge::Down,
        }],
        &[(MonoTime(0), frame())],
    )
    .unwrap();
    pump.poll(MonoTime(0)).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert_eq!(pump.phase(), Phase::ReportPending);
    assert!(!pump.ledger().submitted().is_empty());
    pump.poll(MonoTime(0)).unwrap();
    pump.poll(MonoTime(0)).unwrap();
    assert!(pump.ledger().submitted().is_empty());
    let calls = calls.borrow();
    assert_eq!(calls.len(), 4);
    assert_eq!(
        events::decode_batch(Layout::Time64, &calls[2]).unwrap(),
        [Event::output(0, 0, 0)]
    );
    assert_eq!(
        events::decode_batch(Layout::Time64, &calls[3]).unwrap(),
        [Event::output(1, 30, 0), Event::output(0, 0, 0)]
    );
}
#[test]
fn pointing_spec_checks_shape_replacement_and_unadvertised_buttons_before_write() {
    use keyweave_linux::keys::{BTN_LEFT, PointingButtons};
    let base = spec().with_relative_axes(axes());
    let both = base
        .with_pointing_buttons(PointingButtons::new(&[BTN_LEFT, BTN_LEFT + 1]).unwrap())
        .unwrap();
    let narrowed = both
        .with_pointing_buttons(PointingButtons::new(&[BTN_LEFT]).unwrap())
        .unwrap();
    assert!(!narrowed.codes().any(|code| code == BTN_LEFT + 1));
    let (mut wire, calls) = channel(narrowed.clone(), vec![]);
    assert!(
        wire.emit(&[NativeKey {
            code: BTN_LEFT + 1,
            value: 1
        }])
        .is_err()
    );
    assert!(calls.borrow().is_empty());
    wire.emit(&[NativeKey {
        code: BTN_LEFT,
        value: 1,
    }])
    .unwrap()
    .unwrap();
    assert_eq!(
        events::decode_batch(Layout::Time64, &calls.borrow()[0]).unwrap()[0],
        Event::output(1, BTN_LEFT, 1)
    );
    let bad = narrowed.with_relative_axes(RelativeAxes::new(&[REL_WHEEL]).unwrap());
    assert!(bad.validate_shape().is_err());
    let (mut bad_channel, calls) = channel(bad, vec![]);
    assert!(
        bad_channel
            .emit(&[NativeKey {
                code: BTN_LEFT,
                value: 1
            }])
            .is_err()
    );
    assert!(calls.borrow().is_empty());
}
