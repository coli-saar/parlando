"""Async gRPC server wrapper for Parlando remote agents."""

from __future__ import annotations

import asyncio
import hmac
import importlib
import os
import sys
import uuid
from dataclasses import FrozenInstanceError
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


class AgentResponse:
    """Non-empty response returned by a Python agent decision call."""

    def __init__(
        self,
        action: Optional[dict[str, Any]] = None,
        message: Optional[str] = None,
    ) -> None:
        """Creates an immutable response value without colliding with named constructors."""
        object.__setattr__(self, "action", action)
        object.__setattr__(self, "message", message)

    def __setattr__(self, name: str, value: object) -> None:
        """Rejects mutation after construction like a frozen dataclass."""
        raise FrozenInstanceError(f"cannot assign to field {name!r}")

    def __eq__(self, other: object) -> bool:
        """Compares response payloads structurally for application and test code."""
        return (
            isinstance(other, AgentResponse)
            and self.action == other.action
            and self.message == other.message
        )

    def __repr__(self) -> str:
        """Returns a diagnostic representation without hiding either optional field."""
        return f"AgentResponse(action={self.action!r}, message={self.message!r})"

    @staticmethod
    def action(action: dict[str, Any]) -> "AgentResponse":
        """Creates an action-only agent response."""
        return AgentResponse(action=action)

    @staticmethod
    def say(message: str) -> "AgentResponse":
        """Creates a message-only agent response."""
        return AgentResponse(message=message)

    @staticmethod
    def action_with_message(action: dict[str, Any], message: str) -> "AgentResponse":
        """Creates a combined action and message agent response."""
        return AgentResponse(action=action, message=message)


class GameAgent:
    """Base class for Python remote agents."""

    async def observe_state(self, current_observation: dict[str, Any]) -> None:
        """Observes the current role-specific state snapshot."""
        return None

    async def observe_action(
        self,
        actor: str,
        action: dict[str, Any],
        resulting_observation: dict[str, Any],
    ) -> None:
        """Observes an accepted action and the resulting role-specific state."""
        return None

    async def observe_message(self, speaker: str, kind: str, text: str) -> None:
        """Observes a conversation utterance from a known player role."""
        return None

    async def maybe_act(
        self, available_actions: Optional[list[dict[str, Any]]]
    ) -> Optional[AgentResponse]:
        """Optionally chooses a message and/or action after prior observations."""
        return None

    async def act(self, available_actions: Optional[list[dict[str, Any]]]) -> AgentResponse:
        """Chooses a required non-empty response."""
        raise NotImplementedError

    async def close(self) -> None:
        """Releases any per-agent resources before shutdown."""
        return None


AgentFactory = Callable[[dict[str, Any]], GameAgent | Awaitable[GameAgent]]


