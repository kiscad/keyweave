#![allow(clippy::unwrap_used)]
use keyweave_linux::{events::*, read::*};
use std::io::{self, Cursor, Read};
struct Failure(io::ErrorKind);
impl Read for Failure {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(self.0))
    }
}
#[test]
fn whole_native_records_are_decoded_in_one_bounded_read() {
    let event = Event::output(EV_KEY, 30, 1);
    let bytes = encode(Layout::Time64, event).unwrap();
    let mut reader = Cursor::new(bytes);
    assert_eq!(
        read_batch(&mut reader, Layout::Time64).unwrap(),
        ReadBatch::Events(vec![event])
    );
    assert_eq!(
        read_batch(&mut reader, Layout::Time64).unwrap(),
        ReadBatch::Closed
    );
}
#[test]
fn would_block_is_distinct_from_device_closed_and_interrupt_is_not_hidden() {
    assert_eq!(
        read_batch(&mut Failure(io::ErrorKind::WouldBlock), Layout::Time64).unwrap(),
        ReadBatch::WouldBlock
    );
    assert_eq!(
        read_batch(&mut Failure(io::ErrorKind::Interrupted), Layout::Time64)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(
        read_batch(&mut Failure(io::ErrorKind::NotConnected), Layout::Time64)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotConnected
    );
}
#[test]
fn partial_record_is_a_fault_instead_of_a_truncated_key() {
    assert_eq!(
        read_batch(&mut Cursor::new(vec![0; 23]), Layout::Time64)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}
#[test]
fn native_batch_limit_does_not_read_unbounded_stream() {
    let bytes = vec![0; 24 * (MAX_NATIVE_BATCH + 1)];
    let mut reader = Cursor::new(bytes);
    let ReadBatch::Events(events) = read_batch(&mut reader, Layout::Time64).unwrap() else {
        panic!("no records")
    };
    assert_eq!(events.len(), MAX_NATIVE_BATCH);
    assert_eq!(reader.position(), 24 * MAX_NATIVE_BATCH as u64);
}
#[test]
fn invalid_reader_count_returns_error_without_indexing_past_buffer() {
    struct BadReader;
    impl Read for BadReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(usize::MAX)
        }
    }
    assert_eq!(
        read_batch(&mut BadReader, Layout::Time64)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}
