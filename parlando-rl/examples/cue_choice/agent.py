"""Serve an SDK agent using the cue-choice table learned by train.py."""

import argparse
import json
from pathlib import Path

from parlando_agent_sdk import Agent, Context, Decision, Observation, Reason, serve


class CueAgent(Agent):
    """Choose by cue when player A has a legal choice."""

    def __init__(self, context: Context) -> None:
        """Load the configured table or table.json on the serving machine."""
        table = Path(context.settings.get("table", "table.json"))
        self._table = json.loads(table.read_text(encoding="utf-8"))
        self._view = {}

    async def start(self, view: dict) -> None:
        """Store the learner's initial role-private view."""
        self._view = view

    async def observe(self, observation: Observation) -> None:
        """Update the view from transitions and keep it across message deliveries."""
        if observation.transition is not None:
            self._view = observation.transition.view

    async def decide(self, reason: Reason) -> Decision:
        """Return the learned choice if it is legal, or yield before the deal."""
        actions = self._view.get("available_actions", [])
        if not actions:
            return Decision()
        choice = self._table.get(self._view["cue"])
        action = next((action for action in actions if action["choice"] == choice), actions[0])
        return Decision(action=action)


def main() -> None:
    """Expose the checkpoint-backed agent on a loopback gRPC endpoint."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=50061)
    args = parser.parse_args()
    serve(lambda context: CueAgent(context), port=args.port)


if __name__ == "__main__":
    main()
