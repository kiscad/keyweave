//! Explicit relative mouse output. No capture or automatic injection.
use crate::{
    keys::{InjectionTag, MAX_BATCH},
    pump::{Insertion, SubmitError},
};
use keyweave_types::{BUTTON_PAGE, KeyEdge, KeyId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Motion { dx: i32, dy: i32 },
    Wheel { horizontal: bool, delta: i32 },
    Button { key: KeyId, edge: KeyEdge },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PointerError {
    #[error("Windows mouse output supports button1..5 Down/Up only")]
    Button,
    #[error("mouse output batch exceeds capacity")]
    Capacity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MousePacket {
    pub(crate) dx: i32,
    pub(crate) dy: i32,
    pub(crate) data: u32,
    pub(crate) flags: u32,
    pub(crate) extra_info: usize,
}
impl MousePacket {
    pub fn motion(self) -> (i32, i32) {
        (self.dx, self.dy)
    }
    pub fn data(self) -> u32 {
        self.data
    }
    pub fn flags(self) -> u32 {
        self.flags
    }
    pub fn tag(self) -> usize {
        self.extra_info
    }
}
pub fn encode(action: MouseAction, tag: InjectionTag) -> Result<MousePacket, PointerError> {
    let mut packet = MousePacket {
        dx: 0,
        dy: 0,
        data: 0,
        flags: 0,
        extra_info: tag.value(),
    };
    match action {
        MouseAction::Motion { dx, dy } => {
            packet.dx = dx;
            packet.dy = dy;
            packet.flags = 1;
        }
        MouseAction::Wheel { horizontal, delta } => {
            packet.flags = if horizontal { 0x1000 } else { 0x800 };
            packet.data = u32::from_ne_bytes(delta.to_ne_bytes());
        }
        MouseAction::Button { key, edge } => {
            if key.page != BUTTON_PAGE
                || !(1..=5).contains(&key.usage)
                || !matches!(edge, KeyEdge::Down | KeyEdge::Up)
            {
                return Err(PointerError::Button);
            }
            let up = edge == KeyEdge::Up;
            packet.flags = match key.usage {
                1 => {
                    if up {
                        4
                    } else {
                        2
                    }
                }
                2 => {
                    if up {
                        0x10
                    } else {
                        8
                    }
                }
                3 => {
                    if up {
                        0x40
                    } else {
                        0x20
                    }
                }
                _ => {
                    if up {
                        0x100
                    } else {
                        0x80
                    }
                }
            };
            if key.usage >= 4 {
                packet.data = u32::from(key.usage - 3);
            }
        }
    }
    Ok(packet)
}
pub fn encode_batch(
    actions: &[MouseAction],
    tag: InjectionTag,
) -> Result<Vec<MousePacket>, PointerError> {
    if actions.len() > MAX_BATCH {
        return Err(PointerError::Capacity);
    }
    actions.iter().map(|action| encode(*action, tag)).collect()
}
pub trait SendPointerBackend {
    /// Rejected only before native call; zero acceptance remains an Insertion.
    /// Partial motion/scroll must never be replayed. Count is not app delivery.
    fn submit_pointer(&mut self, packets: &[MousePacket]) -> Result<Insertion, SubmitError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn tag() -> InjectionTag {
        InjectionTag::new(17).unwrap()
    }
    #[test]
    fn relative_motion_and_wheel_preserve_extremes_and_order() {
        let actions = [
            MouseAction::Motion {
                dx: i32::MIN,
                dy: i32::MAX,
            },
            MouseAction::Wheel {
                horizontal: true,
                delta: -1,
            },
            MouseAction::Wheel {
                horizontal: false,
                delta: i32::MIN,
            },
            MouseAction::Motion { dx: 0, dy: 0 },
        ];
        let packets = encode_batch(&actions, tag()).unwrap();
        assert_eq!(packets[0].motion(), (i32::MIN, i32::MAX));
        assert_eq!(
            packets.iter().map(|p| p.flags()).collect::<Vec<_>>(),
            [1, 0x1000, 0x800, 1]
        );
        assert_eq!(packets[1].data(), u32::MAX);
        assert_eq!(packets[2].data(), 0x80000000);
        assert!(packets.iter().all(|p| p.tag() == 17));
    }
    #[test]
    fn five_button_edges_have_distinct_flags_and_xbutton_data() {
        for (usage, down, up, data) in [
            (1, 2, 4, 0),
            (2, 8, 16, 0),
            (3, 32, 64, 0),
            (4, 128, 256, 1),
            (5, 128, 256, 2),
        ] {
            for (edge, flags) in [(KeyEdge::Down, down), (KeyEdge::Up, up)] {
                let p = encode(
                    MouseAction::Button {
                        key: KeyId::new(BUTTON_PAGE, usage),
                        edge,
                    },
                    tag(),
                )
                .unwrap();
                assert_eq!((p.flags(), p.data(), p.motion()), (flags, data, (0, 0)));
            }
        }
    }
    #[test]
    fn unsupported_buttons_repeat_and_batch_overflow_reject() {
        for (key, edge) in [
            (KeyId::new(BUTTON_PAGE, 0), KeyEdge::Down),
            (KeyId::new(BUTTON_PAGE, 6), KeyEdge::Up),
            (KeyId::new(7, 4), KeyEdge::Down),
            (KeyId::new(BUTTON_PAGE, 1), KeyEdge::Repeat),
        ] {
            assert_eq!(
                encode(MouseAction::Button { key, edge }, tag()),
                Err(PointerError::Button)
            );
        }
        let action = MouseAction::Motion { dx: 1, dy: 2 };
        assert_eq!(
            encode_batch(&[action; MAX_BATCH + 1], tag()),
            Err(PointerError::Capacity)
        );
        assert_eq!(
            encode_batch(&[action; MAX_BATCH], tag()).unwrap().len(),
            MAX_BATCH
        );
        assert!(encode_batch(&[], tag()).unwrap().is_empty());
        assert!(InjectionTag::new(0).is_err());
    }
}
