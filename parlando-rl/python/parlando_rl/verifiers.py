"""Run Parlando games as Verifiers v1 episodes and serve the resulting model policy."""

import json
import math
from collections.abc import Awaitable, Callable, Iterator
from dataclasses import asdict
from importlib import import_module
from pathlib import Path
from typing import Any

import verifiers.v1 as vf
from parlando_agent_sdk import AgentSource, PlayerRole
from pydantic import Field

from .deployment import PolicyAgent, PolicyInput
from .episode import (
    AgentPolicy,
    ParlandoAction,
    ParlandoEpisode,
    ParlandoObservation,
    RewardContext,
    SessionInfo,
    completion_reward,
)


__all__ = [
    "ParlandoVerifiersEnv",
    "ParlandoTaskset",
    "ParlandoModelAgent",
    "ParlandoVerifiersConfig",
    "ParlandoTasksetConfig",
]

SYSTEM_PROMPT = (
    "You are one player in a Parlando game. Each user turn is a JSON decision frame. "
    "Use the view, events, and decision reason to choose your next move. "
    'Reply with one JSON object containing optional "action" (a game action object) '
    'and "message" (text to send to the other player). Use {} to yield.'
)
ChatMessages = list[dict[str, str]]
ChatCompletion = Callable[[ChatMessages], Awaitable[str]]


def decision_prompt(inputs: PolicyInput) -> str:
    """Render one seat's callbacks in the format used for rollouts and inference."""
    return json.dumps(
        {
            "view": inputs.view,
            "events": [asdict(event) for event in inputs.events],
            "reason": asdict(inputs.reason),
            "initial_view": inputs.initial_view,
        },
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
    )


def _reject_nonfinite(token: str) -> None:
    """Reject non-JSON numeric literals anywhere in a model action."""
    raise ValueError(f"invalid JSON number {token}")


def _read_finite_float(token: str) -> float:
    """Decode a decimal or exponent, rejecting overflow to infinity."""
    value = float(token)
    if not math.isfinite(value):
        _reject_nonfinite(token)
    return value


def parse_decision(reply: str) -> ParlandoAction:
    """Validate one JSON action/message envelope, including an explicit yield."""
    raw = json.loads(
        reply, parse_constant=_reject_nonfinite, parse_float=_read_finite_float
    )
    if not isinstance(raw, dict) or raw.keys() - {"action", "message"}:
        raise ValueError(
            "reply must be a JSON object with only action and message fields"
        )
    return ParlandoAction.model_validate(raw, strict=True)


class ParlandoTaskData(vf.TaskData):
    """A reproducible game reset carried in a Verifiers task."""

    seed: int = Field(ge=0, lt=2**64)
    scenario: str | dict[str, Any] | None = None


class ParlandoTask(vf.Task[ParlandoTaskData]):
    """A host-driven game task whose first user turn is the learner's opening frame."""


class ParlandoTasksetConfig(vf.TasksetConfig):
    """Create one task per combination of seed and scenario."""

    seeds: list[int] = Field(default_factory=lambda: [0], min_length=1)
    scenarios: list[str | dict[str, Any] | None] = Field(
        default_factory=lambda: [None], min_length=1
    )
    instructions: str = SYSTEM_PROMPT


class ParlandoTaskset(vf.Taskset[ParlandoTask, ParlandoTasksetConfig]):
    """Generate deterministic tasks that can be split or selected by Verifiers."""

    def load(self) -> Iterator[ParlandoTask]:
        """Yield host-driven tasks with the prompt and input needed to reset a game."""
        for scenario in self.config.scenarios:
            for seed in self.config.seeds:
                yield ParlandoTask(
                    ParlandoTaskData(
                        seed=seed,
                        scenario=scenario,
                        system_prompt=self.config.instructions,
                    ),
                    self.config.task,
                )


class ParlandoVerifiersConfig(vf.EnvConfig):
    """Configure the game and opponent that each Verifiers episode instantiates."""

    player: vf.AgentConfig = vf.AgentConfig(
        harness=vf.HarnessConfig(id="null"),
        runtime=vf.SubprocessConfig(),
    )
    game: Path
    opponent: dict[str, Any]
    learner_role: PlayerRole = "A"
    scenario_configs: dict[str, dict[str, Any]] = Field(default_factory=dict)
    schedule: str | dict[str, Any] = "alternate_after_action"
    limits: dict[str, Any] = Field(default_factory=dict)
    wasm_metering: bool | str = True
    sdk_agent: str | None = Field(None, pattern=r"^[\w.]+:[\w]+$")
    """Optional module:factory for a Python SDK agent in model-free evaluation."""
    sdk_settings: dict[str, Any] = Field(default_factory=dict)
    sdk_checkpoint_id: str | None = None


