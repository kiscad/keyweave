//! Cold host-only run identity. No PID/time/counter/device/file fallback.
#![allow(unsafe_code)]
use std::io;
const MAX_DRAWS: usize = 4;
pub struct RunIdentity {
    run_id: u64,
}
impl std::fmt::Debug for RunIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunIdentity").finish_non_exhaustive()
    }
}
impl RunIdentity {
    /// Generate once per new worker transaction, outside active IO loops.
    /// NONBLOCK fails if entropy is unavailable; any RNG error stops immediately.
    pub fn generate() -> io::Result<Self> {
        generate_with(draw)
    }
    pub fn run_id(&self) -> u64 {
        self.run_id
    }
}
fn validate_count(count: libc::ssize_t) -> io::Result<()> {
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    if count != 8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "incomplete run identity entropy",
        ));
    }
    Ok(())
}
fn draw() -> io::Result<[u8; 8]> {
    let mut bytes = [0u8; 8];
    // SAFETY: initialized, aligned writable array of exactly 8 live bytes.
    // getrandom is synchronous, retains no pointer; audited unsigned flags=1
    // select NONBLOCK urandom source. Check signed byte count before decoding.
    let count =
        unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), libc::GRND_NONBLOCK) };
    validate_count(count)?;
    Ok(bytes)
}
fn generate_with(mut draw: impl FnMut() -> io::Result<[u8; 8]>) -> io::Result<RunIdentity> {
    for _ in 0..MAX_DRAWS {
        let run_id = u64::from_le_bytes(draw()?);
        if run_id != 0 {
            return Ok(RunIdentity { run_id });
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "run identity entropy retry budget exhausted",
    ))
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn decode_is_explicit_little_endian_and_zero_causes_complete_redraw() {
        let mut calls = 0;
        let id = generate_with(|| {
            calls += 1;
            Ok(if calls == 1 {
                [0; 8]
            } else {
                [1, 2, 3, 4, 5, 6, 7, 8]
            })
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(id.run_id(), 0x0807060504030201);
    }
    #[test]
    fn zero_budget_and_system_error_never_fall_back() {
        let mut calls = 0;
        assert!(
            generate_with(|| {
                calls += 1;
                Ok([0; 8])
            })
            .is_err()
        );
        assert_eq!(calls, MAX_DRAWS);
        let mut calls = 0;
        assert!(
            matches!(generate_with(|| {calls+=1;Err(io::Error::from_raw_os_error(libc::EAGAIN))}),Err(error) if error.raw_os_error()==Some(libc::EAGAIN))
        );
        assert_eq!(calls, 1);
    }
    #[test]
    fn short_or_oversized_entropy_count_is_rejected() {
        for count in 0..=9 {
            assert_eq!(validate_count(count).is_ok(), count == 8);
        }
    }
    #[test]
    fn actual_kernel_rng_returns_nonzero_without_debug_disclosure() {
        let id = RunIdentity::generate().unwrap();
        assert!(id.run_id() != 0);
        assert_eq!(format!("{id:?}"), "RunIdentity { .. }");
        // No probabilistic collision assertion or printing random bytes.
    }
}
