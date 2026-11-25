"""OpenEnv models exposed to trainers and clients."""

from typing import Any

from openenv.core.env_server.types import Action, Observation
from pydantic import computed_field

from parlando_rl.models import ParlandoAction as EpisodeAction
from parlando_rl.models import ParlandoObservation as EpisodeObservation


class ParlandoAction(EpisodeAction, Action):
    """A game action, a message, both, or an empty decision to yield."""


class ParlandoObservation(Observation, EpisodeObservation):
    """The role-private view and all callbacks delivered since the last decision."""

    @computed_field
    @property
    def episode_metadata(self) -> dict[str, Any]:
        """Carry metadata inside the OpenEnv 0.3 observation payload."""
        # OpenEnv 0.3 drops its base Observation.metadata during serialization.
        return self.metadata