class ParlandoVerifiersEnv(vf.Env[ParlandoVerifiersConfig]):
    """Play the trained seat through one Verifiers interaction per game."""

    def make_episode(self) -> ParlandoEpisode:
        """Create fresh per-rollout state using the environment's native settings."""
        config = self.config
        return ParlandoEpisode(
            game=config.game,
            opponent=config.opponent,
            learner_role=config.learner_role,
            scenario_configs=config.scenario_configs,
            schedule=config.schedule,
            limits=config.limits,
            wasm_metering=config.wasm_metering,
            reward=self.reward,
        )

    def reward(self, context: RewardContext) -> float:
        """Score an accepted game transition or terminal frame on the host."""
        return completion_reward(context)

    async def setup(self, agents: vf.Agents) -> None:
        """Exclude an SDK policy's trace from token-based training."""
        if self.config.sdk_agent is not None:
            agents.player.trainable = False

    async def run(self, task: ParlandoTask, agents: vf.Agents) -> None:
        """Play one episode and attach the game reward to the Verifiers trace."""
        episode = self.make_episode()
        try:
            frame = await episode.reset_async(
                seed=task.data.seed, config=task.data.scenario
            )
            async with agents.player.interaction(task) as interaction:
                if self.config.sdk_agent is None:
                    frame, total = await self._run_model(episode, frame, interaction)
                else:
                    frame, total = await self._run_sdk(
                        episode, frame, interaction.trace, task
                    )
                interaction.trace.info["parlando_final_frame"] = frame.model_dump(
                    mode="json",
                    exclude={"metadata": {"logs"}},
                )
                interaction.trace.record_reward("parlando_reward", total)
        finally:
            await episode.close_async()

    async def _run_model(
        self,
        episode: ParlandoEpisode,
        frame: ParlandoObservation,
        interaction: vf.Interaction,
    ) -> tuple[ParlandoObservation, float]:
        """Alternate model replies and game steps until either participant stops."""
        total = frame.reward or 0.0
        while not frame.done:
            reply = await interaction.turn(_frame_prompt(frame))
            if reply.terminated:
                break
            try:
                action = parse_decision(reply.last_reply)
            except ValueError:
                interaction.trace.stop("invalid_parlando_reply")
                break
            frame = await episode.step_async(action)
            total += frame.reward or 0.0
        if frame.done:
            interaction.trace.stop("parlando_done")
        return frame, total

    async def _run_sdk(
        self,
        episode: ParlandoEpisode,
        frame: ParlandoObservation,
        trace: vf.Trace,
        task: ParlandoTask,
    ) -> tuple[ParlandoObservation, float]:
        """Score an SDK policy and retain its decisions without simulated model calls."""
        module, _, factory = self.config.sdk_agent.partition(":")
        source: AgentSource = getattr(import_module(module), factory)
        policy = AgentPolicy(
            source,
            SessionInfo(
                self.config.learner_role,
                task.data.seed,
                self.config.sdk_settings,
                self.config.sdk_checkpoint_id,
            ),
        )
        total = frame.reward or 0.0
        trace.info["parlando_sdk_agent"] = self.config.sdk_agent
        try:
            await policy.reset(frame)
            while not frame.done:
                action = await policy.act(frame)
                for message in (
                    vf.UserMessage(content=_frame_prompt(frame)),
                    vf.AssistantMessage(
                        content=action.model_dump_json(exclude_none=True)
                    ),
                ):
                    parent = len(trace.nodes) - 1 if trace.nodes else None
                    trace.nodes.append(vf.MessageNode(message=message, parent=parent))
                frame = await episode.step_async(action)
                total += frame.reward or 0.0
            await policy.finish(frame)
            trace.stop("parlando_done")
            return frame, total
        finally:
            await policy.close()


def _frame_prompt(frame: ParlandoObservation) -> str:
    """Render a nonterminal game frame with the shared inference codec."""
    if frame.reason is None:
        raise ValueError("a decision frame requires a Parlando reason")
    return decision_prompt(
        PolicyInput(frame.view, frame.reason, tuple(frame.events), frame.initial_view)
    )


class ParlandoModelAgent(PolicyAgent):
    """Use a chat checkpoint as an SDK agent with the Verifiers decision protocol.

    Supply an async chat completion callable addressed to the checkpoint and
    use the same instructions used for training. The agent owns its transcript.
    The callable owns any connection or model resources it captures.
    """

    def __init__(
        self, complete: ChatCompletion, *, instructions: str = SYSTEM_PROMPT
    ) -> None:
        """Bind a model backend for one SDK session."""
        super().__init__(self._predict)
        self._complete = complete
        self._instructions = instructions
        self._messages: ChatMessages = []

    async def start(self, observation: dict[str, Any]) -> None:
        """Initialize the SDK view and reset the transcript for a new game."""
        await super().start(observation)
        self._messages = [{"role": "system", "content": self._instructions}]

    async def _predict(self, inputs: PolicyInput) -> ParlandoAction:
        """Continue the transcript by one model turn and decode its decision."""
        turn = {"role": "user", "content": decision_prompt(inputs)}
        reply = await self._complete([*self._messages, turn])
        action = parse_decision(reply)
        self._messages.extend((turn, {"role": "assistant", "content": reply}))
        return action
