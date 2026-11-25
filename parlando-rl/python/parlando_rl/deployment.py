"""Run a decision function as an ordinary Parlando SDK agent."""

from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from inspect import isawaitable
from typing import Any

from parlando_agent_sdk import Agent, Decision, Observation, Reason

from .models import ParlandoAction


@dataclass(frozen=True)
class PolicyInput:
    """Role-private inputs the SDK delivers before one live decision.

    Events are ordered and belong to the interval since the previous decision.
    The first decision also carries the view received when the session started.
    """

    view: dict[str, Any]
    reason: Reason
    events: tuple[Observation, ...]
    initial_view: dict[str, Any] | None


class PolicyAgent(Agent):
    """Adapt a trained decision function to the live Parlando agent protocol.

    Construct an instance in your SDK agent factory for each session. Bind any
    model state, input encoding, or action decoding through the supplied
    callable. The callable can be synchronous or asynchronous and returns a
    Parlando action or message in the game's native format.
    """

    def __init__(
        self, decide: Callable[[PolicyInput], ParlandoAction | Awaitable[ParlandoAction]]
    ) -> None:
        """Bind one session's decision function and initialize its view and event buffer."""
        self._decide = decide
        self._view: dict[str, Any] | None = None
        self._initial_view: dict[str, Any] | None = None
        self._events: list[Observation] = []

    async def start(self, observation: dict[str, Any]) -> None:
        """Record the initial view for the first decision and subsequent message-only events."""
        self._view = self._initial_view = observation
        self._events.clear()

    async def observe(self, observation: Observation) -> None:
        """Keep the latest private view and buffer each transition or message in order."""
        if observation.transition is not None:
            self._view = observation.transition.view
        self._events.append(observation)

    async def decide(self, reason: Reason) -> Decision:
        """Pass the current view and newly delivered events to the learned decision function."""
        if self._view is None:
            raise RuntimeError("start the policy agent before requesting a decision")
        inputs = PolicyInput(self._view, reason, tuple(self._events), self._initial_view)
        self._events.clear()
        self._initial_view = None
        result = self._decide(inputs)
        if isawaitable(result):
            result = await result
        return Decision(action=result.action, message=result.message)
