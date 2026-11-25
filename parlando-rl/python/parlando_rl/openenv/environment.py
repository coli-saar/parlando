"""Expose the native Parlando episode through OpenEnv's environment interface."""

from collections.abc import Mapping
from pathlib import Path
from typing import Any

from openenv.core.env_server.interfaces import Environment
from openenv.core.env_server.types import State
from parlando_agent_sdk import PlayerRole

from parlando_rl.episode import ParlandoEpisode, RewardFunction, completion_reward

from .models import ParlandoAction, ParlandoObservation


class ParlandoEnvironment(Environment[ParlandoAction, ParlandoObservation, State]):
    """Run one role-private Parlando episode per OpenEnv environment instance."""

    SUPPORTS_CONCURRENT_SESSIONS = True

    def __init__(
        self,
        *,
        game: str | Path,
        opponent: Mapping[str, Any],
        learner_role: PlayerRole = "A",
        scenario_configs: Mapping[str, dict[str, Any]] | None = None,
        schedule: str | Mapping[str, Any] = "alternate_after_action",
        limits: Mapping[str, Any] | None = None,
        wasm_metering: bool | str = True,
        reward: RewardFunction = completion_reward,
    ) -> None:
        """Configure the episode driver for this OpenEnv environment."""
        super().__init__()
        self._episode = ParlandoEpisode(
            game=game,
            opponent=opponent,
            learner_role=learner_role,
            scenario_configs=scenario_configs,
            schedule=schedule,
            limits=limits,
            wasm_metering=wasm_metering,
            reward=reward,
        )

    def reset(
        self,
        seed: int | None = None,
        episode_id: str | None = None,
        *,
        config: dict[str, Any] | str | None = None,
    ) -> ParlandoObservation:
        """Start an episode from a synchronous OpenEnv caller."""
        observation = self._episode.reset(seed, episode_id, config=config)
        return ParlandoObservation.model_validate(observation, from_attributes=True)

    async def reset_async(
        self,
        seed: int | None = None,
        episode_id: str | None = None,
        *,
        config: dict[str, Any] | str | None = None,
    ) -> ParlandoObservation:
        """Start an episode on the caller's event loop."""
        observation = await self._episode.reset_async(seed, episode_id, config=config)
        return ParlandoObservation.model_validate(observation, from_attributes=True)

    def step(
        self, action: ParlandoAction, timeout_s: float | None = None
    ) -> ParlandoObservation:
        """Advance the episode from a synchronous OpenEnv caller."""
        observation = self._episode.step(action, timeout_s)
        return ParlandoObservation.model_validate(observation, from_attributes=True)

    async def step_async(
        self, action: ParlandoAction, timeout_s: float | None = None
    ) -> ParlandoObservation:
        """Advance the episode on the caller's event loop."""
        observation = await self._episode.step_async(action, timeout_s)
        return ParlandoObservation.model_validate(observation, from_attributes=True)

    @property
    def state(self) -> State:
        """Expose only episode identity and step count."""
        return State.model_validate(self._episode.state, from_attributes=True)

    @property
    def opponent_logs(self) -> tuple[str, ...]:
        """Return private opponent logs to the local environment owner."""
        return self._episode.opponent_logs

    def close(self) -> None:
        """Release the native session through OpenEnv's synchronous cleanup hook."""
        self._episode.close()

    async def close_async(self) -> None:
        """Release the native session and any loopback server on the owning loop."""
        await self._episode.close_async()
