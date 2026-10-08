use crate::events::{Event, Layout, MAX_NATIVE_BATCH, decode_batch};
use std::io::{self, Read};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadBatch {
    Events(Vec<Event>),
    WouldBlock,
    Closed,
}
/// One bounded read. EINTR is returned so the event loop can observe control
/// changes rather than retry forever inside a helper. Partial records are fatal.
pub fn read_batch(source: &mut impl Read, layout: Layout) -> io::Result<ReadBatch> {
    let mut bytes = [0u8; MAX_NATIVE_BATCH * 24];
    let capacity = MAX_NATIVE_BATCH * layout.record_size();
    match source.read(&mut bytes[..capacity]) {
        Ok(0) => Ok(ReadBatch::Closed),
        Ok(count) if count > capacity => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reader exceeded supplied buffer",
        )),
        Ok(count) => decode_batch(layout, &bytes[..count])
            .map(ReadBatch::Events)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(ReadBatch::WouldBlock),
        Err(error) => Err(error),
    }
}
