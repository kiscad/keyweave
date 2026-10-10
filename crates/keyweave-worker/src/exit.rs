//! Static exit classification only: no configuration, input or process metadata.
/// Stable, deliberately small exit codes. No error text enters binary stdio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkerExit {
    Clean = 0,
    Arguments = 64,
    Admission = 65,
    Config = 66,
    Install = 67,
    Loop = 68,
    Cleanup = 69,
    UnsupportedPlatform = 78,
}
/// Interpretation of an observed code, not authenticated cause or recovery proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitDiagnostic {
    pub raw_code: u32,
    pub code: &'static str,
    pub message: &'static str,
    pub remediation: &'static str,
}
impl WorkerExit {
    /// Match the full native code; never truncate an exception to its low byte.
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Clean),
            64 => Some(Self::Arguments),
            65 => Some(Self::Admission),
            66 => Some(Self::Config),
            67 => Some(Self::Install),
            68 => Some(Self::Loop),
            69 => Some(Self::Cleanup),
            78 => Some(Self::UnsupportedPlatform),
            _ => None,
        }
    }
}
impl ExitDiagnostic {
    /// Caller must obtain `raw_code` from the actual owned process exit.
    /// A chosen developer image can return any code; this is not image attestation.
    pub const fn observed(raw_code: u32) -> Self {
        let (code, message, remediation) = match WorkerExit::from_code(raw_code) {
            Some(WorkerExit::Clean) => (
                "KW-WORKER-000",
                "worker reported clean termination",
                "Check the independent supervisor and resource results; this code alone does not prove global key recovery.",
            ),
            Some(WorkerExit::Arguments) => (
                "KW-WORKER-064",
                "worker rejected its private argument contract",
                "Use a matching packaged host and worker; do not invoke the private worker entry directly.",
            ),
            Some(WorkerExit::Admission) => (
                "KW-WORKER-065",
                "worker rejected parent, Job, pipe or configuration-handle admission",
                "Check matching host/worker versions and owned startup resources; do not bypass admission or retry resume.",
            ),
            Some(WorkerExit::Config) => (
                "KW-WORKER-066",
                "worker rejected configuration loading or lossless compilation",
                "Run check-windows on the intended configuration and inspect its diagnostics; verify the sealed configuration handoff.",
            ),
            Some(WorkerExit::Install) => (
                "KW-WORKER-067",
                "worker session installation failed",
                "Check native output support, engine limits and interactive-session access; this code does not identify the failed API.",
            ),
            Some(WorkerExit::Loop) => (
                "KW-WORKER-068",
                "worker loop or progress protocol failed",
                "Inspect the independent supervisor reason; do not treat loop failure as proof of output recovery or automatically restart.",
            ),
            Some(WorkerExit::Cleanup) => (
                "KW-WORKER-069",
                "worker cleanup or safe stopped state was not confirmed",
                "Inspect independent shutdown and session-access results; do not automatically resume or claim held keys were restored.",
            ),
            Some(WorkerExit::UnsupportedPlatform) => (
                "KW-WORKER-078",
                "worker entry is unavailable on this platform",
                "Use a supported worker entry; Linux production bootstrap is still a separate implementation task.",
            ),
            None => (
                "KW-WORKER-UNKNOWN",
                "worker exit does not match the known scalar contract",
                "Preserve the full native exit code and supervisor reason; investigate forced termination, exception or image mismatch without inferring recovery.",
            ),
        };
        Self {
            raw_code,
            code,
            message,
            remediation,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_contract_has_unique_diagnostics_and_exact_native_values() {
        let variants = [
            WorkerExit::Clean,
            WorkerExit::Arguments,
            WorkerExit::Admission,
            WorkerExit::Config,
            WorkerExit::Install,
            WorkerExit::Loop,
            WorkerExit::Cleanup,
            WorkerExit::UnsupportedPlatform,
        ];
        let mut codes = std::collections::BTreeSet::new();
        for variant in variants {
            let raw = variant as u32;
            assert_eq!(WorkerExit::from_code(raw), Some(variant));
            let d = ExitDiagnostic::observed(raw);
            assert_eq!(d.raw_code, raw);
            assert!(codes.insert(d.code));
            assert!(!d.message.is_empty() && !d.remediation.is_empty());
        }
    }
    #[test]
    fn unknown_codes_are_not_truncated_or_mistaken_for_still_active() {
        for raw in [1, 63, 70, 77, 79, 259, 256 + 66, 3763033857, u32::MAX] {
            assert_eq!(WorkerExit::from_code(raw), None);
            let d = ExitDiagnostic::observed(raw);
            assert_eq!(d.raw_code, raw);
            assert_eq!(d.code, "KW-WORKER-UNKNOWN");
        }
    }
    #[test]
    fn diagnostics_are_static_metadata_and_never_claim_recovery() {
        assert!(
            ExitDiagnostic::observed(0)
                .remediation
                .contains("does not prove")
        );
        assert!(
            ExitDiagnostic::observed(69)
                .message
                .contains("not confirmed")
        );
        assert!(
            ExitDiagnostic::observed(67)
                .remediation
                .contains("does not identify")
        );
        assert!(
            ExitDiagnostic::observed(66)
                .remediation
                .contains("check-windows")
        );
    }
}
