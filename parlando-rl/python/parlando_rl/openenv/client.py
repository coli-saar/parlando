"""Typed OpenEnv client for persistent WebSocket sessions."""

import asyncio
from typing import Any

from openenv.core.client_types import StepResult
from openenv.core.env_client import EnvClient
from openenv.core.env_server.types import State
from websockets.exceptions import ConnectionClosed

from .models import ParlandoAction, ParlandoObservation


class ParlandoClient(EnvClient[ParlandoAction, ParlandoObservation, State]):
    """Connect a trainer to a served Parlando environment."""

    async def disconnect(self) -> None:
        """Let the server close its episode before closing the connection."""
        connection = self._ws
        if connection is None:
            return
        try:
            # OpenEnv 0.3 closes the socket after its environment cleanup.
            await self._send({"type": "close"})
            await asyncio.wait_for(connection.wait_closed(), self._message_timeout)
        except (ConnectionClosed, TimeoutError):
            pass
        finally:
            self._ws = None
            await connection.close()

    def _step_payload(self, action: ParlandoAction) -> dict[str, Any]:
        """Serialize an OpenEnv action for the WebSocket step request."""
        return action.model_dump(mode="json")

    def _parse_result(self, payload: dict[str, Any]) -> StepResult[ParlandoObservation]:
        """Reconstruct SDK event types and restore OpenEnv metadata and reward."""
        data = dict(payload["observation"])
        data["metadata"] = data.pop("episode_metadata", {})
        observation = ParlandoObservation.model_validate(
            dict(data, reward=payload.get("reward"), done=payload["done"])
        )
        return StepResult(observation, observation.reward, observation.done)

    def _parse_state(self, payload: dict[str, Any]) -> State:
        """Parse episode identity and step count."""
        return State.model_validate(payload)


class ParlandoOpenenvEnv(ParlandoClient):
    """Manifest-name entry point for OpenEnv source discovery."""
