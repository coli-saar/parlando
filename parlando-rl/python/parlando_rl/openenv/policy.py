"""Adapt SDK policy decisions to the OpenEnv action model."""

from parlando_rl.models import ParlandoObservation
from parlando_rl.policy import AgentPolicy as EpisodeAgentPolicy
from parlando_rl.policy import HeadlessWakeupError, SessionInfo

from .models import ParlandoAction

__all__ = ["AgentPolicy", "HeadlessWakeupError", "SessionInfo"]


class AgentPolicy(EpisodeAgentPolicy):
    """Drive an SDK agent through OpenEnv reset, decision, and finish callbacks."""

    async def act(self, obs: ParlandoObservation) -> ParlandoAction:
        """Replay callbacks and return the SDK decision as an OpenEnv action."""
        decision = await super().act(obs)
        return ParlandoAction.model_validate(decision, from_attributes=True)
