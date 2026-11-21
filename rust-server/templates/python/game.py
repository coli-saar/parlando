"""Beacon Relay is a cooperative game for two players who charge a beacon."""

from dataclasses import dataclass

from parlando_game_sdk import Game, Rejection, Role, U8, mechanics


@dataclass(frozen=True)
class Config:
    """The starter derives its target from the seed and needs no settings."""


@dataclass(frozen=True)
class Pulse:
    """The active player requests a charge pulse of one or two ticks."""

    ticks: U8


@dataclass(frozen=True)
class View:
    """Both players see the shared charge and their own turn permission."""

    charge: U8
    target: U8
    your_turn: bool


@dataclass(frozen=True)
class Outcome:
    """The exact charge at which the team activated the beacon."""

    activated_at: U8


@dataclass
class Beacon(Game[Config, Pulse, View, View, Outcome, None]):
    """Owns a session's charge, deterministic target, and active role."""

    charge: int
    target: int
    turn: Role

    @classmethod
    def init(cls, config: Config, seed: int) -> "Beacon":
        """Derive a target of four, five, or six ticks from the session seed."""
        return cls(charge=0, target=4 + seed % 3, turn=Role.A)

    def step(self, actor: Role, action: Pulse) -> None:
        """Apply a legal pulse and advance the turn, or reject without mutation."""
        if self.charge == self.target:
            raise Rejection("already_activated")
        if actor != self.turn:
            raise Rejection("wrong_turn")
        if action.ticks not in (1, 2):
            raise Rejection("invalid_pulse")
        if action.ticks > self.target - self.charge:
            raise Rejection("overcharge")
        self.charge += action.ticks
        self.turn = Role.B if actor == Role.A else Role.A

    def _view(self, role: Role) -> View:
        """Project the current charge and whether this role can act."""
        return View(
            charge=self.charge,
            target=self.target,
            your_turn=self.charge < self.target and self.turn == role,
        )

    def view_a(self) -> View:
        """Project the observation for participant A."""
        return self._view(Role.A)

    def view_b(self) -> View:
        """Project the observation for participant B."""
        return self._view(Role.B)

    def completion(self) -> Outcome | None:
        """Publish the terminal charge when the team reaches the exact target."""
        if self.charge == self.target:
            return Outcome(activated_at=self.charge)
        return None


# The host recovers this small game by replaying accepted actions from the seed.
Mechanics = mechanics(Beacon)
