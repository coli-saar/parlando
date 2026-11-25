"""Gymnasium rollouts and SDK agent bridges for Parlando decision points."""

import asyncio
from collections.abc import Callable
from copy import deepcopy
from typing import Any, TypeVar

import numpy as np
from gymnasium import Env, Space, spaces
from gymnasium.error import ResetNeeded
from parlando_agent_sdk import Reason

from .deployment import PolicyAgent, PolicyInput
from .episode import AgentPolicy, ParlandoAction, ParlandoEpisode, ParlandoObservation

Obs = TypeVar("Obs")
Act = TypeVar("Act")
View = dict[str, Any]


class ParlandoGymEnv(Env[Obs, Act]):
    """Expose one synchronous Parlando episode as a Gymnasium environment.

    Supply fixed spaces and game-specific codecs. The encoder receives the
    role-private frame, including ordered events, the decision reason, and
    the terminal frame. The decoder receives the
    selected Gymnasium action and the view at that decision, and returns an
    action, message, or yield. The native episode owns scheduling and rewards.
    """

    def __init__(
        self,
        episode: ParlandoEpisode,
        *,
        observation_space: Space[Obs],
        action_space: Space[Act],
        encode_observation: Callable[[ParlandoObservation], Obs],
        decode_action: Callable[[Act, View], ParlandoAction],
        sample_config: Callable[[np.random.Generator], dict[str, Any] | str | None] | None = None,
    ) -> None:
        """Bind the episode this environment resets, steps, and closes."""
        self.observation_space = observation_space
        self.action_space = action_space
        self._episode = episode
        self._encode_observation = encode_observation
        self._decode_action = decode_action
        self._sample_config = sample_config
        self._frame: ParlandoObservation | None = None

    @property
    def parlando_observation(self) -> ParlandoObservation:
        """Return the latest role-private frame for policies needing SDK callbacks."""
        if self._frame is None:
            raise ResetNeeded("reset ParlandoGymEnv before reading its observation")
        return self._frame.model_copy(deep=True)

    def reset(
        self, *, seed: int | None = None, options: dict[str, Any] | None = None
    ) -> tuple[Obs, dict[str, Any]]:
        """Start a session with optional `config` and `episode_id` reset options.

        An explicit seed is also the Parlando replay seed. Later unseeded
        resets draw replay seeds from the environment's Gymnasium RNG. If
        supplied, the scenario sampler uses the same RNG when no explicit
        configuration is provided. The reset info exposes the native status and
        opening reward. Gymnasium starts scoring rewards with the first step.
        """
        self._frame = None
        try:
            super().reset(seed=seed)
            options = options or {}
            replay_seed = seed if seed is not None else int(self.np_random.bit_generator.random_raw())
            config = options.get("config")
            if "config" not in options and self._sample_config is not None:
                config = self._sample_config(self.np_random)
            frame = self._episode.reset(
                seed=replay_seed, config=config, episode_id=options.get("episode_id")
            )
            if frame.done:
                status = frame.metadata.get("status")
                raise ResetNeeded(
                    "Parlando episode ended before the first learner decision "
                    f"({status}, opening reward {frame.reward})"
                )
            observation, info = self._record(frame)
        except BaseException:
            self.close()
            raise
        info["reward"] = frame.reward
        return observation, info

    def step(self, action: Act) -> tuple[Obs, float, bool, bool, dict[str, Any]]:
        """Submit one learner decision and classify the resulting episode status."""
        frame = self._frame
        if frame is None or frame.done:
            raise ResetNeeded("reset ParlandoGymEnv before stepping")
        action = _normalize_action(self.action_space, action)
        decision = self._decode_action(action, deepcopy(frame.view))
        self._frame = None
        successor = self._episode.step(decision)
        observation, info = self._record(successor)
        terminated = successor.done and successor.metadata.get("status") == "completed"
        truncated = successor.done and not terminated
        return observation, float(successor.reward or 0.0), terminated, truncated, info

    def _record(self, frame: ParlandoObservation) -> tuple[Obs, dict[str, Any]]:
        """Keep the SDK frame independent of mutations to Gymnasium outputs."""
        observation = self._encode_observation(frame.model_copy(deep=True))
        self._frame = frame
        return observation, deepcopy(frame.metadata)

    def close(self) -> None:
        """Release the native episode and discard the current decision."""
        self._frame = None
        self._episode.close()


