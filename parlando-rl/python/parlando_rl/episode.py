"""Drive a role-private episode through the native Parlando session engine."""

import asyncio
import json
import math
import os
import secrets
from collections.abc import Mapping
from dataclasses import dataclass, replace
from functools import lru_cache
from pathlib import Path
from threading import Lock
from typing import Any
from uuid import uuid4

import grpc
from parlando_agent_sdk import PlayerRole, start_loopback_agent_service, wire_value

from .deployment import PolicyAgent, PolicyInput
from .models import ParlandoAction, ParlandoObservation
from .policy import AgentPolicy, HeadlessWakeupError, SessionInfo
from .rewards import RewardContext, RewardFunction, completion_reward


__all__ = [
    "AgentPolicy",
    "EpisodeState",
    "HeadlessWakeupError",
    "ParlandoAction",
    "ParlandoEpisode",
    "ParlandoObservation",
    "PolicyAgent",
    "PolicyInput",
    "RewardContext",
    "RewardFunction",
    "SessionInfo",
    "completion_reward",
]


@dataclass(frozen=True)
class EpisodeState:
    """Snapshot of the current episode identity and number of learner steps."""

    episode_id: str | None = None
    step_count: int = 0


def _json(value: Any) -> str:
    """Encode values for the native driver while rejecting non-JSON numbers."""
    return json.dumps(value, allow_nan=False)


def _role(value: str) -> PlayerRole:
    """Read a seat label or its experiment schedule alias."""
    if value in ("A", "a", "player_a"):
        return "A"
    if value in ("B", "b", "player_b"):
        return "B"
    raise ValueError(f"invalid Parlando role {value!r}; expected A or B")


def _normalize_opponent(opponent: Mapping[str, Any]) -> dict[str, Any]:
    """Copy the factory settings and convert filesystem paths into JSON strings."""
    settings = dict(opponent)
    if "path" in settings:
        settings["path"] = os.fspath(settings["path"])
    return settings


_ENGINE_LOCK = Lock()


@lru_cache(maxsize=16)
def _shared_engine(
    game: str, opponent_json: str, wasm_metering: bool,
    _archive_versions: tuple[tuple[int, int, int, int], ...],
) -> Any:
    """Keep reusable native factories alive across connections in this process."""
    from . import _native

    return _native.Engine(game, opponent_json, wasm_metering)


def _load_engine(
    game: str, opponent: Mapping[str, Any], wasm_metering: bool, *, shared: bool
) -> Any:
    """Load a factory, serializing initial compilation of shared configurations."""
    game = os.path.abspath(game)
    opponent = dict(opponent)
    if "path" in opponent:
        opponent["path"] = os.path.abspath(opponent["path"])
    opponent_json = json.dumps(opponent, allow_nan=False, sort_keys=True)
    if not shared:
        from . import _native

        return _native.Engine(game, opponent_json, wasm_metering)
    archive_versions = [_archive_version(game)]
    if "path" in opponent:
        archive_versions.append(_archive_version(opponent["path"]))
    with _ENGINE_LOCK:
        return _shared_engine(game, opponent_json, wasm_metering, tuple(archive_versions))


def _archive_version(path: str) -> tuple[int, int, int, int]:
    """Detect a replacement or edited archive before reusing its compiled factory."""
    stat = os.stat(path)
    return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns


