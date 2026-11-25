"""Framework-neutral decisions and role-private frames for a Parlando episode."""

from typing import Any

from parlando_agent_sdk import Observation as AgentObservation, Reason
from pydantic import BaseModel, Field


class ParlandoAction(BaseModel):
    """A game action, a message, both, or an empty decision to yield."""

    action: dict[str, Any] | None = None
    message: str | None = None


class ParlandoObservation(BaseModel):
    """The role-private view and all callbacks delivered since the last decision."""

    initial_view: dict[str, Any] | None = None
    events: list[AgentObservation]
    view: dict[str, Any]
    reason: Reason | None
    done: bool = False
    reward: float | None = None
    metadata: dict[str, Any] = Field(default_factory=dict)
