#![allow(clippy::unwrap_used)]
use super::*;
#[test]
fn ungranted_and_expiry_are_exact_and_late_success_cannot_revive() {
    let mut lease = AccessLease::new();
    assert_eq!(lease.check(MonoTime(0)), Err(AccessError::Ungranted));
    lease.renew_after_probe(MonoTime(10)).unwrap();
    assert_eq!(lease.next_refresh(), Some(MonoTime(10 + REFRESH_NS)));
    assert!(!lease.refresh_due(MonoTime(9 + REFRESH_NS)).unwrap());
    assert!(lease.refresh_due(MonoTime(10 + REFRESH_NS)).unwrap());
    assert!(lease.check(MonoTime(9 + LEASE_NS)).is_ok());
    assert_eq!(
        lease.check(MonoTime(10 + LEASE_NS)),
        Err(AccessError::Expired)
    );
    let before = lease.clone();
    assert_eq!(
        lease.renew_after_probe(MonoTime(10 + LEASE_NS)),
        Err(AccessError::Expired)
    );
    assert_eq!(lease, before);
}
#[test]
fn renewal_uses_completion_time_and_refuses_backwards_or_overflow_atomically() {
    let mut lease = AccessLease::new();
    lease.renew_after_probe(MonoTime(10)).unwrap();
    lease.renew_after_probe(MonoTime(20 + REFRESH_NS)).unwrap();
    assert_eq!(lease.next_refresh(), Some(MonoTime(20 + 2 * REFRESH_NS)));
    let before = lease.clone();
    assert_eq!(
        lease.renew_after_probe(MonoTime(19 + REFRESH_NS)),
        Err(AccessError::ClockBackwards)
    );
    assert_eq!(lease, before);
    let mut lease = AccessLease::new();
    let before = lease.clone();
    assert_eq!(
        lease.renew_after_probe(MonoTime(u64::MAX - REFRESH_NS)),
        Err(AccessError::TimeOverflow)
    );
    assert_eq!(lease, before);
    lease
        .renew_after_probe(MonoTime(u64::MAX - LEASE_NS))
        .unwrap();
    let before = lease.clone();
    assert_eq!(
        lease.renew_after_probe(MonoTime(u64::MAX - LEASE_NS + 1)),
        Err(AccessError::TimeOverflow)
    );
    assert_eq!(lease, before);
}
