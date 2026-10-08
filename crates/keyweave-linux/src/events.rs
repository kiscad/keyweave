use thiserror::Error;

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const SYN_REPORT: u16 = 0;
pub const SYN_DROPPED: u16 = 3;
pub const KEY_MAX: u16 = 0x2ff;
pub const MAX_NATIVE_BATCH: usize = 1024;
pub const MAX_FRAME_EVENTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Time32,
    Time64,
}
impl Layout {
    pub fn record_size(self) -> usize {
        match self {
            Self::Time32 => 16,
            Self::Time64 => 24,
        }
    }
    fn time_size(self) -> usize {
        self.record_size() - 8
    }
}
/// Currently audited native ABI: 64-bit x86, ARM and RISC-V Linux.
/// Other targets can use explicit fixture layouts but cannot open native input yet.
pub fn native_layout() -> Option<Layout> {
    #[cfg(all(
        target_os = "linux",
        any(
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "riscv64"
        )
    ))]
    {
        if std::mem::size_of::<libc::input_event>() == 24
            && std::mem::offset_of!(libc::input_event, type_) == 16
            && std::mem::offset_of!(libc::input_event, code) == 18
            && std::mem::offset_of!(libc::input_event, value) == 20
        {
            return Some(Layout::Time64);
        }
    }
    None
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub seconds: i64,
    pub microseconds: i64,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}
