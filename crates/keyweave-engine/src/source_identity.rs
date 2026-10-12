//! Identity domains of one owned physical stage controller, not native authorization.
use keyweave_types::SourceToken;
pub(crate) const ORIGIN_DOMAIN_START: u64 = 1 << 63;
/// The returned successor may equal the domain boundary, representing exhaustion.
pub(crate) fn prepare_origin(counter: u64) -> Option<(SourceToken, u64)> {
    (counter != 0 && counter < ORIGIN_DOMAIN_START).then_some((
        SourceToken(ORIGIN_DOMAIN_START | counter),
        counter.checked_add(1)?,
    ))
}
