//! Opt-in cooperative stop on the same private ordered command stream.
use super::{CHALLENGE_BYTES, ProgressError, number};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopRequest {
    pub run_id: u64,
    pub id: u64,
}
impl StopRequest {
    pub fn encode(self) -> [u8; CHALLENGE_BYTES] {
        let mut bytes = [0; CHALLENGE_BYTES];
        bytes[..4].copy_from_slice(b"KWPS");
        bytes[4] = 1;
        bytes[8..16].copy_from_slice(&self.run_id.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.id.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProgressError> {
        if bytes.len() != CHALLENGE_BYTES
            || &bytes[..4] != b"KWPS"
            || bytes[4] != 1
            || bytes[5..8] != [0; 3]
        {
            return Err(ProgressError::Frame);
        }
        let request = Self {
            run_id: number(bytes, 8),
            id: number(bytes, 16),
        };
        if request.run_id == 0 || request.id == 0 {
            return Err(ProgressError::Identity);
        }
        Ok(request)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestEvent {
    Pending,
    Challenge,
    Stop,
}
#[cfg(test)]
mod tests;
