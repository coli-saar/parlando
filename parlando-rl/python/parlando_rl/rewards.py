"""Server-side reward calculation over accepted transitions and completion."""

from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

from parlando_agent_sdk import PlayerRole


@dataclass(frozen=True)
class RewardContext:
    """One accepted action or a terminal result without an accompanying action.

    Before and after map seat A and seat B to their respective views. No
    private frame is available when a session ends before its first action.
    """

    before: Mapping[PlayerRole, dict[str, Any]] | None
    after: Mapping[PlayerRole, dict[str, Any]] | None
    actor: PlayerRole | None
    action: dict[str, Any] | None
    completion: dict[str, Any] | None
    learner_role: PlayerRole


# A custom callable can run concurrently for independent served environments.
RewardFunction = Callable[[RewardContext], float]


def completion_reward(context: RewardContext) -> float:
    """Use the game's numeric reward, or one for a true success or correct flag."""
    completion = context.completion or {}
    reward = completion.get("reward")
    if type(reward) in (int, float):
        return float(reward)
    return float(completion.get("success", completion.get("correct")) is True)
