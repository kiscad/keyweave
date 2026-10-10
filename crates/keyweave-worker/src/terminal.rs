//! Windows-only native exit encoding; pure metadata and bounded static issues.
use crate::{TerminalFacts, WorkerExit};
const TAG: u32 = 0x4b57_0000;
const MASK: u32 = 0x03ff;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum TerminalIssue {
    Fault = 1,
    ProgressFailed = 2,
    ShutdownMissing = 4,
    StoppedUnconfirmed = 8,
    UnhookUnconfirmed = 16,
    RecoveryUnconfirmed = 32,
    NotificationsUnconfirmed = 64,
    AccessLost = 128,
    SessionChanged = 256,
    UntrackedInput = 512,
}
const ISSUES: [TerminalIssue; 10] = [
    TerminalIssue::Fault,
    TerminalIssue::ProgressFailed,
    TerminalIssue::ShutdownMissing,
    TerminalIssue::StoppedUnconfirmed,
    TerminalIssue::UnhookUnconfirmed,
    TerminalIssue::RecoveryUnconfirmed,
    TerminalIssue::NotificationsUnconfirmed,
    TerminalIssue::AccessLost,
    TerminalIssue::SessionChanged,
    TerminalIssue::UntrackedInput,
];
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TerminalEvidence {
    bits: u16,
}
impl TerminalEvidence {
    /// Exact tagged nonzero mask only. Never interpret a low byte or reserved bit.
    pub const fn decode(raw: u32) -> Option<Self> {
        if raw & !MASK == TAG && raw & MASK != 0 {
            Some(Self {
                bits: (raw & MASK) as u16,
            })
        } else {
            None
        }
    }
    pub const fn raw(self) -> u32 {
        TAG | self.bits as u32
    }
    pub const fn contains(self, issue: TerminalIssue) -> bool {
        self.bits & issue as u16 != 0
    }
    pub fn issues(self) -> impl Iterator<Item = TerminalIssue> {
        ISSUES
            .into_iter()
            .filter(move |issue| self.contains(*issue))
    }
    /// Cleanup obligations dominate the recommendation, without dropping faults.
    pub const fn classification(self) -> WorkerExit {
        if self.bits & !3 != 0 {
            WorkerExit::Cleanup
        } else {
            WorkerExit::Loop
        }
    }
}
impl std::fmt::Debug for TerminalEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.issues()).finish()
    }
}
/// Closed set of legacy or tagged codes, always a nonnegative native i32.
/// Tagged values are for Windows only; Unix status would lose high bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeWorkerExit {
    raw: u32,
}
impl NativeWorkerExit {
    pub const fn raw(self) -> u32 {
        self.raw
    }
    pub const fn evidence(self) -> Option<TerminalEvidence> {
        TerminalEvidence::decode(self.raw)
    }
    /// Safe because all constructors emit small legacy codes or 0x4b57_03ff max.
    pub const fn native_i32(self) -> i32 {
        self.raw as i32
    }
}
impl From<WorkerExit> for NativeWorkerExit {
    fn from(value: WorkerExit) -> Self {
        Self { raw: value as u32 }
    }
}
impl TerminalFacts {
    pub fn native_exit(self) -> NativeWorkerExit {
        let flags = [
            self.fault,
            self.progress_failed,
            !self.shutdown_present,
            !self.stopped,
            !self.unhook_ok,
            !self.recovery_ok,
            !self.notifications_ok,
            self.access_lost,
            self.session_changed,
            self.untracked_input,
        ];
        let bits = ISSUES
            .into_iter()
            .zip(flags)
            .fold(
                0u16,
                |bits, (issue, present)| if present { bits | issue as u16 } else { bits },
            );
        NativeWorkerExit {
            raw: if bits == 0 { 0 } else { TAG | bits as u32 },
        }
    }
}
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    fn facts(bits: u16) -> TerminalFacts {
        let has = |issue| bits & issue as u16 != 0;
        TerminalFacts {
            fault: has(TerminalIssue::Fault),
            progress_failed: has(TerminalIssue::ProgressFailed),
            shutdown_present: !has(TerminalIssue::ShutdownMissing),
            stopped: !has(TerminalIssue::StoppedUnconfirmed),
            unhook_ok: !has(TerminalIssue::UnhookUnconfirmed),
            recovery_ok: !has(TerminalIssue::RecoveryUnconfirmed),
            notifications_ok: !has(TerminalIssue::NotificationsUnconfirmed),
            access_lost: has(TerminalIssue::AccessLost),
            session_changed: has(TerminalIssue::SessionChanged),
            untracked_input: has(TerminalIssue::UntrackedInput),
        }
    }
    #[test]
    fn all_combinations_preserve_each_fact_and_original_classification() {
        for bits in 0..=1023 {
            let f = facts(bits);
            let native = f.native_exit();
            assert!(native.native_i32() >= 0);
            assert_eq!(native.native_i32() as u32, native.raw());
            if bits == 0 {
                assert_eq!(native.raw(), 0);
                assert_eq!(native.evidence(), None);
                continue;
            }
            let evidence = native.evidence().unwrap();
            assert_eq!(evidence.raw(), native.raw());
            assert_eq!(evidence.classification(), f.exit());
            let diagnostic = crate::ExitDiagnostic::observed(native.raw());
            assert_eq!(diagnostic.terminal, Some(evidence));
            assert_eq!(
                diagnostic.code,
                if bits & !3 != 0 {
                    "KW-WORKER-069"
                } else {
                    "KW-WORKER-068"
                }
            );
            for issue in ISSUES {
                assert_eq!(evidence.contains(issue), bits & issue as u16 != 0);
            }
            assert_eq!(evidence.issues().count(), bits.count_ones() as usize);
        }
    }
    #[test]
    fn malformed_reserved_and_wrong_tag_never_decode() {
        for raw in [
            TAG,
            TAG | 1024,
            TAG | 32768,
            0,
            69,
            259,
            0x4b56_0001,
            0x4b58_0001,
            u32::MAX,
            3763033857,
        ] {
            assert_eq!(TerminalEvidence::decode(raw), None);
        }
    }
    #[test]
    fn cleanup_priority_does_not_erase_loop_or_concurrent_shutdown_facts() {
        let evidence = facts(1 | 2 | 32 | 128 | 256 | 512)
            .native_exit()
            .evidence()
            .unwrap();
        assert_eq!(evidence.classification(), WorkerExit::Cleanup);
        assert_eq!(
            format!("{evidence:?}"),
            "[Fault, ProgressFailed, RecoveryUnconfirmed, AccessLost, SessionChanged, UntrackedInput]"
        );
        for legacy in [
            WorkerExit::Clean,
            WorkerExit::Arguments,
            WorkerExit::Admission,
            WorkerExit::Config,
            WorkerExit::Install,
            WorkerExit::Loop,
            WorkerExit::Cleanup,
            WorkerExit::UnsupportedPlatform,
        ] {
            let exit = NativeWorkerExit::from(legacy);
            assert_eq!(exit.raw(), legacy as u32);
            assert_eq!(exit.evidence(), None);
        }
    }
}
