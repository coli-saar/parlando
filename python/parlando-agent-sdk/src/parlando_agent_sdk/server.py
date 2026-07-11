"""Async gRPC server wrapper for Parlando remote agents."""

from __future__ import annotations

import asyncio
import importlib
import sys
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Awaitable, Callable, Optional

import grpc
from google.protobuf import json_format
from google.protobuf.struct_pb2 import Struct


def _generated_modules() -> tuple[Any, Any]:
    """Imports generated protobuf modules and explains how to create them if missing."""
    try:
        generated_dir = Path(__file__).resolve().parent / "generated"
        if str(generated_dir) not in sys.path:
            sys.path.insert(0, str(generated_dir))
        pb2 = importlib.import_module("parlando_agent_sdk.generated.parlando_agent_v1_pb2")
        pb2_grpc = importlib.import_module(
            "parlando_agent_sdk.generated.parlando_agent_v1_pb2_grpc"
        )
        return pb2, pb2_grpc
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            "Parlando agent protobuf modules are missing. Run "
            "`python -m parlando_agent_sdk.generate_protos` after installing the SDK."
        ) from exc


@dataclass(frozen=True)
class AgentResult:
    """Result returned by a Python agent act call."""

    kind: str
    action: Optional[dict[str, Any]] = None
    message: str = ""

    @staticmethod
    def none() -> "AgentResult":
        """Returns a no-op agent result."""
        return AgentResult(kind="none")

    @staticmethod
    def action(action: dict[str, Any]) -> "AgentResult":
        """Returns an action-only agent result."""
        return AgentResult(kind="action", action=action)

    @staticmethod
    def message(message: str) -> "AgentResult":
        """Returns a message-only agent result."""
        return AgentResult(kind="message", message=message)

    @staticmethod
    def action_with_message(action: dict[str, Any], message: str) -> "AgentResult":
        """Returns a combined action and message agent result."""
        return AgentResult(kind="action_with_message", action=action, message=message)


class GameAgent:
    """Base class for Python remote agents."""

    async def act(
        self, observation: dict[str, Any], available_actions: Optional[list[dict[str, Any]]]
    ) -> AgentResult:
        """Chooses the agent result from the same player-facing view shown to the UI."""
        raise NotImplementedError

    async def close(self) -> None:
        """Releases any per-agent resources before shutdown."""
        return None


AgentFactory = Callable[[dict[str, Any]], GameAgent | Awaitable[GameAgent]]


class _AgentService:
    """Generated gRPC servicer implementation backed by Python GameAgent instances."""

    def __init__(self, factory: AgentFactory) -> None:
        """Creates a service that instantiates one Python agent per CreateAgent call."""
        self._factory = factory
        self._agents: dict[str, GameAgent] = {}
        self._pb2, _pb2_grpc = _generated_modules()

    async def CreateAgent(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote CreateAgent request from the Rust server."""
        init_context = {
            "protocol_version": request.protocol_version,
            "agent_name": request.agent_name,
            "agent_version": request.agent_version,
            "role": request.role,
            "seed": request.seed if request.HasField("seed") else None,
            "config": _struct_to_dict(request.config),
        }
        maybe_agent = self._factory(init_context)
        agent = await maybe_agent if asyncio.iscoroutine(maybe_agent) else maybe_agent
        agent_id = str(uuid.uuid4())
        self._agents[agent_id] = agent
        return self._pb2.CreateAgentResponse(agent_id=agent_id)

    async def Act(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Act request from the Rust server."""
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        observation = _struct_to_dict(request.observation)
        available_actions = (
            [_struct_to_dict(action) for action in request.available_actions]
            if request.available_actions_provided
            else None
        )
        result = await agent.act(observation, available_actions)
        return _result_to_response(self._pb2, result)

    async def Shutdown(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Shutdown request from the Rust server."""
        agent = self._agents.pop(request.agent_id, None)
        if agent is not None:
            await agent.close()
        return self._pb2.ShutdownResponse()


async def serve_agent_async(
    factory: type[GameAgent] | AgentFactory,
    host: str = "127.0.0.1",
    port: int = 50051,
) -> None:
    """Starts an async gRPC server for a Python Parlando agent factory."""
    pb2, pb2_grpc = _generated_modules()

    def normalized_factory(context: dict[str, Any]) -> GameAgent | Awaitable[GameAgent]:
        """Creates a GameAgent from either a class or a callable factory."""
        if isinstance(factory, type):
            return factory()
        return factory(context)

    server = grpc.aio.server()
    pb2_grpc.add_AgentServiceServicer_to_server(_AgentService(normalized_factory), server)
    server.add_insecure_port(f"{host}:{port}")
    await server.start()
    await server.wait_for_termination()


def serve_agent(
    factory: type[GameAgent] | AgentFactory,
    host: str = "127.0.0.1",
    port: int = 50051,
) -> None:
    """Runs a Python Parlando agent server until interrupted."""
    asyncio.run(serve_agent_async(factory, host=host, port=port))


def _struct_to_dict(value: Struct) -> dict[str, Any]:
    """Converts a protobuf Struct into a plain Python dictionary."""
    return json_format.MessageToDict(value, preserving_proto_field_name=True)


def _dict_to_struct(value: dict[str, Any]) -> Struct:
    """Converts a plain Python dictionary into a protobuf Struct."""
    struct = Struct()
    json_format.ParseDict(value, struct)
    return struct


def _result_to_response(pb2: Any, result: AgentResult) -> Any:
    """Converts an AgentResult into the generated protobuf response type."""
    if result.kind == "none":
        return pb2.ActResponse(result_kind=pb2.AGENT_RESULT_KIND_NONE)
    if result.kind == "message":
        return pb2.ActResponse(
            result_kind=pb2.AGENT_RESULT_KIND_MESSAGE,
            message=result.message,
        )
    if result.kind == "action":
        return pb2.ActResponse(
            result_kind=pb2.AGENT_RESULT_KIND_ACTION,
            action=_dict_to_struct(result.action or {}),
        )
    if result.kind == "action_with_message":
        return pb2.ActResponse(
            result_kind=pb2.AGENT_RESULT_KIND_ACTION_WITH_MESSAGE,
            action=_dict_to_struct(result.action or {}),
            message=result.message,
        )
    raise ValueError(f"unknown AgentResult kind: {result.kind}")
