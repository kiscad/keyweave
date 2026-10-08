#![allow(clippy::unwrap_used)]
use keyweave_linux::events::*;
fn key(value: i32) -> Event {
    Event::output(EV_KEY, 30, value)
}
fn report() -> Event {
    Event::output(EV_SYN, SYN_REPORT, 0)
}
fn drop_marker() -> Event {
    Event::output(EV_SYN, SYN_DROPPED, 0)
}
#[test]
fn time32_and_time64_have_fixed_native_endian_offsets_and_round_trip() {
    let event = Event {
        seconds: 7,
        microseconds: 123456,
        kind: EV_KEY,
        code: 30,
        value: 2,
    };
    for layout in [Layout::Time32, Layout::Time64] {
        let bytes = encode(layout, event).unwrap();
        assert_eq!(bytes.len(), layout.record_size());
        let offset = layout.record_size() - 8;
        assert_eq!(&bytes[offset..offset + 2], &EV_KEY.to_ne_bytes());
        assert_eq!(&bytes[offset + 2..offset + 4], &30u16.to_ne_bytes());
        assert_eq!(&bytes[offset + 4..], &2i32.to_ne_bytes());
        assert_eq!(decode(layout, &bytes).unwrap(), event);
    }
}
#[test]
fn invalid_record_lengths_microseconds_and_time32_overflow_are_explicit() {
    assert_eq!(
        decode(Layout::Time64, &[0; 23]),
        Err(ProtocolError::RecordLength)
    );
    assert_eq!(
        decode_batch(Layout::Time64, &[0; 25]),
        Err(ProtocolError::RecordLength)
    );
    assert_eq!(
        encode(
            Layout::Time32,
            Event {
                seconds: i64::MAX,
                ..key(1)
            }
        ),
        Err(ProtocolError::InvalidTime)
    );
    for microseconds in [-1, 1_000_000] {
        assert_eq!(
            encode(
                Layout::Time64,
                Event {
                    microseconds,
                    ..key(1)
                }
            ),
            Err(ProtocolError::InvalidTime)
        );
        let mut bytes = encode(Layout::Time64, key(1)).unwrap();
        bytes[8..16].copy_from_slice(&microseconds.to_ne_bytes());
        assert_eq!(
            decode(Layout::Time64, &bytes),
            Err(ProtocolError::InvalidTime)
        );
    }
}
#[test]
fn decode_batch_is_bounded_and_does_not_return_a_partial_valid_prefix() {
    let mut bytes = encode(Layout::Time64, key(1)).unwrap();
    let mut bad = encode(Layout::Time64, key(0)).unwrap();
    bad[8..16].copy_from_slice(&(-1i64).to_ne_bytes());
    bytes.extend(bad);
    assert_eq!(
        decode_batch(Layout::Time64, &bytes),
        Err(ProtocolError::InvalidTime)
    );
    assert_eq!(
        decode_batch(Layout::Time64, &vec![0; 24 * (MAX_NATIVE_BATCH + 1)]),
        Err(ProtocolError::Capacity)
    );
}
#[test]
fn frames_span_reads_and_preserve_repeat_scan_and_unknown_records() {
    let mut decoder = FrameDecoder::new();
    let scan = Event::output(4, 4, 0x1234);
    let unknown = Event::output(55, 9, 42);
    assert!(decoder.ingest(&[scan, key(1)]).unwrap().is_empty());
    assert_eq!(
        decoder.ingest(&[key(2), unknown, report()]).unwrap(),
        [Notice::Frame(vec![scan, key(1), key(2), unknown])]
    );
    assert_eq!(decoder.buffered_events(), 0);
}
#[test]
fn dropped_discards_partial_frame_and_ignores_until_report_and_explicit_snapshot() {
    let mut decoder = FrameDecoder::new();
    decoder.ingest(&[key(1)]).unwrap();
    assert_eq!(
        decoder.ingest(&[drop_marker(), key(0)]).unwrap(),
        [Notice::Dropped]
    );
    assert_eq!(decoder.buffered_events(), 0);
    assert!(decoder.needs_resynchronization());
    assert_eq!(
        decoder.resynchronized(),
        Err(ProtocolError::InvalidResynchronization)
    );
    assert_eq!(
        decoder
            .ingest(&[key(1), report(), key(0), report()])
            .unwrap(),
        [Notice::Resynchronize]
    );
    assert!(decoder.ingest(&[key(1), report()]).unwrap().is_empty());
    decoder.resynchronized().unwrap();
    assert!(!decoder.needs_resynchronization());
    assert_eq!(
        decoder.ingest(&[key(1), report()]).unwrap(),
        [Notice::Frame(vec![key(1)])]
    );
    assert_eq!(
        decoder.resynchronized(),
        Err(ProtocolError::InvalidResynchronization)
    );
}
#[test]
fn local_frame_overflow_uses_same_recovery_instead_of_delivering_truncated_keys() {
    let mut decoder = FrameDecoder::new();
    let frame = vec![key(1); MAX_FRAME_EVENTS];
    decoder.ingest(&frame).unwrap();
    assert_eq!(
        decoder.ingest(&[key(0), report()]).unwrap(),
        [Notice::Dropped, Notice::Resynchronize]
    );
    assert_eq!(decoder.buffered_events(), 0);
}
#[test]
fn rejected_native_batch_does_not_mutate_partial_frame() {
    let mut decoder = FrameDecoder::new();
    decoder.ingest(&[key(1)]).unwrap();
    assert_eq!(
        decoder.ingest(&vec![drop_marker(); MAX_NATIVE_BATCH + 1]),
        Err(ProtocolError::Capacity)
    );
    assert!(!decoder.needs_resynchronization());
    assert_eq!(decoder.buffered_events(), 1);
    assert_eq!(
        decoder.ingest(&[report()]).unwrap(),
        [Notice::Frame(vec![key(1)])]
    );
}
#[test]
fn output_frame_has_key_edges_and_exactly_one_report_with_zero_native_time() {
    let bytes = output_frame(
        Layout::Time64,
        &[
            NativeKey { code: 30, value: 1 },
            NativeKey { code: 30, value: 0 },
        ],
    )
    .unwrap();
    assert_eq!(
        decode_batch(Layout::Time64, &bytes).unwrap(),
        [key(1), key(0), report()]
    );
    assert!(output_frame(Layout::Time64, &[]).unwrap().is_empty());
    assert_eq!(
        output_frame(
            Layout::Time64,
            &[NativeKey {
                code: KEY_MAX + 1,
                value: 1
            }]
        ),
        Err(ProtocolError::InvalidKey)
    );
    assert_eq!(
        output_frame(Layout::Time64, &[NativeKey { code: 30, value: 3 }]),
        Err(ProtocolError::InvalidKey)
    );
    assert_eq!(
        output_frame(
            Layout::Time64,
            &vec![NativeKey { code: 30, value: 1 }; MAX_FRAME_EVENTS + 1]
        ),
        Err(ProtocolError::Capacity)
    );
}
#[test]
fn write_prefix_accounts_for_missing_report_even_when_all_key_edges_were_accepted() {
    for layout in [Layout::Time32, Layout::Time64] {
        let size = layout.record_size();
        for records in 0..=2 {
            assert_eq!(
                classify_write(layout, 2, records * size).unwrap(),
                FrameWrite::Incomplete {
                    accepted: records,
                    report_pending: true
                }
            );
        }
        assert_eq!(
            classify_write(layout, 2, 3 * size).unwrap(),
            FrameWrite::Complete { accepted: 2 }
        );
        assert_eq!(
            classify_write(layout, 2, size + 1),
            Err(ProtocolError::AmbiguousWrite)
        );
        assert_eq!(
            classify_write(layout, 2, 4 * size),
            Err(ProtocolError::AmbiguousWrite)
        );
        assert_eq!(
            classify_write(layout, 0, 0).unwrap(),
            FrameWrite::Complete { accepted: 0 }
        );
    }
}
#[test]
fn consecutive_drops_and_empty_reports_do_not_invent_downs() {
    let mut decoder = FrameDecoder::new();
    assert_eq!(
        decoder
            .ingest(&[report(), drop_marker(), drop_marker(), report()])
            .unwrap(),
        [
            Notice::Frame(vec![]),
            Notice::Dropped,
            Notice::Resynchronize
        ]
    );
}
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
))]
#[test]
fn local_native_layout_matches_compiled_libc_uapi_record_size() {
    assert_eq!(native_layout(), Some(Layout::Time64));
    assert_eq!(
        native_layout().unwrap().record_size(),
        std::mem::size_of::<libc::input_event>()
    );
}
