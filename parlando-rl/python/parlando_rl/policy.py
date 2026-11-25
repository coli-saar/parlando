"""Replay a Parlando episode into an ordinary Parlando SDK agent."""

import asyncio
from dataclasses import dataclass, field
from typing import Any

from parlando_agent_sdk import Agent, AgentSource, Context, PlayerRole, create_agent, wire_value

from .models import ParlandoAction, ParlandoObservation


class HeadlessWakeupError(ValueError):
    """An agent requested a live wakeup during headless training."""


@dataclass(frozen=True)
class SessionInfo:
    """The learner's role, seed, and agent settings for one episode."""

    role: PlayerRole
    seed: int
    settings: dict[str, Any] = field(default_factory=dict)
    checkpoint_id: str | None = None

    def context(self) -> Context:
        """Build a fresh SDK context with the same wire values as remote evaluation."""
        return Context(self.role, self.seed, wire_value(self.settings), self.checkpoint_id)


class AgentPolicy:
    """Drive an SDK agent through reset, decision, and finish callbacks."""

    def __init__(self, source: AgentSource, info: SessionInfo) -> None:
        """Bind an SDK agent source to the learner's session information."""
        self._lock = asyncio.Lock()
        self._source = source
        self._info = info
        self._context: Context | None = None
        self._agent: Agent | None = None
        self.logs: list[str] = []

    async def reset(self, obs: ParlandoObservation) -> None:
        """Create a fresh agent and deliver start and any opening events."""
        async with self._lock:
            await self._reset_agent(obs)

    async def _reset_agent(self, obs: ParlandoObservation) -> None:
        """Replace the agent and deliver the initial callbacks under the policy lock."""
        if obs.initial_view is None:
            raise ValueError("policy reset requires an environment reset observation")
        await self._close_agent()
        self.logs.clear()
        self._context = self._info.context()
        self._agent = await create_agent(self._source, self._context)
        try:
            await self._agent.start(obs.initial_view)
            await self._deliver(obs)
        except BaseException:
            await self._close_agent()
            raise
        self._collect_logs(obs)

    async def act(self, obs: ParlandoObservation) -> ParlandoAction:
        """Deliver new events and request a decision using the host's reason."""
        async with self._lock:
            return await self._act(obs)

    async def _act(self, obs: ParlandoObservation) -> ParlandoAction:
        """Run observations and decide as one serialized callback sequence."""
        if obs.done or obs.reason is None:
            raise ValueError("a completed observation has no decision")
        agent = self._require_agent()
        if obs.initial_view is None:
            await self._deliver(obs)
        decision = await agent.decide(obs.reason)
        self._collect_logs(obs)
        if decision.wakeup is not None:
            raise HeadlessWakeupError("a headless agent cannot request or cancel a wakeup")
        action = wire_value(decision.action) if decision.action is not None else None
        return ParlandoAction(action=action, message=decision.message)

    async def finish(self, obs: ParlandoObservation) -> None:
        """Deliver terminal events, finish when the game produced a completion, and shut down."""
        async with self._lock:
            await self._finish(obs)

    async def _finish(self, obs: ParlandoObservation) -> None:
        """Run terminal callbacks under the policy lock and publish their logs."""
        if not obs.done:
            raise ValueError("finish requires a completed observation")
        agent = self._require_agent()
        try:
            if obs.initial_view is None:
                await self._deliver(obs)
            if "completion" in obs.metadata:
                await agent.finish(obs.metadata["completion"])
        finally:
            await self._close_agent(obs)

    async def close(self) -> None:
        """Shut down the current agent, including when a rollout stops early."""
        async with self._lock:
            await self._close_agent()

    async def _close_agent(self, obs: ParlandoObservation | None = None) -> None:
        """Shut down under the policy lock and drain final log entries."""
        agent, self._agent = self._agent, None
        if agent is not None:
            try:
                await agent.shutdown()
            finally:
                self._collect_logs(obs)
                self._context = None

    def _collect_logs(self, obs: ParlandoObservation | None) -> None:
        """Copy learner logs into the local episode log and the associated frame."""
        if self._context is not None:
            entries = self._context.logger.drain()
            self.logs.extend(entries)
            if obs is not None and entries:
                obs.metadata.setdefault("logs", []).extend(entries)

    def _require_agent(self) -> Agent:
        """Return the current agent or report an invalid callback sequence."""
        if self._agent is None:
            raise RuntimeError("reset the policy before using it")
        return self._agent

    async def _deliver(self, obs: ParlandoObservation) -> None:
        """Replay a batch in order. Reset owns all batches carrying an initial view."""
        agent = self._require_agent()
        for event in obs.events:
            await agent.observe(event)
