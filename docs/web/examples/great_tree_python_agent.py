"""A minimal Python agent that can play either role in The Great Tree."""

from __future__ import annotations

from typing import Any

from parlando_agent_sdk import Agent, Context, Response, serve


class GreatTreePythonAgent(Agent):
    """Chooses simple legal actions from its role-specific Great Tree observation."""

    def __init__(self, context: Context) -> None:
        """Retains session context and starts without a pending action."""
        self.context = context
        self.pending_action: dict[str, Any] | None = None

    async def start(self, observation: dict[str, Any]) -> None:
        """Receives the initial observation and prepares the first action."""
        self.context.logger.log(f"Python agent started as seat {self.context.role}")
        self.pending_action = self._choose_action(observation)

    async def observe_transition(
        self,
        actor: str,
        action: dict[str, Any],
        observation: dict[str, Any],
    ) -> None:
        """Updates the pending action after every accepted task transition."""
        self.context.logger.log(f"Observed {actor}: {action}")
        self.pending_action = self._choose_action(observation)

    async def respond(
        self, available_actions: list[dict[str, Any]] | None
    ) -> Response | None:
        """Returns one prepared action, or yields until the task changes again."""
        del available_actions  # Great Tree does not enumerate its action space.
        if self.pending_action is None:
            return None
        action = self.pending_action
        self.pending_action = None
        return Response.action(action)

    def _choose_action(self, observation: dict[str, Any]) -> dict[str, Any] | None:
        """Selects a small deterministic policy for Crown or Root."""
        role = observation.get("role")
        if role == "crown":
            limbs = observation.get("limbs", [])
            if sum(bool(limb.get("sun")) for limb in limbs) >= 3:
                return None
            for limb in limbs:
                if not limb.get("sun"):
                    return {"type": "setSun", "limb": limb["id"], "lit": True}
        elif role == "root":
            for root in observation.get("roots", []):
                if root.get("thawed") and not root.get("running"):
                    return {"type": "setFlow", "root": root["id"], "open": True}
        return None


def create_agent(context: Context) -> Agent:
    """Creates one independent Python policy instance for a Parlando session."""
    return GreatTreePythonAgent(context)


if __name__ == "__main__":
    try:
        serve(create_agent, host="127.0.0.1", port=50051)
    except KeyboardInterrupt:
        pass
