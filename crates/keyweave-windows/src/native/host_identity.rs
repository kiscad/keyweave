//! Cold per-launch identifiers. No input, process, files or background work.
use crate::keys::InjectionTag;
const ATTEMPTS: usize = 4;
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("identity generation is forbidden in keyboard callback")]
    InCallback,
    #[error("BCryptGenRandom failed: {0}")]
    Random(i32),
    #[error("random source returned zero identifiers in every bounded attempt")]
    Exhausted,
}
pub struct RunIdentity {
    run: u64,
    tag: InjectionTag,
}
impl std::fmt::Debug for RunIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunIdentity").finish_non_exhaustive()
    }
}
impl RunIdentity {
    pub fn generate() -> Result<Self, IdentityError> {
        if !super::outside_callback() {
            return Err(IdentityError::InCallback);
        }
        for _ in 0..ATTEMPTS {
            let mut bytes = [0u8; 12];
            // SAFETY: synchronous BCrypt call writes exactly the live 12-byte
            // buffer; NULL provider with SYSTEM_PREFERRED_RNG opens no handle.
            let status =
                unsafe { api::BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 12, 2) };
            if status != 0 {
                return Err(IdentityError::Random(status));
            }
            let run = u64::from_le_bytes(
                bytes[..8]
                    .try_into()
                    .map_err(|_| IdentityError::Exhausted)?,
            );
            let marker = u32::from_le_bytes(
                bytes[8..]
                    .try_into()
                    .map_err(|_| IdentityError::Exhausted)?,
            );
            if run == 0 || marker == 0 {
                continue;
            }
            let tag = InjectionTag::new(marker as usize).map_err(|_| IdentityError::Exhausted)?;
            return Ok(Self { run, tag });
        }
        Err(IdentityError::Exhausted)
    }
    pub fn run_id(&self) -> u64 {
        self.run
    }
    pub fn tag(&self) -> InjectionTag {
        self.tag
    }
}
#[cfg(not(test))]
mod api {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        pub(super) fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            size: u32,
            flags: u32,
        ) -> i32;
    }
}
#[cfg(test)]
#[allow(non_snake_case)]
mod api {
    use std::{cell::RefCell, collections::VecDeque};
    #[derive(Default)]
    pub(super) struct Mock {
        pub calls: usize,
        pub replies: VecDeque<(i32, [u8; 12])>,
    }
    thread_local! {pub(super) static MOCK:RefCell<Mock> = RefCell::new(Mock::default());}
    pub(super) unsafe fn BCryptGenRandom(
        algorithm: *mut std::ffi::c_void,
        buffer: *mut u8,
        size: u32,
        flags: u32,
    ) -> i32 {
        assert!(algorithm.is_null());
        assert_eq!(size, 12);
        assert_eq!(flags, 2);
        MOCK.with(|m| {
            let mut m = m.borrow_mut();
            m.calls += 1;
            let (status, bytes) = m.replies.pop_front().unwrap_or((0, [0; 12]));
            if status == 0 {
                // SAFETY: production caller supplies a live 12-byte buffer.
                unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, 12) };
            }
            status
        })
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use api::MOCK;
    fn bytes(run: u64, tag: u32) -> [u8; 12] {
        let mut b = [0; 12];
        b[..8].copy_from_slice(&run.to_le_bytes());
        b[8..].copy_from_slice(&tag.to_le_bytes());
        b
    }
    #[test]
    fn exact_decode_full_width_and_debug_redaction() {
        MOCK.with(|m| {
            m.borrow_mut()
                .replies
                .push_back((0, bytes(u64::MAX, u32::MAX)))
        });
        let identity = RunIdentity::generate().unwrap();
        assert_eq!(identity.run_id(), u64::MAX);
        assert_eq!(identity.tag().value(), u32::MAX as usize);
        assert_eq!(format!("{identity:?}"), "RunIdentity { .. }");
        MOCK.with(|m| assert_eq!(m.borrow().calls, 1));
    }
    #[test]
    fn zeros_retry_entire_pair_then_fresh_sample() {
        MOCK.with(|m| {
            m.borrow_mut()
                .replies
                .extend([(0, bytes(0, 3)), (0, bytes(4, 0)), (0, bytes(5, 6))])
        });
        let identity = RunIdentity::generate().unwrap();
        assert_eq!((identity.run_id(), identity.tag().value()), (5, 6));
        MOCK.with(|m| assert_eq!(m.borrow().calls, 3));
    }
    #[test]
    fn bounded_zero_exhaustion_has_no_fallback() {
        assert_eq!(
            RunIdentity::generate().unwrap_err(),
            IdentityError::Exhausted
        );
        MOCK.with(|m| assert_eq!(m.borrow().calls, ATTEMPTS));
    }
    #[test]
    fn last_attempt_can_succeed_and_subsequent_call_draws_new_pair() {
        MOCK.with(|m| {
            let mut m = m.borrow_mut();
            m.replies.extend([(0, [0; 12]); ATTEMPTS - 1]);
            m.replies.extend([(0, bytes(11, 12)), (0, bytes(13, 14))]);
        });
        let first = RunIdentity::generate().unwrap();
        let second = RunIdentity::generate().unwrap();
        assert_eq!((first.run_id(), first.tag().value()), (11, 12));
        assert_eq!((second.run_id(), second.tag().value()), (13, 14));
        MOCK.with(|m| assert_eq!(m.borrow().calls, ATTEMPTS + 1));
    }
    #[test]
    fn rng_status_aborts_without_reading_failed_bytes_or_retrying() {
        for prefix in [false, true] {
            MOCK.with(|m| {
                *m.borrow_mut() = api::Mock::default();
                if prefix {
                    m.borrow_mut().replies.push_back((0, bytes(0, 7)));
                }
                m.borrow_mut().replies.push_back((-1073741823, bytes(8, 9)));
            });
            assert_eq!(
                RunIdentity::generate().unwrap_err(),
                IdentityError::Random(-1073741823)
            );
            MOCK.with(|m| assert_eq!(m.borrow().calls, 1 + usize::from(prefix)));
        }
    }
    #[test]
    fn callback_rejection_is_zero_native_work() {
        super::super::hook_thread::with_callback_borrow(|| {
            assert_eq!(
                RunIdentity::generate().unwrap_err(),
                IdentityError::InCallback
            )
        });
        MOCK.with(|m| assert_eq!(m.borrow().calls, 0));
    }
}
