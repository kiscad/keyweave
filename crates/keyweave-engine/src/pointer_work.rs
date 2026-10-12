//! Passive pointer payloads travel through the same causal queues as key actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerMotion {
    Wheel { horizontal: i32, vertical: i32 },
    Move { x: i32, y: i32 },
}