class GymAgentPolicy:
    """Use an ordinary SDK agent to select Gymnasium actions.

    Drive this policy with the environment's full `parlando_observation` after
    reset, at each decision, and at the end. A game-specific encoder maps the
    SDK agent's decisions to the action space, including any modeled messages or
    yields. Use the context manager to shut down after an abandoned rollout.
    """

    def __init__(
        self,
        policy: AgentPolicy,
        *,
        encode_action: Callable[[ParlandoAction, View], Any],
    ) -> None:
        """Keep SDK callbacks on a persistent loop across synchronous calls."""
        self._policy = policy
        self._encode_action = encode_action
        self._loop = asyncio.new_event_loop()

    def __enter__(self) -> "GymAgentPolicy":
        """Enable scoped cleanup of the policy and its callback loop."""
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        """Release callback resources even when a rollout raises."""
        self.close()

    def reset(self, frame: ParlandoObservation) -> None:
        """Start a fresh SDK agent and replay the opening callbacks."""
        self._event_loop().run_until_complete(self._policy.reset(frame))

    def __call__(self, frame: ParlandoObservation) -> Any:
        """Replay new callbacks and encode the SDK agent's decision."""
        decision = self._event_loop().run_until_complete(self._policy.act(frame))
        return self._encode_action(decision, frame.view)

    def finish(self, frame: ParlandoObservation) -> None:
        """Deliver final callbacks and shut down the SDK agent for this rollout."""
        self._event_loop().run_until_complete(self._policy.finish(frame))

    def close(self) -> None:
        """Shut down the SDK agent and the loop, including after early exit."""
        if self._loop.is_closed():
            return
        loop = self._event_loop()
        try:
            loop.run_until_complete(self._policy.close())
            loop.run_until_complete(loop.shutdown_asyncgens())
            loop.run_until_complete(loop.shutdown_default_executor())
        finally:
            loop.close()

    def _event_loop(self) -> asyncio.AbstractEventLoop:
        """Require synchronous calls from outside any running event loop."""
        if self._loop.is_closed():
            raise RuntimeError("GymAgentPolicy is closed")
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            return self._loop
        raise RuntimeError("GymAgentPolicy requires a synchronous caller")


class GymPolicyAgent(PolicyAgent):
    """Serve a learned `predict` policy through the ordinary Parlando SDK.

    Reuse the training frame encoder and action decoder. At live decisions the
    encoder receives the latest private view, initial view at the first
    decision, new SDK events, and decision reason. Its reward is unavailable,
    metadata is empty, and done is false. Use these live inputs for any
    features the model needs at inference. Each agent keeps its prediction
    state per session and can yield using an optional `should_act` predicate.
    """

    def __init__(
        self,
        model: Any,
        *,
        action_space: Space,
        encode_observation: Callable[[ParlandoObservation], Any],
        decode_action: Callable[[Any, View], ParlandoAction],
        should_act: Callable[[View, Reason], bool] | None = None,
        deterministic: bool = True,
    ) -> None:
        """Bind a loaded SB3-style model and the codecs used during training."""
        super().__init__(self._predict)
        self._model = model
        self._action_space = action_space
        self._encode_observation = encode_observation
        self._decode_action = decode_action
        self._should_act = should_act
        self._deterministic = deterministic
        self._state: Any = None
        self._episode_start = True

    async def start(self, view: View) -> None:
        """Save the initial view and clear prediction state for the episode."""
        await super().start(view)
        self._state = None
        self._episode_start = True

    async def _predict(self, inputs: PolicyInput) -> ParlandoAction:
        """Run model inference on the private view and decode its SDK response."""
        view = inputs.view
        if self._should_act is not None and not self._should_act(deepcopy(view), inputs.reason):
            return ParlandoAction()
        frame = ParlandoObservation(
            view=view,
            initial_view=inputs.initial_view,
            events=list(inputs.events),
            reason=inputs.reason,
        )
        observation = self._encode_observation(frame.model_copy(deep=True))
        action, state = await asyncio.to_thread(
            self._model.predict,
            observation,
            state=self._state,
            episode_start=np.array([self._episode_start]),
            deterministic=self._deterministic,
        )
        decision = self._decode_action(_normalize_action(self._action_space, action), deepcopy(view))
        self._state = state
        self._episode_start = False
        return decision


def _normalize_action(action_space: Space, action: Any) -> Any:
    """Validate a space member and normalize scalar Discrete model predictions."""
    if not action_space.contains(action):
        raise ValueError(f"action {action!r} is outside {action_space!r}")
    return int(action) if isinstance(action_space, spaces.Discrete) else action
