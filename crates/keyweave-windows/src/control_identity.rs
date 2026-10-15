//! Bounded identity values for local Windows control. No process/input APIs.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ControlIdentityError {
    #[error("control identity operation in input callback")]
    InCallback,
    #[error("control identity resources unresolved; restart required")]
    Poisoned,
    #[error("caller thread is impersonating")]
    Impersonating,
    #[error("invalid or oversized control principal metadata")]
    Malformed,
    #[error("control principal changed during observation")]
    Changed,
    #[error("session zero is not an interactive control principal")]
    SessionZero,
    #[error("control principal does not match this caller")]
    CallerChanged,
    #[error("{call} failed: {code}")]
    Api { call: &'static str, code: u32 },
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "control identity failed: operation={operation:?}, token_close={token_close:?}, process_close={process_close:?}, sid_text_free={sid_text_free:?}"
)]
pub struct ControlIdentityFailure {
    pub operation: Option<ControlIdentityError>,
    pub token_close: Option<u32>,
    pub process_close: Option<u32>,
    pub sid_text_free: Option<u32>,
}
impl ControlIdentityFailure {
    pub(crate) fn operation(error: ControlIdentityError) -> Self {
        Self {
            operation: Some(error),
            token_close: None,
            process_close: None,
            sid_text_free: None,
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Sid {
    bytes: [u8; 68],
    length: usize,
}
impl Sid {
    pub(crate) fn binary(bytes: &[u8]) -> Result<Self, ControlIdentityError> {
        if bytes.len() < 8
            || bytes[0] != 1
            || bytes[1] > 15
            || bytes.len() != 8 + usize::from(bytes[1]) * 4
        {
            return Err(ControlIdentityError::Malformed);
        }
        let mut result = Self {
            bytes: [0; 68],
            length: bytes.len(),
        };
        result.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(result)
    }
    pub(crate) fn text(text: &str) -> Result<Self, ControlIdentityError> {
        if text.len() > 256 || !text.is_ascii() {
            return Err(ControlIdentityError::Malformed);
        }
        let mut parts = text.split('-');
        if parts.next() != Some("S") || parts.next() != Some("1") {
            return Err(ControlIdentityError::Malformed);
        }
        let authority = parts.next().ok_or(ControlIdentityError::Malformed)?;
        let authority = if let Some(hex) = authority
            .strip_prefix("0x")
            .or_else(|| authority.strip_prefix("0X"))
        {
            if hex.is_empty() || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err(ControlIdentityError::Malformed);
            }
            u64::from_str_radix(hex, 16)
        } else {
            if authority.is_empty() || !authority.bytes().all(|c| c.is_ascii_digit()) {
                return Err(ControlIdentityError::Malformed);
            }
            authority.parse()
        }
        .map_err(|_| ControlIdentityError::Malformed)?;
        if authority > 0xffff_ffff_ffff {
            return Err(ControlIdentityError::Malformed);
        }
        let mut bytes = vec![1, 0];
        bytes.extend_from_slice(&authority.to_be_bytes()[2..]);
        for word in parts {
            if bytes[1] == 15 || word.is_empty() || !word.bytes().all(|c| c.is_ascii_digit()) {
                return Err(ControlIdentityError::Malformed);
            }
            let word: u32 = word.parse().map_err(|_| ControlIdentityError::Malformed)?;
            bytes.extend_from_slice(&word.to_le_bytes());
            bytes[1] += 1;
        }
        Self::binary(&bytes)
    }
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Principal {
    pub sid: Sid,
    pub logon: [u8; 8],
    pub session: u32,
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn sid_text_roundtrip_is_bounded_and_cannot_inject_an_acl() {
        let sid = Sid::text("S-1-5-21-100").unwrap();
        assert!(Sid::binary(sid.bytes()).unwrap() == sid);
        assert!(Sid::text("S-1-0x010000000001-4294967295").is_ok());
        for bad in [
            "S-1-5-21);(A;;GA;;;WD)",
            "S-1-5-+1",
            "S-1-5--1",
            "S-1-5-4294967296",
            "S-1-0x1000000000000-1",
            "s-1-5-1",
            "S-2-5-1",
        ] {
            assert!(Sid::text(bad).is_err(), "{bad}");
        }
        assert!(Sid::text(&format!("S-1-5{}", "-1".repeat(16))).is_err());
    }
}