class ParlandoEpisode:
    """Run one role-private episode against a Parlando opponent.

    Reuse a configured driver for successive episodes. Each instance owns
    its current native session and any loopback server for a Python opponent.
    """

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
        """Set factory, scenario, and limit defaults for subsequent episodes."""
        self._game = os.fspath(game)
        self._opponent = _normalize_opponent(opponent)
        self._role = _role(learner_role)
        self._scenarios = dict(scenario_configs or {})
        schedule_options = dict(schedule) if isinstance(schedule, Mapping) else {"kind": schedule}
        self._schedule = schedule_options.pop("kind")
        first = schedule_options.pop("first", None)
        self._first = _role(first) if first is not None else None
        if schedule_options:
            raise ValueError(f"unknown schedule options: {sorted(schedule_options)}")
        self._limits = _json(dict(limits or {}))
        if wasm_metering in ("fuel", "none"):
            wasm_metering = wasm_metering == "fuel"
        if type(wasm_metering) is not bool:
            raise ValueError("wasm_metering must be fuel, none, or a boolean")
        self._wasm_metering = wasm_metering
        self._reward = reward
        self._engine = None
        self._session = None
        self._opponent_server: grpc.aio.Server | None = None
        self._state = EpisodeState()
        self._done = False
        self._opponent_logs: list[str] = []
        self._lock = asyncio.Lock()
        self._sync_loop: asyncio.AbstractEventLoop | None = None
        self._owner_loop: asyncio.AbstractEventLoop | None = None
        self._close_task: asyncio.Task[None] | None = None

    async def _engine_for_episode(self) -> Any:
        """Initialize the shared engine, hosting a local SDK opponent over loopback if needed."""
        if self._engine is not None:
            return self._engine
        opponent = self._opponent.copy()
        local = opponent.get("factory") == "python"
        if local:
            source = opponent.pop("source")
            server, endpoint = await start_loopback_agent_service(
                source, max_agents=opponent.pop("max_agents", 128)
            )
            self._opponent_server = server
            opponent["factory"] = "remote_grpc"
            opponent["endpoint"] = endpoint
            opponent["auth_token"] = ""
        try:
            self._engine = await asyncio.to_thread(
                _load_engine, self._game, opponent, self._wasm_metering, shared=not local
            )
        except BaseException:
            await self._close_opponent_server()
            raise
        return self._engine

    def _configuration(self, config: dict[str, Any] | str | None) -> dict[str, Any]:
        """Resolve an inline or named game configuration for reset."""
        if isinstance(config, str):
            return self._scenarios[config]
        return {} if config is None else config

    def _loop(self) -> asyncio.AbstractEventLoop:
        """Keep loop-bound transports alive between synchronous calls."""
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            pass
        else:
            raise RuntimeError("use reset_async or step_async from an event loop")
        if self._sync_loop is None:
            self._sync_loop = asyncio.new_event_loop()
        return self._sync_loop

    def reset(
        self,
        seed: int | None = None,
        episode_id: str | None = None,
        *,
        config: dict[str, Any] | str | None = None,
    ) -> ParlandoObservation:
        """Start an episode and return its first decision or terminal frame."""
        return self._loop().run_until_complete(self.reset_async(seed, episode_id, config=config))

    async def reset_async(
        self,
        seed: int | None = None,
        episode_id: str | None = None,
        *,
        config: dict[str, Any] | str | None = None,
    ) -> ParlandoObservation:
        """Start a fresh native session with an optional replay seed and scenario."""
        loop = asyncio.get_running_loop()
        if self._owner_loop is not None and self._owner_loop is not loop:
            raise RuntimeError("an episode driver must stay on the event loop that reset it")
        self._owner_loop = loop
        config_json = _json(self._configuration(config))
        seed = secrets.randbits(64) if seed is None else seed
        if type(seed) is not int or not 0 <= seed < 2**64:
            raise ValueError("seed must be an unsigned 64-bit integer")
        async with self._lock:
            await self._close_session()
            self._state = EpisodeState()
            self._opponent_logs.clear()
            engine = await self._engine_for_episode()
            self._session = engine.session(
                config_json, seed, self._role, self._schedule, self._limits, self._first
            )
            try:
                obs = await self._observation(await self._session.reset())
            except BaseException:
                await asyncio.shield(self._close_session())
                raise
            self._state = EpisodeState(episode_id=episode_id if episode_id is not None else str(uuid4()))
            self._done = obs.done
            return obs

    def step(
        self, action: ParlandoAction, timeout_s: float | None = None
    ) -> ParlandoObservation:
        """Apply a learner decision and run the opponent until the next frame."""
        return self._loop().run_until_complete(self.step_async(action, timeout_s))

    async def step_async(
        self, action: ParlandoAction, timeout_s: float | None = None
    ) -> ParlandoObservation:
        """Apply a decision with an optional result timeout and bounded cleanup."""
        action_json = _json({"action": action.action, "message": action.message})
        async with self._lock:
            if self._session is None or self._done:
                raise RuntimeError("reset the episode before stepping")
            try:
                raw = await asyncio.wait_for(self._session.step(action_json), timeout_s)
                obs = await self._observation(raw)
            except BaseException:
                await asyncio.shield(self._close_session())
                raise
            self._state = replace(self._state, step_count=self._state.step_count + 1)
            self._done = obs.done
            return obs

    @property
    def state(self) -> EpisodeState:
        """Expose only episode identity and step count."""
        return self._state

    @property
    def opponent_logs(self) -> tuple[str, ...]:
        """Return this episode's opponent logs to the local episode owner.

        Opponents can log their private inputs. The driver exposes these entries
        only through this property.
        """
        return tuple(self._opponent_logs)

    async def _observation(self, raw: str) -> ParlandoObservation:
        """Build the learner frame while keeping both seats' views local to the reward callback."""
        frame = json.loads(raw)
        metadata = frame["metadata"]
        self._opponent_logs.extend(metadata["logs"])
        if self._reward is completion_reward:
            reward = self._reward_for_frame(frame)
        else:
            reward = await asyncio.to_thread(self._reward_for_frame, frame)
        initial_view = frame["initial_view"]
        events = frame["events"]
        for event in events:
            if (transition := event["transition"]) is not None:
                transition["action"] = wire_value(transition["action"])
                transition["view"] = wire_value(transition["view"])
            if (message := event["message"]) is not None:
                message["sender"] = message.pop("from_role")
        public_metadata = {
            key: metadata[key] for key in ("status", "counters") if key in metadata
        }
        if metadata.get("completion") is not None:
            public_metadata["completion"] = wire_value(metadata["completion"])
        reason = frame["reason"]
        return ParlandoObservation(
            initial_view=wire_value(initial_view) if initial_view is not None else None,
            events=events,
            view=wire_value(frame["view"]),
            reason={"kind": reason, "rejection_code": frame["rejection_code"]} if reason else None,
            done=frame["done"],
            reward=reward,
            metadata=public_metadata,
        )

    def _reward_for_frame(self, frame: dict[str, Any]) -> float:
        """Accumulate rewards across all accepted actions between learner decisions."""
        transitions = frame["transitions"]
        reward = sum(
            self._reward(RewardContext(**transition, learner_role=self._role))
            for transition in transitions
        )
        completion = frame["metadata"].get("completion")
        if completion is not None and not any(item["completion"] is not None for item in transitions):
            reward += self._reward(RewardContext(None, None, None, None, completion, self._role))
        if not math.isfinite(reward):
            raise ValueError("reward must be finite")
        return float(reward)

    async def _close_session(self) -> None:
        """Release the native episode while keeping the compiled engine available."""
        session, self._session = self._session, None
        if session is not None:
            await session.close()

    async def close_async(self) -> None:
        """Close the episode and any loopback SDK server owned by this driver."""
        async with self._lock:
            try:
                await self._close_session()
            finally:
                await self._close_opponent_server()
                self._owner_loop = None

    async def _close_opponent_server(self) -> None:
        """Dispose of the driver-owned endpoint and its cached factory."""
        if self._opponent_server is not None:
            await self._opponent_server.stop(grace=None)
            self._opponent_server = None
            self._engine = None

    def close(self) -> None:
        """Release resources from a sync caller or schedule cleanup on the owning loop."""
        if self._session is None and self._opponent_server is None and self._sync_loop is None:
            return
        owner = self._owner_loop
        try:
            caller = asyncio.get_running_loop()
        except RuntimeError:
            caller = None
        if owner is caller and caller is not None:
            # An event-loop caller cannot block waiting on cleanup on that loop.
            if self._close_task is None or self._close_task.done():
                self._close_task = caller.create_task(self.close_async())
        elif owner is not None and owner.is_running():
            asyncio.run_coroutine_threadsafe(self.close_async(), owner).result()
        elif self._sync_loop is not None:
            runner = self._sync_loop
            try:
                runner.run_until_complete(self.close_async())
                runner.run_until_complete(runner.shutdown_asyncgens())
                runner.run_until_complete(runner.shutdown_default_executor())
            finally:
                runner.close()
                self._sync_loop = None
        elif caller is None:
            asyncio.run(self.close_async())
        else:
            self._close_task = caller.create_task(self.close_async())
