"""Implement the Last Token mechanics world with one state per Wasm instance."""

import json

from componentize_py_types import Err
from wit_world import exports
from wit_world.exports.mechanics import Frame, Role

_VIEW = {
    "type": "struct",
    "value": [
        {"name": "remaining", "schema": {"type": "u64"}},
        {"name": "your_turn", "schema": {"type": "bool"}},
    ],
}
_DESCRIPTION = json.dumps({
    "config": {"type": "struct", "value": []},
    "action": {
        "type": "enum",
        "value": {
            "representation": {"type": "internal", "value": {"tag": "type"}},
            "variants": [
                {"name": name, "payload": {"type": "unit"}}
                for name in ("take_one", "take_two", "take_three")
            ],
        },
    },
    "view_a": _VIEW,
    "view_b": _VIEW,
    "completion": {
        "type": "struct",
        "value": [{"name": "winner", "schema": {"type": "string"}}],
    },
    "metadata": {"type": "null"},
})

# Each Wasm instance has its own Python module globals.
_remaining = 0
_turn = Role.A


def _frame() -> Frame:
    """Encode both bounded MessagePack views and the winner on the final move."""
    view_a = bytes((0x92, _remaining, 0xC3 if _remaining and _turn == Role.A else 0xC2))
    view_b = bytes((0x92, _remaining, 0xC3 if _remaining and _turn == Role.B else 0xC2))
    completion = None
    if _remaining == 0:
        completion = b"\x91\xa1A" if _turn == Role.A else b"\x91\xa1B"
    return Frame(checkpoint=None, view_a=view_a, view_b=view_b,
                 completion=completion, metadata=None)


class Mechanics(exports.Mechanics):
    """Adapt Parlando's compact payloads to the Python game state."""

    def describe(self) -> str:
        """Return the schema used for both seats and all sessions."""
        return _DESCRIPTION

    def init(self, config: bytes, seed: int) -> Frame:
        """Accept the empty configuration and derive the pile from the recorded seed."""
        global _remaining, _turn
        if config != b"\x90":
            raise Err("invalid_config")
        _remaining = 12 + seed % 4
        _turn = Role.A
        return _frame()

    def restore(self, checkpoint: bytes) -> None:
        """Reject checkpoint input because recovery uses deterministic replay."""
        raise Err("unsupported_checkpoint")

    def step(self, actor: Role, action: bytes) -> Frame:
        """Validate before changing state and retain the final actor as the winner."""
        global _remaining, _turn
        # The host encodes the three unit variants as one-byte positive fixints.
        if len(action) != 1 or action[0] > 2:
            raise Err("invalid_action")
        if _remaining == 0:
            raise Err("game_over")
        if actor != _turn:
            raise Err("wrong_turn")
        amount = action[0] + 1
        if amount > _remaining:
            raise Err("too_many_tokens")

        _remaining -= amount
        if _remaining:
            _turn = Role.B if actor == Role.A else Role.A
        return _frame()
