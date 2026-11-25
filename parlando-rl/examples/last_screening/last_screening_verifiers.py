"""Verifiers environment and SDK policies for The Last Screening."""

import os
from pathlib import Path

from openai import AsyncOpenAI
from parlando_agent_sdk import Agent, Context, Decision, Observation, Reason, serve
from parlando_rl import RewardContext
from parlando_rl.verifiers import (
    ChatMessages,
    ParlandoModelAgent,
    ParlandoTaskset,
    ParlandoVerifiersEnv,
)


__all__ = ["LastScreeningEnv", "ParlandoTaskset"]


class LastScreeningEnv(ParlandoVerifiersEnv):
    """Translate the game's public escape outcome into a cooperative reward."""

    def reward(self, context: RewardContext) -> float:
        """Award one when both players escape."""
        return float((context.completion or {}).get("escaped") is True)


class Stagehand(Agent):
    """Scripted stagehand for checking the dialogue rollout without a model."""

    def __init__(self) -> None:
        """Start with no cue exchange."""
        self.view = {}
        self.tower = None
        self.announce = False

    async def start(self, view: dict) -> None:
        """Store the first backstage view."""
        self._update_view(view)

    def _update_view(self, view: dict) -> None:
        """Begin a fresh cue exchange whenever the reel changes."""
        if view["level"] != self.view.get("level"):
            self.tower = None
            self.announce = True
        self.view = view

    async def observe(self, observation: Observation) -> None:
        """Apply role-visible transitions and read the projectionist's cue."""
        if observation.transition is not None:
            self._update_view(observation.transition.view)
        message = observation.message
        if (
            message is not None
            and message.sender == "A"
            and message.text.startswith("tower ")
        ):
            self.tower = int(message.text.split()[1].rstrip(".")) - 1

    async def decide(self, reason: Reason) -> Decision:
        """Announce the window, set the flats, and take the exit when it opens."""
        window = int(self.view["cue"]["window"])
        message = f"window {window + 1}" if self.announce else None
        self.announce = False
        marks = {"window": window, "tower": self.tower}
        for action in self.view["actions"]:
            if action["type"] == "exit" or (
                action["type"] == "moveFlat"
                and action["position"] == marks[action["flat"]]
            ):
                return Decision(action=action, message=message)
        return Decision(message=message)


class CheckpointStagehand(ParlandoModelAgent):
    """Serve a checkpoint using the same decision frames and instructions as training."""

    def __init__(self, context: Context) -> None:
        """Use the requested checkpoint's model name at an OpenAI-compatible endpoint."""
        model = context.checkpoint_id or os.environ["PARLANDO_MODEL"]
        self.client = AsyncOpenAI()

        async def complete(messages: ChatMessages) -> str:
            """Query one model turn and return its action envelope."""
            response = await self.client.chat.completions.create(
                model=model, messages=messages
            )
            return response.choices[0].message.content or ""

        instructions = Path(__file__).with_name("instructions.txt").read_text().strip()
        super().__init__(complete, instructions=instructions)

    async def shutdown(self) -> None:
        """Release the session's model endpoint connection."""
        await self.client.close()


def checkpoint_stagehand(context: Context) -> CheckpointStagehand:
    """Construct the checkpoint agent with its SDK session context."""
    return CheckpointStagehand(context)


if __name__ == "__main__":
    serve(checkpoint_stagehand)
