"""Combined ordinary-agent and learner gRPC service."""

from __future__ import annotations

import argparse
import asyncio
from typing import Any

import grpc
from parlando_agent_sdk import Agent, Context, Learner, Response, add_agent_service, add_learner_service

from .runtime import LearnerRuntime, RandomBackend


class CueChoiceAgent(Agent):
    """Session-local checkpoint-pinned inference handle."""

    def __init__(self, context: Context, runtime: LearnerRuntime) -> None:
        """Captures immutable session settings and the shared learner runtime."""
        if context.role != "A":
            raise ValueError("the cue-choice learner must occupy player A")
        self._runtime = runtime
        if context.checkpoint_id is None:
            raise ValueError("cue-choice inference requires a checkpoint")
        self._checkpoint = context.checkpoint_id
        self._seed = context.seed
        self._temperature = float(context.settings["temperature"])
        self._observation: dict[str, Any] = {}

    async def start(self, observation: dict[str, Any]) -> None:
        """Stores the initial learner observation."""
        self._observation = observation

    async def observe_transition(
        self, actor: str, action: dict[str, Any], observation: dict[str, Any]
    ) -> None:
        """Advances the policy's role-safe observation after an accepted action."""
        del actor, action
        self._observation = observation

    async def respond(
        self, available_actions: list[dict[str, Any]] | None
    ) -> Response | None:
        """Returns one legal action when the runner activates this agent."""
        if available_actions is None or not available_actions:
            return None
        action = await self._runtime.choose(
            self._checkpoint,
            self._observation,
            available_actions,
            self._seed,
            self._temperature,
        )
        return Response.action(action)


class CueChoiceLearner(Learner):
    """Exposes the game-specific learner through the SDK's generic service."""

    def __init__(self, runtime: LearnerRuntime) -> None:
        """Shares one checkpoint runtime with inference agents."""
        self._runtime = runtime

    async def train(self, update_id: str, base_checkpoint_id: str, completed_epochs: int,
                    steps: list[dict[str, Any]], settings: dict[str, Any]) -> str:
        """Delegates an SDK-decoded training batch to the cue-choice runtime."""
        return await self._runtime.train(
            update_id, base_checkpoint_id, completed_epochs, steps, settings
        )


async def serve(runtime: LearnerRuntime, host: str, port: int) -> None:
    """Serves both protocols on one loopback-only port until interrupted."""
    if host not in {"127.0.0.1", "::1", "localhost"}:
        raise ValueError("the v1 cue-choice learner is loopback-only")
    server = grpc.aio.server()
    add_agent_service(server, lambda context: CueChoiceAgent(context, runtime))
    add_learner_service(server, CueChoiceLearner(runtime))
    server.add_insecure_port(f"{host}:{port}")
    await server.start()
    print(f"cue-choice learner listening on {host}:{port}", flush=True)
    try:
        await server.wait_for_termination()
    finally:
        await server.stop(grace=2.0)


def _arguments() -> argparse.Namespace:
    """Parses the deliberately small process-level service configuration."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=50061)
    parser.add_argument("--backend", choices=("random", "qwen"), default="qwen")
    parser.add_argument("--model", default="Qwen/Qwen2.5-0.5B-Instruct")
    parser.add_argument("--device", choices=("mps", "cpu", "auto"), default="mps")
    return parser.parse_args()


def main() -> None:
    """Loads the selected backend before accepting experiment sessions."""
    arguments = _arguments()
    if arguments.backend == "random":
        backend = RandomBackend()
    else:
        from .qwen_backend import QwenPpoBackend

        backend = QwenPpoBackend(arguments.model, arguments.device)
    try:
        asyncio.run(serve(LearnerRuntime(backend), arguments.host, arguments.port))
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