class _AgentService:
    """Generated gRPC servicer implementation backed by Python GameAgent instances."""

    def __init__(
        self, factory: AgentFactory, auth_token: str | None = None, max_agents: int = 128
    ) -> None:
        """Creates a service that instantiates one Python agent per CreateAgent call."""
        self._factory = factory
        self._auth_token = auth_token
        self._max_agents = max_agents
        self._agents: dict[str, GameAgent] = {}
        self._creating_agents = 0
        self._agent_lock = asyncio.Lock()
        self._pb2, _pb2_grpc = _generated_modules()

    async def _authenticate(self, context: grpc.aio.ServicerContext) -> None:
        """Rejects an RPC whose bearer metadata does not match the runtime-only token."""
        if self._auth_token is None:
            return
        supplied = dict(context.invocation_metadata()).get("authorization", "")
        expected = f"Bearer {self._auth_token}"
        if not hmac.compare_digest(supplied, expected):
            await context.abort(grpc.StatusCode.UNAUTHENTICATED, "authentication required")

    async def CreateAgent(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote CreateAgent request from the Rust server."""
        await self._authenticate(context)
        async with self._agent_lock:
            if len(self._agents) + self._creating_agents >= self._max_agents:
                await context.abort(
                    grpc.StatusCode.RESOURCE_EXHAUSTED, "agent capacity reached"
                )
            self._creating_agents += 1
        init_context = {
            "protocol_version": request.protocol_version,
            "agent_name": request.agent_name,
            "agent_version": request.agent_version,
            "role": request.role,
            "seed": request.seed if request.HasField("seed") else None,
            "config": _struct_to_dict(request.config),
        }
        try:
            maybe_agent = self._factory(init_context)
            agent = (
                await maybe_agent if asyncio.iscoroutine(maybe_agent) else maybe_agent
            )
            agent_id = str(uuid.uuid4())
            async with self._agent_lock:
                self._agents[agent_id] = agent
        finally:
            async with self._agent_lock:
                self._creating_agents -= 1
        return self._pb2.CreateAgentResponse(agent_id=agent_id)

    async def ObserveState(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote ObserveState request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        await agent.observe_state(_struct_to_dict(request.current_observation))
        return self._pb2.ObserveResponse()

    async def ObserveAction(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote ObserveAction request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        await agent.observe_action(
            request.actor,
            _struct_to_dict(request.action),
            _struct_to_dict(request.resulting_observation),
        )
        return self._pb2.ObserveResponse()

    async def ObserveMessage(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote ObserveMessage request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        await agent.observe_message(
            request.speaker,
            _utterance_kind(self._pb2, request.kind),
            request.text,
        )
        return self._pb2.ObserveResponse()

    async def MaybeAct(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote MaybeAct request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        result = await agent.maybe_act(_available_actions(request))
        if result is None:
            return self._pb2.MaybeActResponse()
        return self._pb2.MaybeActResponse(response=_response_to_proto(self._pb2, result))

    async def Act(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Act request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        return _response_to_proto(self._pb2, await agent.act(_available_actions(request)))

    async def Shutdown(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Shutdown request from the Rust server."""
        await self._authenticate(context)
        async with self._agent_lock:
            agent = self._agents.pop(request.agent_id, None)
        if agent is not None:
            await agent.close()
        return self._pb2.ShutdownResponse()


async def serve_agent_async(
    factory: type[GameAgent] | AgentFactory,
    host: str = "127.0.0.1",
    port: int = 50051,
    certificate_chain: bytes | None = None,
    private_key: bytes | None = None,
    client_ca: bytes | None = None,
    auth_token: str | None = None,
    max_agents: int = 128,
) -> None:
    """Starts a bounded gRPC server, requiring TLS for every non-loopback binding."""
    if max_agents <= 0:
        raise ValueError("max_agents must be greater than zero")
    if not 0 <= port <= 65535:
        raise ValueError("port must be between 0 and 65535")
    pb2, pb2_grpc = _generated_modules()

    def normalized_factory(context: dict[str, Any]) -> GameAgent | Awaitable[GameAgent]:
        """Creates a GameAgent from either a class or a callable factory."""
        if isinstance(factory, type):
            return factory()
        return factory(context)

    auth_token = auth_token or os.environ.get("PARLANDO_REMOTE_AGENT_TOKEN")
    server = grpc.aio.server(
        options=(
            ("grpc.max_receive_message_length", 1_048_576),
            ("grpc.max_send_message_length", 1_048_576),
            ("grpc.max_concurrent_streams", 128),
        )
    )
    pb2_grpc.add_AgentServiceServicer_to_server(
        _AgentService(normalized_factory, auth_token, max_agents), server
    )
    if (certificate_chain is None) != (private_key is None):
        raise ValueError("certificate_chain and private_key must be configured together")
    if certificate_chain is not None and private_key is not None:
        credentials = grpc.ssl_server_credentials(
            ((private_key, certificate_chain),),
            root_certificates=client_ca,
            require_client_auth=client_ca is not None,
        )
        server.add_secure_port(f"{host}:{port}", credentials)
    elif host in {"127.0.0.1", "::1", "localhost"}:
        server.add_insecure_port(f"{host}:{port}")
    else:
        raise ValueError("non-loopback remote-agent bindings require TLS credentials")
    if host not in {"127.0.0.1", "::1", "localhost"} and client_ca is None and auth_token is None:
        raise ValueError("non-loopback remote-agent bindings require mTLS or a bearer token")
    await server.start()
    await server.wait_for_termination()


def serve_agent(
    factory: type[GameAgent] | AgentFactory,
    host: str = "127.0.0.1",
    port: int = 50051,
    certificate_chain: bytes | None = None,
    private_key: bytes | None = None,
    client_ca: bytes | None = None,
    auth_token: str | None = None,
    max_agents: int = 128,
) -> None:
    """Runs a Python Parlando agent server with the selected TLS or loopback policy."""
    asyncio.run(
        serve_agent_async(
            factory,
            host=host,
            port=port,
            certificate_chain=certificate_chain,
            private_key=private_key,
            client_ca=client_ca,
            auth_token=auth_token,
            max_agents=max_agents,
        )
    )


def _struct_to_dict(value: Struct) -> dict[str, Any]:
    """Converts a protobuf Struct into a plain Python dictionary."""
    return json_format.MessageToDict(value, preserving_proto_field_name=True)


def _dict_to_struct(value: dict[str, Any]) -> Struct:
    """Converts a plain Python dictionary into a protobuf Struct."""
    struct = Struct()
    json_format.ParseDict(value, struct)
    return struct


def _available_actions(request: Any) -> Optional[list[dict[str, Any]]]:
    """Converts optional protobuf action hints into plain Python dictionaries."""
    if not request.available_actions_provided:
        return None
    return [_struct_to_dict(action) for action in request.available_actions]


def _utterance_kind(pb2: Any, value: int) -> str:
    """Converts a protobuf utterance kind into the Python string API."""
    if value == pb2.UTTERANCE_KIND_SPOKEN:
        return "spoken"
    if value == pb2.UTTERANCE_KIND_AGENT:
        return "agent"
    return "typed"


def _response_to_proto(pb2: Any, response: AgentResponse) -> Any:
    """Converts an AgentResponse into the generated protobuf response type."""
    if response.message is None and response.action is None:
        raise ValueError("AgentResponse must include a message or action")
    converted = pb2.AgentResponse()
    if response.message is not None:
        converted.message = response.message
    if response.action is not None:
        converted.action.CopyFrom(_dict_to_struct(response.action))
    return converted
