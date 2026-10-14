//! Passive pointer payloads travel through the same causal queues as key actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerMotion {
    Wheel { horizontal: i32, vertical: i32 },
    Move { x: i32, y: i32 },
    Activity { moved: bool, scrolled: bool },
}

impl PointerMotion {
    pub fn cancels_alone(&self) -> bool {
        match self {
            Self::Wheel {
                horizontal,
                vertical,
            } => *horizontal != 0 || *vertical != 0,
            Self::Move { .. } => false,
            Self::Activity { scrolled, .. } => *scrolled,
        }
    }
}