impl Event {
    pub fn output(kind: u16, code: u16, value: i32) -> Self {
        Self {
            seconds: 0,
            microseconds: 0,
            kind,
            code,
            value,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ProtocolError {
    #[error("native record length is invalid or not aligned")]
    RecordLength,
    #[error("native batch capacity exceeded")]
    Capacity,
    #[error("invalid native timeval or time32 overflow")]
    InvalidTime,
    #[error("invalid EV_KEY code or value")]
    InvalidKey,
    #[error("resynchronization is not currently requested")]
    InvalidResynchronization,
    #[error("write result cannot identify a known event prefix")]
    AmbiguousWrite,
}
/// Native endian bytes; never reinterpret an unaligned byte slice as a C struct.
pub fn decode(layout: Layout, bytes: &[u8]) -> Result<Event, ProtocolError> {
    if bytes.len() != layout.record_size() {
        return Err(ProtocolError::RecordLength);
    }
    let (seconds, microseconds) = match layout {
        Layout::Time32 => (
            i64::from(i32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
            i64::from(i32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]])),
        ),
        Layout::Time64 => (
            i64::from_ne_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            i64::from_ne_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15],
            ]),
        ),
    };
    if !(0..1_000_000).contains(&microseconds) {
        return Err(ProtocolError::InvalidTime);
    }
    let offset = layout.time_size();
    Ok(Event {
        seconds,
        microseconds,
        kind: u16::from_ne_bytes([bytes[offset], bytes[offset + 1]]),
        code: u16::from_ne_bytes([bytes[offset + 2], bytes[offset + 3]]),
        value: i32::from_ne_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]),
    })
}
pub fn encode(layout: Layout, event: Event) -> Result<Vec<u8>, ProtocolError> {
    if !(0..1_000_000).contains(&event.microseconds) {
        return Err(ProtocolError::InvalidTime);
    }
    let mut bytes = Vec::with_capacity(layout.record_size());
    match layout {
        Layout::Time32 => {
            bytes.extend_from_slice(
                &i32::try_from(event.seconds)
                    .map_err(|_| ProtocolError::InvalidTime)?
                    .to_ne_bytes(),
            );
            bytes.extend_from_slice(
                &i32::try_from(event.microseconds)
                    .map_err(|_| ProtocolError::InvalidTime)?
                    .to_ne_bytes(),
            );
        }
        Layout::Time64 => {
            bytes.extend_from_slice(&event.seconds.to_ne_bytes());
            bytes.extend_from_slice(&event.microseconds.to_ne_bytes());
        }
    }
    bytes.extend_from_slice(&event.kind.to_ne_bytes());
    bytes.extend_from_slice(&event.code.to_ne_bytes());
    bytes.extend_from_slice(&event.value.to_ne_bytes());
    Ok(bytes)
}
pub fn decode_batch(layout: Layout, bytes: &[u8]) -> Result<Vec<Event>, ProtocolError> {
    if !bytes.len().is_multiple_of(layout.record_size()) {
        return Err(ProtocolError::RecordLength);
    }
    if bytes.len() / layout.record_size() > MAX_NATIVE_BATCH {
        return Err(ProtocolError::Capacity);
    }
    bytes
        .chunks_exact(layout.record_size())
        .map(|record| decode(layout, record))
        .collect()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    Frame(Vec<Event>),
    /// Discard this partial frame; stop capture/output normal processing.
    Dropped,
    /// Boundary reached. Query kernel state before any ordinary event resumes.
    Resynchronize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncState {
    Normal,
    Dropping,
    AwaitingSnapshot,
}
/// The raw layer is the sole SYN_DROPPED owner. No synthetic downs are inferred.
pub struct FrameDecoder {
    state: SyncState,
    frame: Vec<Event>,
}
impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}
impl FrameDecoder {
    pub fn new() -> Self {
        Self {
            state: SyncState::Normal,
            frame: Vec::new(),
        }
    }
    pub fn buffered_events(&self) -> usize {
        self.frame.len()
    }
    pub fn needs_resynchronization(&self) -> bool {
        self.state != SyncState::Normal
    }
    pub fn ingest(&mut self, events: &[Event]) -> Result<Vec<Notice>, ProtocolError> {
        if events.len() > MAX_NATIVE_BATCH {
            return Err(ProtocolError::Capacity);
        }
        let mut notices = Vec::new();
        for event in events {
            match self.state {
                SyncState::AwaitingSnapshot => continue,
                SyncState::Dropping => {
                    if event.kind == EV_SYN && event.code == SYN_REPORT {
                        self.state = SyncState::AwaitingSnapshot;
                        notices.push(Notice::Resynchronize);
                    }
                    continue;
                }
                SyncState::Normal => {}
            }
            if event.kind == EV_SYN && event.code == SYN_DROPPED {
                self.frame.clear();
                self.state = SyncState::Dropping;
                notices.push(Notice::Dropped);
            } else if event.kind == EV_SYN && event.code == SYN_REPORT {
                notices.push(Notice::Frame(std::mem::take(&mut self.frame)));
            } else if self.frame.len() >= MAX_FRAME_EVENTS {
                // Local overflow has the same conservative boundary recovery.
                self.frame.clear();
                self.state = SyncState::Dropping;
                notices.push(Notice::Dropped);
            } else {
                self.frame.push(*event);
            }
        }
        Ok(notices)
    }
    /// Caller has queried/reconciled device state and applied its neutral gate.
    pub fn resynchronized(&mut self) -> Result<(), ProtocolError> {
        if self.state != SyncState::AwaitingSnapshot {
            return Err(ProtocolError::InvalidResynchronization);
        }
        self.state = SyncState::Normal;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeKey {
    pub code: u16,
    pub value: i32,
}
/// One EV_KEY per semantic edge plus a final SYN_REPORT; uinput ignores time.
pub fn output_frame(layout: Layout, keys: &[NativeKey]) -> Result<Vec<u8>, ProtocolError> {
    if keys.len() > MAX_FRAME_EVENTS {
        return Err(ProtocolError::Capacity);
    }
    if keys
        .iter()
        .any(|key| key.code == 0 || key.code > KEY_MAX || !(0..=2).contains(&key.value))
    {
        return Err(ProtocolError::InvalidKey);
    }
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut bytes = Vec::with_capacity((keys.len() + 1) * layout.record_size());
    for key in keys {
        bytes.extend(encode(layout, Event::output(EV_KEY, key.code, key.value))?);
    }
    bytes.extend(encode(layout, Event::output(EV_SYN, SYN_REPORT, 0))?);
    Ok(bytes)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWrite {
    Complete {
        accepted: usize,
    },
    /// Even accepted==edge count still needs recovery/flush if SYN_REPORT is absent.
    Incomplete {
        accepted: usize,
        report_pending: bool,
    },
}
/// Convert only record-aligned positive write results; never guess ambiguous bytes.
pub fn classify_write(
    layout: Layout,
    edges: usize,
    written: usize,
) -> Result<FrameWrite, ProtocolError> {
    if edges > MAX_FRAME_EVENTS {
        return Err(ProtocolError::Capacity);
    }
    let records = if edges == 0 { 0 } else { edges + 1 };
    if written > records * layout.record_size() || !written.is_multiple_of(layout.record_size()) {
        return Err(ProtocolError::AmbiguousWrite);
    }
    if written == records * layout.record_size() {
        return Ok(FrameWrite::Complete { accepted: edges });
    }
    Ok(FrameWrite::Incomplete {
        accepted: (written / layout.record_size()).min(edges),
        report_pending: true,
    })
}
