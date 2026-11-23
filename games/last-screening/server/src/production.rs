//! The production is an ordered score. Each reel sets its scenery, holds a set of
//! optical conditions continuously, and recognizes successive pictures on screen.

use super::{Slide, State};
use std::cmp::Ordering;

/// The position of a particular projected transparency relative to the scenery.
#[derive(Clone, Copy)]
enum Picture {
    InWindow(Slide),
    AtTower(Slide, Ordering),
}

impl Picture {
    /// Reads actual stage geometry. An image inside the aperture must also be visible.
    fn matches(self, state: &State) -> bool {
        match self {
            Self::InWindow(slide) => state.visible(slide) == Some(state.flats.window),
            Self::AtTower(slide, order) => state
                .image(slide)
                .is_some_and(|x| x.cmp(&state.flats.tower) == order),
        }
    }
}

/// A visible cue name and the picture the players must make in that phase.
struct Frame {
    phase: &'static str,
    picture: Picture,
}

/// Every reel needs the two flats on their cue marks. Further conditions in
/// `hold` must persist while its nonempty list of successive frames is played.
pub(super) struct Reel {
    pub(super) id: &'static str,
    hold: &'static [Picture],
    frames: &'static [Frame],
}

impl Reel {
    /// Names the active cue for the participant UI.
    pub(super) fn phase(&self, frame: usize) -> &'static str {
        self.frames[frame].phase
    }

    /// Returns the next expected frame or completion. Remaining on the last accepted
    /// picture holds progress. A broken hold or skipped frame restarts the sequence.
    pub(super) fn advance(&self, state: &State, expected: usize) -> Option<usize> {
        if state.flats != state.cue() || !self.hold.iter().all(|p| p.matches(state)) {
            return Some(0);
        }
        if self.frames[expected].picture.matches(state) {
            let next = expected + 1;
            return (next < self.frames.len()).then_some(next);
        }
        if expected > 0 && self.frames[expected - 1].picture.matches(state) {
            return Some(expected);
        }
        Some(usize::from(self.frames[0].picture.matches(state)))
    }
}

use Ordering::{Equal, Greater, Less};
use Picture::{AtTower, InWindow};
use Slide::{Light, Moon, Ship};

/// Reorder or add reels here. Layouts are drawn for each entry before the show starts.
pub(super) const PRODUCTION: &[Reel] = &[
    Reel {
        id: "moonrise",
        hold: &[],
        frames: &[Frame {
            phase: "align",
            picture: InWindow(Moon),
        }],
    },
    Reel {
        id: "crossing",
        hold: &[InWindow(Light)],
        frames: &[
            Frame {
                phase: "align",
                picture: AtTower(Ship, Less),
            },
            Frame {
                phase: "hide",
                picture: AtTower(Ship, Equal),
            },
            Frame {
                phase: "emerge",
                picture: AtTower(Ship, Greater),
            },
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A replacement reel hides and reveals the moon from the opposite direction while
    /// the ship is framed in the window. The same score reader handles all three pictures.
    #[test]
    fn reverse_lunar_transit_uses_existing_optics() {
        let reel = Reel {
            id: "eclipse",
            hold: &[InWindow(Ship)],
            frames: &[
                Frame {
                    phase: "rise",
                    picture: AtTower(Moon, Greater),
                },
                Frame {
                    phase: "eclipse",
                    picture: AtTower(Moon, Equal),
                },
                Frame {
                    phase: "set",
                    picture: AtTower(Moon, Less),
                },
            ],
        };
        let mut s = State::deal(22);
        s.flats = s.cue();
        s.lanterns[0].slide = Ship;
        s.lanterns[0].aim = s.flats.window;
        s.lanterns[1].slide = Moon;
        let mut cursor = Some(0);
        for aim in [6, s.flats.tower, 0] {
            s.lanterns[1].aim = aim;
            cursor = reel.advance(&s, cursor.unwrap());
        }
        assert_eq!(cursor, None);
    }
}
