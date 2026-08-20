"""Async gRPC server wrapper for Parlando remote agents."""

from __future__ import annotations

import asyncio
import hmac
import importlib
import inspect
import os
import sys
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, Literal, Optional

import grpc
from google.protobuf import json_format
from google.protobuf.struct_pb2 import Struct


def _generated_modules() -> tuple[Any, Any]:
    """Imports generated protobuf modules and explains how to create them if missing."""
    try:
        generated_dir = Path(__file__).resolve().parent / "generated"
        if str(generated_dir) not in sys.path:
            sys.path.insert(0, str(generated_dir))
        pb2 = importlib.import_module("parlando_agent_sdk.generated.parlando_agent_v3_pb2")
        pb2_grpc = importlib.import_module(
            "parlando_agent_sdk.generated.parlando_agent_v3_pb2_grpc"
        )
        return pb2, pb2_grpc
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            "Parlando agent protobuf modules are missing. Run "
            "`python -m parlando_agent_sdk.generate_protos` after installing the SDK."
        ) from exc


PlayerRole = Literal["A", "B"]


class SecretValues:
    """Non-serializable values explicitly authorized for this agent instance."""

    def __init__(self, values: dict[str, str]) -> None:
        """Stores one isolated path-to-value mapping."""
        self._values = dict(values)

    def get(self, path: str) -> str | None:
        """Returns one referenced value by its semantic configuration path."""
        return self._values.get(path)

    def __repr__(self) -> str:
        """Redacts all secret material from diagnostics."""
        return "SecretValues([REDACTED])"

    def __eq__(self, other: object) -> bool:
        """Compares isolated values without exposing them in assertion output."""
        return isinstance(other, SecretValues) and self._values == other._values


@dataclass(frozen=True)
class Context:
    """Session-local information supplied before an agent receives game data."""

    role: PlayerRole
    seed: int
    settings: dict[str, Any]
    secrets: SecretValues = field(default_factory=lambda: SecretValues({}))


@dataclass(frozen=True)
class Response:
    """Non-empty response returned by a Python agent decision call."""

    _action: Optional[dict[str, Any]] = None
    _message: Optional[str] = None

    def __post_init__(self) -> None:
        """Rejects the empty value because declining to respond is represented by None."""
        if self._action is None and self._message is None:
            raise ValueError("Response must include an action or message")

    @staticmethod
    def action(action: dict[str, Any]) -> "Response":
        """Creates an action-only agent response."""
        return Response(_action=action)

    @staticmethod
    def message(message: str) -> "Response":
        """Creates a message-only agent response."""
        return Response(_message=message)

    @staticmethod
    def action_with_message(action: dict[str, Any], message: str) -> "Response":
        """Creates a combined action and message agent response."""
        return Response(_action=action, _message=message)


class Agent:
    """Base class for Python remote agents."""

    async def start(self, observation: dict[str, Any]) -> None:
        """Receives the first role-specific observation after initialization."""
        return None

    async def observe_transition(
        self,
        actor: PlayerRole,
        action: dict[str, Any],
        observation: dict[str, Any],
    ) -> None:
        """Observes an accepted action and the resulting role-specific observation."""
        return None

    async def observe_message(self, sender: PlayerRole, text: str) -> None:
        """Observes a player-to-player message from a known role."""
        return None

    async def finish(self, completion: dict[str, Any]) -> None:
        """Receives the shared game-specific terminal result before shutdown."""
        return None

    async def respond(
        self, available_actions: Optional[list[dict[str, Any]]]
    ) -> Optional[Response]:
        """Optionally chooses a message and/or action after prior observations."""
        return None

    async def shutdown(self) -> None:
        """Releases any per-agent resources before shutdown."""
        return None


AgentFactory = Callable[[Context], Agent | Awaitable[Agent]]


class _AgentService:
    """Generated gRPC servicer implementation backed by Python Agent instances."""

    def __init__(
        self, factory: AgentFactory, auth_token: str | None = None, max_agents: int = 128
    ) -> None:
        """Creates a service that instantiates one Python agent per CreateAgent call."""
        self._factory = factory
        self._auth_token = auth_token
        self._max_agents = max_agents
        self._agents: dict[str, Agent] = {}
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
        if request.protocol_version != "parlando-agent-v4":
            await context.abort(
                grpc.StatusCode.FAILED_PRECONDITION,
                "unsupported protocol version",
            )
        if request.role not in {"A", "B"}:
            await context.abort(grpc.StatusCode.INVALID_ARGUMENT, "unknown player role")
        async with self._agent_lock:
            if len(self._agents) + self._creating_agents >= self._max_agents:
                await context.abort(
                    grpc.StatusCode.RESOURCE_EXHAUSTED, "agent capacity reached"
                )
            self._creating_agents += 1
        init_context = Context(
            role=request.role,
            seed=request.seed,
            settings=_struct_to_dict(request.config),
            secrets=SecretValues(_struct_to_dict(request.agent_instance_secrets)),
        )
        try:
            maybe_agent = self._factory(init_context)
            agent = (
                await maybe_agent if inspect.isawaitable(maybe_agent) else maybe_agent
            )
            agent_id = str(uuid.uuid4())
            async with self._agent_lock:
                self._agents[agent_id] = agent
        finally:
            async with self._agent_lock:
                self._creating_agents -= 1
        return self._pb2.CreateAgentResponse(agent_id=agent_id)

    async def Start(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Start request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        await agent.start(_struct_to_dict(request.observation))
        return self._pb2.ObserveResponse()

    async def ObserveTransition(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote ObserveTransition request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        await agent.observe_transition(
            request.actor,
            _struct_to_dict(request.action),
            _struct_to_dict(request.observation),
        )
        return self._pb2.ObserveResponse()

    async def ObserveMessage(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote ObserveMessage request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        await agent.observe_message(
            request.sender, request.text
        )
        return self._pb2.ObserveResponse()

    async def Finish(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Finish request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        await agent.finish(_struct_to_dict(request.completion))
        return self._pb2.ObserveResponse()

    async def Respond(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Respond request from the Rust server."""
        await self._authenticate(context)
        agent = self._agents.get(request.agent_id)
        if agent is None:
            await context.abort(grpc.StatusCode.NOT_FOUND, "unknown agent_id")
        result = await agent.respond(_available_actions(request))
        if result is None:
            return self._pb2.RespondResponse()
        return self._pb2.RespondResponse(response=_response_to_proto(self._pb2, result))

    async def Shutdown(self, request: Any, context: grpc.aio.ServicerContext) -> Any:
        """Handles a remote Shutdown request from the Rust server."""
        await self._authenticate(context)
        async with self._agent_lock:
            agent = self._agents.pop(request.agent_id, None)
        if agent is not None:
            await agent.shutdown()
        return self._pb2.ShutdownResponse()


async def _serve_async(
    factory: type[Agent] | AgentFactory,
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

    def normalized_factory(context: Context) -> Agent | Awaitable[Agent]:
        """Creates an Agent from either a zero-argument class or a context-aware factory."""
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


def serve(
    factory: type[Agent] | AgentFactory,
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
        _serve_async(
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


def _response_to_proto(pb2: Any, response: Response) -> Any:
    """Converts a Response into the generated protobuf response type."""
    converted = pb2.AgentResponse()
    if response._message is not None:
        converted.message = response._message
    if response._action is not None:
        converted.action.CopyFrom(_dict_to_struct(response._action))
    return converted
