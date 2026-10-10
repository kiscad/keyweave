//! Bounded paired-clock evidence; no signal/event/physical suspend proof.
const MAX_SAMPLE_NS: u64 = 10_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sample {
    pub before: u64,
    pub boot: u64,
    pub after: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    Revoked,
    Backwards,
    WideSample,
    OffsetChanged,
}
#[derive(Debug, Default)]
pub(crate) struct Continuity {
    interval: Option<(i128, i128)>,
    last: Option<(u64, u64)>,
    fault: Option<Fault>,
}
impl Continuity {
    fn reject<T>(&mut self, fault: Fault) -> Result<T, Fault> {
        self.fault = Some(fault);
        Err(fault)
    }
    pub(crate) fn revoke(&mut self) {
        self.fault.get_or_insert(Fault::Revoked);
    }
    pub(crate) fn observe(&mut self, s: Sample) -> Result<u64, Fault> {
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        let Some(span) = s.after.checked_sub(s.before) else {
            return self.reject(Fault::Backwards);
        };
        if self
            .last
            .is_some_and(|(mono, boot)| s.before < mono || s.boot < boot)
        {
            return self.reject(Fault::Backwards);
        }
        if span > MAX_SAMPLE_NS {
            return self.reject(Fault::WideSample);
        }
        // Signed offsets allow independently offset Linux time namespaces.
        // True simultaneous BOOTTIME-MONOTONIC lies inside this bracket.
        let mut lower = i128::from(s.boot) - i128::from(s.after);
        let mut upper = i128::from(s.boot) - i128::from(s.before);
        if let Some((old_lower, old_upper)) = self.interval {
            lower = lower.max(old_lower);
            upper = upper.min(old_upper);
        }
        if lower > upper {
            return self.reject(Fault::OffsetChanged);
        }
        self.interval = Some((lower, upper));
        self.last = Some((s.after, s.boot));
        Ok(s.after)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_offset_survives_different_sample_order_latency_and_narrows_evidence() {
        let mut c = Continuity::default();
        assert_eq!(
            c.observe(Sample {
                before: 100,
                boot: 151,
                after: 103
            }),
            Ok(103)
        );
        assert_eq!(c.interval, Some((48, 51)));
        assert_eq!(
            c.observe(Sample {
                before: 200,
                boot: 252,
                after: 202
            }),
            Ok(202)
        );
        assert_eq!(c.interval, Some((50, 51)));
        assert_eq!(
            c.observe(Sample {
                before: 300,
                boot: 350,
                after: 300
            }),
            Ok(300)
        );
        assert_eq!(c.interval, Some((50, 50)));
    }
    #[test]
    fn offset_change_revokes_even_when_monotonic_ttl_has_not_elapsed() {
        let mut c = Continuity::default();
        c.observe(Sample {
            before: 100,
            boot: 150,
            after: 101,
        })
        .ok();
        assert_eq!(
            c.observe(Sample {
                before: 102,
                boot: 1_000_152,
                after: 103
            }),
            Err(Fault::OffsetChanged)
        );
        assert_eq!(
            c.observe(Sample {
                before: 104,
                boot: 154,
                after: 105
            }),
            Err(Fault::OffsetChanged)
        );
        assert_eq!(c.last, Some((101, 150)));
    }
    #[test]
    fn negative_namespace_offsets_and_equal_times_are_valid() {
        let mut c = Continuity::default();
        let sample = Sample {
            before: 200,
            boot: 10,
            after: 200,
        };
        assert_eq!(c.observe(sample), Ok(200));
        assert_eq!(c.observe(sample), Ok(200));
        assert_eq!(c.interval, Some((-190, -190)));
    }
    #[test]
    fn backwards_either_clock_or_bracket_and_overwide_sample_permanently_revoke() {
        for sample in [
            Sample {
                before: 99,
                boot: 151,
                after: 100,
            },
            Sample {
                before: 102,
                boot: 149,
                after: 103,
            },
            Sample {
                before: 102,
                boot: 153,
                after: 101,
            },
        ] {
            let mut c = Continuity::default();
            c.observe(Sample {
                before: 100,
                boot: 150,
                after: 101,
            })
            .ok();
            assert_eq!(c.observe(sample), Err(Fault::Backwards));
        }
        let mut c = Continuity::default();
        assert_eq!(
            c.observe(Sample {
                before: 0,
                boot: 0,
                after: MAX_SAMPLE_NS
            }),
            Ok(MAX_SAMPLE_NS)
        );
        let mut c = Continuity::default();
        assert_eq!(
            c.observe(Sample {
                before: 0,
                boot: 0,
                after: MAX_SAMPLE_NS + 1
            }),
            Err(Fault::WideSample)
        );
        assert_eq!(
            c.observe(Sample {
                before: 0,
                boot: 0,
                after: 0
            }),
            Err(Fault::WideSample)
        );
    }
    #[test]
    fn overlapping_intervals_are_ambiguous_not_a_claim_of_no_suspend() {
        let mut c = Continuity::default();
        c.observe(Sample {
            before: 0,
            boot: 5,
            after: 10,
        })
        .ok();
        assert_eq!(
            c.observe(Sample {
                before: 20,
                boot: 29,
                after: 30
            }),
            Ok(30)
        );
        assert_eq!(c.interval, Some((-1, 5))); // offset change could fit this bracket
    }
    #[test]
    fn external_syscall_failure_revocation_cannot_be_cleared() {
        let mut c = Continuity::default();
        c.revoke();
        assert_eq!(
            c.observe(Sample {
                before: 1,
                boot: 1,
                after: 1
            }),
            Err(Fault::Revoked)
        );
    }
}
