"""Exercise Beacon Relay mechanics directly, without a component host."""

import unittest

from game import Beacon, Config, Outcome, Pulse, View
from parlando_game_sdk import Rejection, Role


class BeaconTest(unittest.TestCase):
    """Check seed-derived targets, rejection atomicity, and replay."""

    def test_targets_depend_on_seed(self) -> None:
        """The same seed always chooses the same target and initial role."""
        for seed, target in ((0, 4), (1, 5), (2, 6), (3, 4), (2**64 - 1, 4)):
            with self.subTest(seed=seed):
                game = Beacon.init(Config(), seed)
                self.assertEqual(game.view_a(), View(0, target, True))
                self.assertEqual(game.view_b(), View(0, target, False))

    def test_rejected_actions_leave_state_unchanged_and_replay_completes(self) -> None:
        """Replay of accepted pulses produces the live views and outcome."""
        game = Beacon.init(Config(), 0)
        with self.assertRaisesRegex(Rejection, "^wrong_turn$"):
            game.step(Role.B, Pulse(2))
        with self.assertRaisesRegex(Rejection, "^invalid_pulse$"):
            game.step(Role.A, Pulse(0))
        self.assertEqual(game.view_a(), View(0, 4, True))

        game.step(Role.A, Pulse(2))
        game.step(Role.B, Pulse(1))
        with self.assertRaisesRegex(Rejection, "^overcharge$"):
            game.step(Role.A, Pulse(2))
        self.assertIsNone(game.completion())

        replay = Beacon.init(Config(), 0)
        replay.step(Role.A, Pulse(2))
        replay.step(Role.B, Pulse(1))
        self.assertEqual(game.view_a(), replay.view_a())
        self.assertEqual(game.view_b(), replay.view_b())

        game.step(Role.A, Pulse(1))
        replay.step(Role.A, Pulse(1))
        self.assertEqual(game.completion(), Outcome(activated_at=4))
        self.assertEqual(game.completion(), replay.completion())
        self.assertEqual(game.view_a(), View(4, 4, False))
        self.assertEqual(game.view_b(), View(4, 4, False))
        with self.assertRaisesRegex(Rejection, "^already_activated$"):
            game.step(Role.B, Pulse(1))
