//! Beacon Relay is a cooperative game for two players who alternate charging a beacon.

use parlando_game_sdk::{export, Game, GameValue, Rejection, Role};

/// The starter uses a seed-derived target and needs no experiment settings.
#[derive(GameValue)]
pub struct Config {}

/// A charge pulse supplied by the active player.
#[derive(GameValue)]
pub struct Pulse {
    ticks: u8,
}

/// Each player sees the shared charge and whether they can act.
#[derive(Debug, PartialEq, GameValue)]
pub struct View {
    charge: u8,
    target: u8,
    your_turn: bool,
}

/// Records how much charge the team reached when the beacon activated.
#[derive(Debug, PartialEq, GameValue)]
pub struct Outcome {
    activated_at: u8,
}

/// The session's charge, deterministic target and active player.
pub struct Beacon {
    charge: u8,
    target: u8,
    turn: Role,
}

impl Beacon {
    /// Projects the same charge to both roles with role-specific action permission.
    fn view(&self, role: Role) -> View {
        View {
            charge: self.charge,
            target: self.target,
            your_turn: self.charge < self.target && self.turn == role,
        }
    }
}

impl Game for Beacon {
    type Config = Config;
    type Action = Pulse;
    type ViewA<'a> = View;
    type ViewB<'a> = View;
    type Completion<'a> = Outcome;
    type Metadata<'a> = ();

    /// Sets a target of four, five, or six ticks using the recorded seed.
    fn init(_: Config, seed: u64) -> Result<Self, String> {
        Ok(Self {
            charge: 0,
            target: 4 + (seed % 3) as u8,
            turn: Role::A,
        })
    }

    /// Accepts a one- or two-tick pulse if it comes from the active player and fits the beacon.
    fn step(&mut self, actor: Role, pulse: Pulse) -> Result<(), Rejection> {
        if self.charge == self.target {
            return Err(Rejection::new("already_activated"));
        }
        if actor != self.turn {
            return Err(Rejection::new("wrong_turn"));
        }
        if !(1..=2).contains(&pulse.ticks) {
            return Err(Rejection::new("invalid_pulse"));
        }
        if pulse.ticks > self.target - self.charge {
            return Err(Rejection::new("overcharge"));
        }
        self.charge += pulse.ticks;
        self.turn = match actor {
            Role::A => Role::B,
            Role::B => Role::A,
        };
        Ok(())
    }

    /// Projects the observation for participant A.
    fn view_a(&self) -> View {
        self.view(Role::A)
    }

    /// Projects the observation for participant B.
    fn view_b(&self) -> View {
        self.view(Role::B)
    }

    /// Publishes the terminal charge only when the exact target was reached.
    fn completion(&self) -> Option<Outcome> {
        (self.charge == self.target).then_some(Outcome {
            activated_at: self.charge,
        })
    }
}

// The host replays accepted actions from the seed to recover this small game.
export!(Beacon);

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks rejected actions leave charge and turn intact and replay can continue.
    #[test]
    fn rejection_preserves_state_and_replay_reaches_completion() {
        let mut game = Beacon::init(Config {}, 0).unwrap();
        assert_eq!(
            game.step(Role::B, Pulse { ticks: 2 }).unwrap_err().code(),
            "wrong_turn"
        );
        assert_eq!(
            game.view_a(),
            View {
                charge: 0,
                target: 4,
                your_turn: true
            }
        );
        game.step(Role::A, Pulse { ticks: 2 }).unwrap();
        game.step(Role::B, Pulse { ticks: 1 }).unwrap();
        assert_eq!(
            game.step(Role::A, Pulse { ticks: 2 }).unwrap_err().code(),
            "overcharge"
        );
        assert!(game.completion().is_none());

        let mut replay = Beacon::init(Config {}, 0).unwrap();
        replay.step(Role::A, Pulse { ticks: 2 }).unwrap();
        replay.step(Role::B, Pulse { ticks: 1 }).unwrap();
        assert_eq!(game.view_a(), replay.view_a());
        assert_eq!(game.view_b(), replay.view_b());
        game.step(Role::A, Pulse { ticks: 1 }).unwrap();
        replay.step(Role::A, Pulse { ticks: 1 }).unwrap();
        assert_eq!(game.completion(), Some(Outcome { activated_at: 4 }));
        assert_eq!(game.completion(), replay.completion());
    }
}
