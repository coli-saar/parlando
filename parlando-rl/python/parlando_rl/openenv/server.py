"""Construct a WebSocket service with one Parlando episode per connection."""

import logging
import os
from collections.abc import Iterator
from contextlib import contextmanager
from functools import cache
from pathlib import Path
from typing import Any

import yaml
from fastapi import FastAPI
from openenv.core.env_server import create_app

from .environment import ParlandoEnvironment
from .models import ParlandoAction, ParlandoObservation


_logger = logging.getLogger(__name__)


@contextmanager
def _redact_episode_errors(operation: str) -> Iterator[None]:
    """Keep diagnostics about private views and local configuration server-side."""
    try:
        yield
    except Exception:
        message = f"Parlando episode {operation} failed"
        _logger.exception(message)
        raise RuntimeError(message) from None


def load_settings(path: str | Path) -> dict[str, Any]:
    """Read trusted server-side YAML and resolve its package paths relative to the file."""
    path = Path(path).resolve()
    with path.open() as file:
        settings = yaml.safe_load(file)
    if not isinstance(settings, dict):
        raise ValueError(f"{path} must contain an environment settings mapping")
    settings["game"] = path.parent / settings["game"]
    opponent = dict(settings["opponent"])
    if "path" in opponent:
        opponent["path"] = path.parent / opponent["path"]
    settings["opponent"] = opponent
    return settings


@cache
def _settings_from_environment() -> dict[str, Any]:
    """Load process configuration on the first session that needs it."""
    path = os.environ.get("PARLANDO_OPENENV_CONFIG")
    if not path:
        raise RuntimeError("set PARLANDO_OPENENV_CONFIG to an environment YAML file")
    return load_settings(path)


def create_server(
    *, max_concurrent_envs: int = 128, **settings: Any
) -> FastAPI:
    """Serve provider-supplied settings, or load process configuration from YAML."""

    class ConfiguredEnvironment(ParlandoEnvironment):
        """Create independent rollouts from one server configuration."""

        def __init__(self) -> None:
            """Defer loading process settings until the first reset."""
            super().__init__(game="", opponent={})
            self._configured = False

        def reset(
            self,
            seed: int | None = None,
            episode_id: str | None = None,
            *,
            config: dict[str, Any] | str | None = None,
        ) -> ParlandoObservation:
            """Open a synchronous episode when used outside the WebSocket."""
            with _redact_episode_errors("reset"):
                self._configure()
                return super().reset(seed, episode_id, config=config)

        async def reset_async(
            self,
            seed: int | None = None,
            episode_id: str | None = None,
            *,
            config: dict[str, Any] | str | None = None,
        ) -> ParlandoObservation:
            """Resolve server settings once, then start a fresh episode."""
            with _redact_episode_errors("reset"):
                self._configure()
                return await super().reset_async(seed, episode_id, config=config)

        def step(
            self, action: ParlandoAction, timeout_s: float | None = None
        ) -> ParlandoObservation:
            """Keep native diagnostics and callback failures in the server log."""
            with _redact_episode_errors("step"):
                return super().step(action, timeout_s)

        async def step_async(
            self, action: ParlandoAction, timeout_s: float | None = None
        ) -> ParlandoObservation:
            """Keep native diagnostics and callback failures in the server log."""
            with _redact_episode_errors("step"):
                return await super().step_async(action, timeout_s)

        def _configure(self) -> None:
            """Initialize this connection using the shared server settings."""
            if not self._configured:
                super().__init__(**(settings or _settings_from_environment()))
                self._configured = True

    return create_app(
        ConfiguredEnvironment,
        ParlandoAction,
        ParlandoObservation,
        env_name="parlando_openenv",
        max_concurrent_envs=max_concurrent_envs,
    )


app = create_server(
    max_concurrent_envs=int(os.environ.get("PARLANDO_OPENENV_MAX_SESSIONS", "128"))
)


def main() -> None:
    """Run the configured service on the OpenEnv manifest port."""
    import uvicorn

    uvicorn.run(app, host="0.0.0.0", port=int(os.environ.get("PORT", "8000")))
