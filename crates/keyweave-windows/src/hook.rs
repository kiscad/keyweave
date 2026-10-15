//! Synchronous capture policy; no IO or clock. OS callbacks own the thread.
use crate::keys::{self, HookRecord, IdentityPolicy, InjectionTag, Origin, ResolvedKey};
use keyweave_types::{CaptureDecision, KeyEdge};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookFault {
    Mouse(crate::mouse_hook::MouseFault),
    Record,
    Handler,
    Panic,
    Reentrant,
    Emergency,
    UntrackedInput,
    SessionChanged,
    AccessLease(crate::access::AccessError),
    InputAccess(crate::startup::ProbeError),
    EmergencyDetector(crate::emergency::EmergencyError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookEvent {
    pub identity: ResolvedKey,
    pub edge: KeyEdge,
    /// None only for first down; repeat/up must preserve this decision.
    pub binding: Option<CaptureDecision>,
}
/// Bounded synchronous logic only: no blocking IO/IPC/config parsing. Error
/// terminates capture; caller must explicitly reconcile output after unhook.
pub trait HookHandler {
    fn handle(&mut self, event: HookEvent) -> Result<CaptureDecision, HookFault>;
}
impl<T: HookHandler + ?Sized> HookHandler for Box<T> {
    fn handle(&mut self, event: HookEvent) -> Result<CaptureDecision, HookFault> {
        (**self).handle(event)
    }
}
#[derive(Clone, Copy)]
struct Binding {
    identity: Option<ResolvedKey>,
    decision: CaptureDecision,
}
pub struct HookDispatcher<H: HookHandler> {
    handler: H,
    slots: [Option<Binding>; 512],
    tag: InjectionTag,
    policy: IdentityPolicy,
    fault: Option<HookFault>,
}
impl<H: HookHandler> HookDispatcher<H> {
    pub fn new(handler: H, tag: InjectionTag, policy: IdentityPolicy) -> Self {
        Self {
            handler,
            slots: [None; 512],
            tag,
            policy,
            fault: None,
        }
    }
    pub fn fault(&self) -> Option<HookFault> {
        self.fault
    }
    /// Includes Pass and unsupported identities; startup needs all bindings.
    pub fn held_keys(&self) -> usize {
        self.slots.iter().flatten().count()
    }
    pub fn held_captured(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|slot| slot.decision == CaptureDecision::Capture)
            .count()
    }
    /// Freezes evidence; does not claim neutral or emit any output releases.
    pub fn stop(&mut self, fault: HookFault) {
        self.fault.get_or_insert(fault);
    }
    pub fn into_handler(self) -> H {
        self.handler
    }
    pub fn dispatch(&mut self, n_code: i32, record: Option<HookRecord>) -> CaptureDecision {
        if n_code != 0 || self.fault.is_some() {
            return CaptureDecision::Pass;
        }
        let Some(record) = record else {
            self.stop(HookFault::Record);
            return CaptureDecision::Pass;
        };
        if record.origin(self.tag) != Origin::Physical {
            return CaptureDecision::Pass;
        }
        let (Ok(edge), Ok(scan)) = (record.validate(), record.scan()) else {
            self.stop(HookFault::Record);
            return CaptureDecision::Pass;
        };
        let slot = scan.slot();
        let previous = self.slots[slot];
        if previous.is_none() && edge == KeyEdge::Up {
            return CaptureDecision::Pass;
        }
        let (identity, binding, actual_edge) = if let Some(previous) = previous {
            (
                previous.identity,
                Some(previous.decision),
                if edge == KeyEdge::Down {
                    KeyEdge::Repeat
                } else {
                    edge
                },
            )
        } else {
            (keys::resolve(record, self.policy).ok(), None, edge)
        };
        let decision = if let Some(identity) = identity {
            let result = catch_unwind(AssertUnwindSafe(|| {
                self.handler.handle(HookEvent {
                    identity,
                    edge: actual_edge,
                    binding,
                })
            }));
            match result {
                Ok(Ok(decision)) => binding.unwrap_or(decision),
                Ok(Err(fault)) => {
                    self.stop(fault);
                    return CaptureDecision::Pass;
                }
                Err(payload) => {
                    // A user-supplied panic payload may panic in Drop. Do not let
                    // that second unwind cross the callback boundary. One leak
                    // per permanently stopped dispatcher, never per normal key.
                    std::mem::forget(payload);
                    self.stop(HookFault::Panic);
                    return CaptureDecision::Pass;
                }
            }
        } else {
            CaptureDecision::Pass
        };
        self.slots[slot] = if edge == KeyEdge::Up {
            None
        } else {
            Some(Binding { identity, decision })
        };
        decision
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    struct Handler {
        events: Vec<HookEvent>,
        next: Result<CaptureDecision, HookFault>,
        panic: bool,
    }
    impl HookHandler for Handler {
        fn handle(&mut self, event: HookEvent) -> Result<CaptureDecision, HookFault> {
            self.events.push(event);
            assert!(!self.panic, "handler failure");
            self.next
        }
    }
    fn dispatcher() -> HookDispatcher<Handler> {
        HookDispatcher::new(
            Handler {
                events: vec![],
                next: Ok(CaptureDecision::Capture),
                panic: false,
            },
            InjectionTag::new(1).unwrap(),
            IdentityPolicy::CanonicalScan,
        )
    }
    fn record(scan: u32, up: bool) -> HookRecord {
        HookRecord {
            message: if up { 0x101 } else { 0x100 },
            vk_code: 0x41,
            scan_code: scan,
            flags: if up { 0x80 } else { 0 },
            extra_info: 0,
        }
    }
    #[test]
    fn capture_binding_is_preserved_through_repeat_and_up_policy_changes() {
        let mut dispatcher = dispatcher();
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, false))),
            CaptureDecision::Capture
        );
        dispatcher.handler.next = Ok(CaptureDecision::Pass);
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, false))),
            CaptureDecision::Capture
        );
        let mut up = record(0x1e, true);
        up.vk_code = 0x51;
        assert_eq!(dispatcher.dispatch(0, Some(up)), CaptureDecision::Capture);
        assert_eq!(dispatcher.held_captured(), 0);
        assert_eq!(dispatcher.handler.events[1].edge, KeyEdge::Repeat);
        assert_eq!(
            dispatcher.handler.events[2].binding,
            Some(CaptureDecision::Capture)
        );
        assert_eq!(dispatcher.dispatch(0, Some(up)), CaptureDecision::Pass);
        assert_eq!(dispatcher.handler.events.len(), 3);
    }
    #[test]
    fn pass_binding_does_not_become_capture_on_reload() {
        let mut dispatcher = dispatcher();
        dispatcher.handler.next = Ok(CaptureDecision::Pass);
        dispatcher.dispatch(0, Some(record(0x1e, false)));
        dispatcher.handler.next = Ok(CaptureDecision::Capture);
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, true))),
            CaptureDecision::Pass
        );
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, false))),
            CaptureDecision::Capture
        );
    }
    #[test]
    fn unsupported_down_keeps_pass_binding_despite_vk_changes() {
        let mut dispatcher = dispatcher();
        let mut down = record(0x1e, false);
        down.vk_code = 0x13;
        assert_eq!(dispatcher.dispatch(0, Some(down)), CaptureDecision::Pass);
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, false))),
            CaptureDecision::Pass
        );
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, true))),
            CaptureDecision::Pass
        );
        assert!(dispatcher.handler.events.is_empty());
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, false))),
            CaptureDecision::Capture
        );
    }
    #[test]
    fn injected_and_nonaction_records_never_reach_handler_or_mutate_binding() {
        let mut dispatcher = dispatcher();
        assert_eq!(dispatcher.dispatch(-1, None), CaptureDecision::Pass);
        assert_eq!(dispatcher.dispatch(1, None), CaptureDecision::Pass);
        dispatcher.dispatch(0, Some(record(0x1e, false)));
        for marker in [1, 2] {
            let mut injected = record(0x1e, true);
            injected.flags |= 0x10;
            injected.extra_info = marker;
            assert_eq!(
                dispatcher.dispatch(0, Some(injected)),
                CaptureDecision::Pass
            );
        }
        assert_eq!(dispatcher.held_captured(), 1);
        assert_eq!(dispatcher.handler.events.len(), 1);
        assert!(dispatcher.fault().is_none());
    }
    #[test]
    fn invalid_record_and_handler_fault_stop_capture_but_preserve_evidence() {
        for fault in [HookFault::Record, HookFault::Handler] {
            let mut dispatcher = dispatcher();
            dispatcher.dispatch(0, Some(record(0x1e, false)));
            let input = if fault == HookFault::Record {
                None
            } else {
                dispatcher.handler.next = Err(fault);
                Some(record(0x1e, true))
            };
            assert_eq!(dispatcher.dispatch(0, input), CaptureDecision::Pass);
            assert_eq!(dispatcher.fault(), Some(fault));
            assert_eq!(dispatcher.held_captured(), 1);
            assert_eq!(
                dispatcher.dispatch(0, Some(record(0x30, false))),
                CaptureDecision::Pass
            );
        }
    }
    #[test]
    fn handler_panic_is_contained_and_permanently_disables_capture() {
        let mut dispatcher = dispatcher();
        dispatcher.handler.panic = true;
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, false))),
            CaptureDecision::Pass
        );
        assert_eq!(dispatcher.fault(), Some(HookFault::Panic));
        assert_eq!(dispatcher.held_captured(), 0);
        dispatcher.handler.panic = false;
        assert_eq!(
            dispatcher.dispatch(0, Some(record(0x1e, false))),
            CaptureDecision::Pass
        );
        assert_eq!(dispatcher.into_handler().events.len(), 1);
    }
}
