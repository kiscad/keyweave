//! Mouse low-level Hook semantics only; no native registration or IO.
use crate::keys::{InjectionTag, Origin};
use keyweave_types::{BUTTON_PAGE, CaptureDecision, KeyEdge, KeyId};
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseHookRecord {
    pub message: u32,
    pub x: i32,
    pub y: i32,
    pub data: u32,
    pub flags: u32,
    pub extra_info: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseInputEvent {
    /// Screen coordinates, never raw relative motion.
    Position {
        x: i32,
        y: i32,
    },
    Wheel {
        horizontal: bool,
        delta: i32,
    },
    Button {
        key: KeyId,
        edge: KeyEdge,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MouseFault {
    #[error("invalid mouse hook record")]
    Record,
    #[error("combined XBUTTON1/XBUTTON2 requires multi-button capture semantics")]
    CombinedButtons,
    #[error("mouse handler failed")]
    Handler,
    #[error("mouse handler panicked")]
    Panic,
    #[error("screen-coordinate movement cannot be captured by this dispatcher")]
    MovementCapture,
}
impl MouseHookRecord {
    pub fn decode(self, tag: InjectionTag) -> Result<(Origin, MouseInputEvent), MouseFault> {
        if self.flags & !3 != 0 || self.flags & 2 != 0 && self.flags & 1 == 0 {
            return Err(MouseFault::Record);
        }
        let origin = if self.flags & 1 == 0 {
            Origin::Physical
        } else if self.extra_info == tag.value() {
            Origin::OwnInjected
        } else {
            Origin::OtherInjected
        };
        let input = match self.message {
            0x200 => MouseInputEvent::Position {
                x: self.x,
                y: self.y,
            },
            0x20a | 0x20e => MouseInputEvent::Wheel {
                horizontal: self.message == 0x20e,
                delta: i32::from(i16::from_ne_bytes(((self.data >> 16) as u16).to_ne_bytes())),
            },
            0x201 | 0x202 | 0x204 | 0x205 | 0x207 | 0x208 | 0x20b | 0x20c => {
                let (usage, edge) = match self.message {
                    0x201 => (1, KeyEdge::Down),
                    0x202 => (1, KeyEdge::Up),
                    0x204 => (2, KeyEdge::Down),
                    0x205 => (2, KeyEdge::Up),
                    0x207 => (3, KeyEdge::Down),
                    0x208 => (3, KeyEdge::Up),
                    _ => {
                        let button = self.data >> 16;
                        if button == 3 {
                            return Err(MouseFault::CombinedButtons);
                        }
                        if !(1..=2).contains(&button) {
                            return Err(MouseFault::Record);
                        }
                        (
                            (button + 3) as u16,
                            if self.message == 0x20b {
                                KeyEdge::Down
                            } else {
                                KeyEdge::Up
                            },
                        )
                    }
                };
                MouseInputEvent::Button {
                    key: KeyId::new(BUTTON_PAGE, usage),
                    edge,
                }
            }
            _ => return Err(MouseFault::Record),
        };
        Ok((origin, input))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseHookEvent {
    pub input: MouseInputEvent,
    pub binding: Option<CaptureDecision>,
}
pub trait MouseHandler {
    /// Synchronous bounded callback logic; Position must return Pass.
    fn handle_mouse(&mut self, event: MouseHookEvent) -> Result<CaptureDecision, MouseFault>;
}
impl<T: MouseHandler + ?Sized> MouseHandler for Box<T> {
    fn handle_mouse(&mut self, event: MouseHookEvent) -> Result<CaptureDecision, MouseFault> {
        (**self).handle_mouse(event)
    }
}
pub struct MouseDispatcher<H: MouseHandler> {
    handler: H,
    tag: InjectionTag,
    slots: [Option<CaptureDecision>; 5],
    fault: Option<MouseFault>,
}
impl<H: MouseHandler> MouseDispatcher<H> {
    pub fn new(handler: H, tag: InjectionTag) -> Self {
        Self {
            handler,
            tag,
            slots: [None; 5],
            fault: None,
        }
    }
    pub fn fault(&self) -> Option<MouseFault> {
        self.fault
    }
    pub fn held_buttons(&self) -> usize {
        self.slots.iter().flatten().count()
    }
    pub fn held_captured(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|d| **d == CaptureDecision::Capture)
            .count()
    }
    pub fn stop(&mut self, fault: MouseFault) {
        self.fault.get_or_insert(fault);
    }
    pub fn into_handler(self) -> H {
        self.handler
    }
    pub fn dispatch(&mut self, n_code: i32, record: Option<MouseHookRecord>) -> CaptureDecision {
        if n_code != 0 || self.fault.is_some() {
            return CaptureDecision::Pass;
        }
        let Some(record) = record else {
            self.stop(MouseFault::Record);
            return CaptureDecision::Pass;
        };
        let (origin, mut input) = match record.decode(self.tag) {
            Ok(v) => v,
            Err(e) => {
                self.stop(e);
                return CaptureDecision::Pass;
            }
        };
        if origin != Origin::Physical {
            return CaptureDecision::Pass;
        }
        let mut slot = None;
        let mut binding = None;
        let mut up = false;
        if let MouseInputEvent::Button { key, edge } = input {
            let index = usize::from(key.usage - 1);
            slot = Some(index);
            binding = self.slots[index];
            up = edge == KeyEdge::Up;
            if up && binding.is_none() {
                return CaptureDecision::Pass;
            }
            if edge == KeyEdge::Down && binding.is_some() {
                input = MouseInputEvent::Button {
                    key,
                    edge: KeyEdge::Repeat,
                };
            }
        }
        let decision = match catch_unwind(AssertUnwindSafe(|| {
            self.handler.handle_mouse(MouseHookEvent { input, binding })
        })) {
            Ok(Ok(d)) => binding.unwrap_or(d),
            Ok(Err(e)) => {
                self.stop(e);
                return CaptureDecision::Pass;
            }
            Err(payload) => {
                std::mem::forget(payload);
                self.stop(MouseFault::Panic);
                return CaptureDecision::Pass;
            }
        };
        if matches!(input, MouseInputEvent::Position { .. }) && decision == CaptureDecision::Capture
        {
            self.stop(MouseFault::MovementCapture);
            return CaptureDecision::Pass;
        }
        if let Some(index) = slot {
            self.slots[index] = if up { None } else { Some(decision) };
        }
        decision
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn tag() -> InjectionTag {
        InjectionTag::new(23).unwrap()
    }
    fn record(message: u32, data: u32) -> MouseHookRecord {
        MouseHookRecord {
            message,
            x: i32::MIN,
            y: i32::MAX,
            data,
            flags: 0,
            extra_info: 23,
        }
    }
    #[test]
    fn physical_button_table_and_reserved_data_decode() {
        for (down, up, usage, data) in [
            (0x201, 0x202, 1, u32::MAX),
            (0x204, 0x205, 2, u32::MAX),
            (0x207, 0x208, 3, u32::MAX),
            (0x20b, 0x20c, 4, 0x1ffff),
            (0x20b, 0x20c, 5, 0x2abcd),
        ] {
            for (msg, edge) in [(down, KeyEdge::Down), (up, KeyEdge::Up)] {
                assert_eq!(
                    record(msg, data).decode(tag()).unwrap(),
                    (
                        Origin::Physical,
                        MouseInputEvent::Button {
                            key: KeyId::new(BUTTON_PAGE, usage),
                            edge
                        }
                    )
                );
            }
        }
        assert_eq!(
            record(0x20b, 0x30000).decode(tag()),
            Err(MouseFault::CombinedButtons)
        );
    }
    #[test]
    fn wheel_high_word_sign_and_screen_coordinates_preserved() {
        for horizontal in [false, true] {
            for delta in [i16::MIN, -120, -1, 0, 1, 120, i16::MAX] {
                let data = (u32::from(u16::from_ne_bytes(delta.to_ne_bytes())) << 16) | 0xabcd;
                assert_eq!(
                    record(if horizontal { 0x20e } else { 0x20a }, data)
                        .decode(tag())
                        .unwrap()
                        .1,
                    MouseInputEvent::Wheel {
                        horizontal,
                        delta: i32::from(delta)
                    }
                );
            }
        }
        assert_eq!(
            record(0x200, u32::MAX).decode(tag()).unwrap().1,
            MouseInputEvent::Position {
                x: i32::MIN,
                y: i32::MAX
            }
        );
    }
    #[test]
    fn injection_marker_requires_flags_and_malformed_records_reject() {
        for (flags, extra_info, origin) in [
            (0, 23, Origin::Physical),
            (1, 23, Origin::OwnInjected),
            (3, 23, Origin::OwnInjected),
            (1, 24, Origin::OtherInjected),
            (3, 0, Origin::OtherInjected),
        ] {
            assert_eq!(
                MouseHookRecord {
                    flags,
                    extra_info,
                    ..record(0x201, 0)
                }
                .decode(tag())
                .unwrap()
                .0,
                origin
            );
        }
        for invalid in [
            MouseHookRecord {
                flags: 2,
                ..record(0x201, 0)
            },
            MouseHookRecord {
                flags: 4,
                ..record(0x201, 0)
            },
            record(0x203, 0),
            record(0x20b, 0),
            record(0x20c, 4 << 16),
        ] {
            assert_eq!(invalid.decode(tag()), Err(MouseFault::Record));
        }
    }
    struct Handler {
        events: Vec<MouseHookEvent>,
        decision: CaptureDecision,
        error: bool,
        panic: bool,
    }
    impl MouseHandler for Handler {
        fn handle_mouse(&mut self, event: MouseHookEvent) -> Result<CaptureDecision, MouseFault> {
            self.events.push(event);
            assert!(!self.panic, "fixture panic");
            if self.error {
                Err(MouseFault::Handler)
            } else {
                Ok(self.decision)
            }
        }
    }
    fn dispatcher() -> MouseDispatcher<Handler> {
        MouseDispatcher::new(
            Handler {
                events: Vec::new(),
                decision: CaptureDecision::Capture,
                error: false,
                panic: false,
            },
            tag(),
        )
    }
    #[test]
    fn saved_decision_survives_duplicate_down_up_and_orphan_up() {
        let mut d = dispatcher();
        assert_eq!(d.dispatch(0, Some(record(0x202, 0))), CaptureDecision::Pass);
        assert!(d.handler.events.is_empty());
        assert_eq!(
            d.dispatch(0, Some(record(0x201, 0))),
            CaptureDecision::Capture
        );
        assert_eq!(d.held_captured(), 1);
        d.handler.decision = CaptureDecision::Pass;
        assert_eq!(
            d.dispatch(0, Some(record(0x201, 0))),
            CaptureDecision::Capture
        );
        assert_eq!(d.handler.events[1].binding, Some(CaptureDecision::Capture));
        assert!(matches!(
            d.handler.events[1].input,
            MouseInputEvent::Button {
                edge: KeyEdge::Repeat,
                ..
            }
        ));
        assert_eq!(
            d.dispatch(0, Some(record(0x202, 0))),
            CaptureDecision::Capture
        );
        assert_eq!(d.held_buttons(), 0);
        assert_eq!(d.dispatch(0, Some(record(0x204, 0))), CaptureDecision::Pass);
        d.handler.decision = CaptureDecision::Capture;
        assert_eq!(d.dispatch(0, Some(record(0x205, 0))), CaptureDecision::Pass);
        assert_eq!(d.held_buttons(), 0);
    }
    #[test]
    fn injected_and_non_action_bypass_handler_and_slots() {
        let mut d = dispatcher();
        for flags in [1, 3] {
            for extra_info in [23, 24] {
                assert_eq!(
                    d.dispatch(
                        0,
                        Some(MouseHookRecord {
                            flags,
                            extra_info,
                            ..record(0x201, 0)
                        })
                    ),
                    CaptureDecision::Pass
                );
            }
        }
        assert_eq!(d.dispatch(-1, None), CaptureDecision::Pass);
        assert_eq!(d.dispatch(1, None), CaptureDecision::Pass);
        assert_eq!(d.fault(), None);
        assert!(d.handler.events.is_empty());
        assert_eq!(d.held_buttons(), 0);
        assert_eq!(d.dispatch(0, None), CaptureDecision::Pass);
        assert_eq!(d.fault(), Some(MouseFault::Record));
    }
    #[test]
    fn faults_preserve_held_slots_and_position_capture_is_rejected() {
        for panic in [false, true] {
            let mut d = dispatcher();
            d.dispatch(0, Some(record(0x201, 0)));
            d.handler.error = !panic;
            d.handler.panic = panic;
            assert_eq!(d.dispatch(0, Some(record(0x202, 0))), CaptureDecision::Pass);
            assert_eq!(d.held_captured(), 1);
            assert_eq!(
                d.fault(),
                Some(if panic {
                    MouseFault::Panic
                } else {
                    MouseFault::Handler
                })
            );
            let calls = d.handler.events.len();
            d.dispatch(0, Some(record(0x202, 0)));
            assert_eq!(d.handler.events.len(), calls);
        }
        let mut d = dispatcher();
        d.handler.decision = CaptureDecision::Pass;
        assert_eq!(d.dispatch(0, Some(record(0x200, 0))), CaptureDecision::Pass);
        assert_eq!(d.fault(), None);
        d.handler.decision = CaptureDecision::Capture;
        assert_eq!(d.dispatch(0, Some(record(0x200, 0))), CaptureDecision::Pass);
        assert_eq!(d.fault(), Some(MouseFault::MovementCapture));
    }
}
