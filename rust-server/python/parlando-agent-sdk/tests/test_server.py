"""Unit tests for the provider-neutral Python remote-agent SDK."""

from __future__ import annotations

import asyncio
import dataclasses
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

from google.protobuf.struct_pb2 import Struct

PACKAGE_SRC = Path(__file__).resolve().parents[1] / "src"
if str(PACKAGE_SRC) not in sys.path:
    sys.path.insert(0, str(PACKAGE_SRC))

from parlando_agent_sdk import server  # noqa: E402


class FakeMessage:
    """Records protobuf-like attributes without generated modules."""

    def __init__(self, **values: object) -> None:
        """Stores keyword fields using the generated-message construction shape."""
        self.__dict__.update(values)


class FakeActionField:
    """Implements the CopyFrom method used for response Struct values."""

    def __init__(self) -> None:
        """Creates an empty copied-value slot."""
        self.value: Struct | None = None

    def CopyFrom(self, value: Struct) -> None:
        """Retains the supplied Struct for assertions."""
        self.value = value


class FakeResponseMessage:
    """Imitates the generated response fields touched by conversion code."""

    def __init__(self) -> None:
        """Creates empty optional message and action fields."""
        self.message = ""
        self.action = FakeActionField()


class FakePb2:
    """Minimal generated protobuf module contract used by service tests."""

    Response = FakeResponseMessage
    AgentResponse = FakeResponseMessage
    CreateAgentResponse = FakeMessage
    ObserveResponse = FakeMessage
    RespondResponse = FakeMessage
    ShutdownResponse = FakeMessage


class AbortedRpc(Exception):
    """Signals that a fake gRPC context aborted the current RPC."""


class FakeContext:
    """Captures authentication metadata and abort status from service calls."""

    def __init__(self, metadata: tuple[tuple[str, str], ...] = ()) -> None:
        """Stores invocation metadata in the gRPC tuple shape."""
        self._metadata = metadata
        self.abort_call: tuple[object, str] | None = None

    def invocation_metadata(self) -> tuple[tuple[str, str], ...]:
        """Returns configured request metadata."""
        return self._metadata

    async def abort(self, code: object, detail: str) -> None:
        """Records and raises an abort as real gRPC does not return normally."""
        self.abort_call = (code, detail)
        raise AbortedRpc(detail)


class RecordingAgent(server.Agent):
    """Captures observations and shutdown calls for service delegation tests."""

    def __init__(self) -> None:
        """Creates empty observation storage."""
        self.states: list[dict[str, object]] = []
        self.messages: list[tuple[str, str]] = []
        self.completions: list[dict[str, object]] = []
        self.shutdowns = 0

    async def start(self, observation: dict[str, object]) -> None:
        """Records a state observation."""
        self.states.append(observation)

    async def observe_message(self, sender: server.PlayerRole, text: str) -> None:
        """Records a converted utterance."""
        self.messages.append((sender, text))

    async def finish(self, completion: dict[str, object]) -> None:
        """Records the shared terminal result."""
        self.completions.append(completion)

    async def shutdown(self) -> None:
        """Counts idempotent service shutdown ownership."""
        self.shutdowns += 1


class ConversionTests(unittest.TestCase):
    """Covers immutable responses and protobuf conversion boundaries."""

    def test_agent_response_constructors_and_frozen_state(self) -> None:
        """All convenience constructors preserve content and instances are immutable."""
        action = {"move": {"x": 1}}
        self.assertEqual(server.Response.action(action), server.Response(_action=action))
        self.assertEqual(server.Response.message("hello"), server.Response(_message="hello"))
        self.assertEqual(
            server.Response.action_with_message(action, "hello"),
            server.Response(_action=action, _message="hello"),
        )
        with self.assertRaises(dataclasses.FrozenInstanceError):
            server.Response.message("hello")._message = "changed"  # type: ignore[misc]

    def test_struct_round_trip_preserves_nested_json_values(self) -> None:
        """Struct conversion preserves nested null, Boolean, Unicode, list, and float values."""
        value = {
            "none": None,
            "truth": True,
            "unicode": "Grüße 🌍",
            "nested": {"items": [1.5, "two", False]},
            "empty": {},
        }
        self.assertEqual(server._struct_to_dict(server._dict_to_struct(value)), value)

    def test_available_actions_distinguishes_omitted_empty_and_populated(self) -> None:
        """The optional affordance contract distinguishes missing hints from an empty set."""
        omitted = SimpleNamespace(
            available_actions_provided=False, available_actions=[]
        )
        empty = SimpleNamespace(available_actions_provided=True, available_actions=[])
        populated = SimpleNamespace(
            available_actions_provided=True,
            available_actions=[server._dict_to_struct({"type": "pass"})],
        )
        self.assertIsNone(server._available_actions(omitted))
        self.assertEqual(server._available_actions(empty), [])
        self.assertEqual(server._available_actions(populated), [{"type": "pass"}])

    def test_response_conversion_rejects_empty_and_maps_both_fields(self) -> None:
        """Empty decisions fail while a combined response populates both protobuf fields."""
        with self.assertRaisesRegex(ValueError, "action or message"):
            server.Response()
        converted = server._response_to_proto(
            FakePb2, server.Response.action_with_message({"type": "go"}, "moving")
        )
        self.assertEqual(converted.message, "moving")
        self.assertEqual(server._struct_to_dict(converted.action.value), {"type": "go"})


class ServiceTests(unittest.IsolatedAsyncioTestCase):
    """Covers service authentication, capacity, cleanup, and delegation."""

    def service(self, factory: object, **options: object) -> server._AgentService:
        """Constructs a service while replacing generated protobuf imports."""
        with patch.object(server, "_generated_modules", return_value=(FakePb2, object())):
            return server._AgentService(factory, **options)  # type: ignore[arg-type]

    def create_request(self) -> SimpleNamespace:
        """Builds a complete CreateAgent request with an absent optional seed."""
        return SimpleNamespace(
            protocol_version="parlando-agent-v4",
            agent_name="test-agent",
            agent_version="1.0.0",
            role="B",
            seed=0,
            config=server._dict_to_struct({"difficulty": 2}),
            agent_instance_secrets=server._dict_to_struct({"config.token": "sentinel"}),
        )

    async def test_authentication_accepts_exact_bearer_and_rejects_others(self) -> None:
        """Bearer comparison requires the exact configured authorization value."""
        service = self.service(lambda _: RecordingAgent(), auth_token="secret")
        await service._authenticate(FakeContext((("authorization", "Bearer secret"),)))
        for metadata in [(), (("authorization", "secret"),), (("authorization", "Bearer wrong"),)]:
            with self.assertRaises(AbortedRpc):
                await service._authenticate(FakeContext(metadata))

    async def test_concurrent_creation_never_exceeds_capacity(self) -> None:
        """Reservations count factories still awaiting completion at the exact limit."""
        gate = asyncio.Event()

        async def factory(_context: server.Context) -> RecordingAgent:
            """Waits until both RPC attempts have reached the capacity boundary."""
            await gate.wait()
            return RecordingAgent()

        service = self.service(factory, max_agents=1)
        first = asyncio.create_task(service.CreateAgent(self.create_request(), FakeContext()))
        await asyncio.sleep(0)
        with self.assertRaises(AbortedRpc):
            await service.CreateAgent(self.create_request(), FakeContext())
        gate.set()
        created = await first
        self.assertIn(created.agent_id, service._agents)
        self.assertEqual(len(service._agents), 1)

    async def test_factory_failure_releases_capacity_reservation(self) -> None:
        """A failed factory leaves no ghost agent and permits a later successful retry."""
        calls = 0

        async def factory(_context: server.Context) -> RecordingAgent:
            """Fails once before returning a usable agent."""
            nonlocal calls
            calls += 1
            if calls == 1:
                raise RuntimeError("factory failed")
            return RecordingAgent()

        service = self.service(factory, max_agents=1)
        with self.assertRaisesRegex(RuntimeError, "factory failed"):
            await service.CreateAgent(self.create_request(), FakeContext())
        created = await service.CreateAgent(self.create_request(), FakeContext())
        self.assertIn(created.agent_id, service._agents)

    async def test_factory_receives_separate_redacting_secrets(self) -> None:
        """Agent-instance values stay separate from ordinary settings and diagnostics."""
        contexts: list[server.Context] = []

        def factory(context: server.Context) -> RecordingAgent:
            contexts.append(context)
            return RecordingAgent()

        await self.service(factory).CreateAgent(self.create_request(), FakeContext())
        self.assertEqual(contexts[0].settings, {"difficulty": 2})
        self.assertEqual(contexts[0].secrets.get("config.token"), "sentinel")
        self.assertNotIn("sentinel", repr(contexts[0].secrets))

    async def test_observation_and_double_shutdown_delegate_safely(self) -> None:
        """RPC conversion reaches the agent and repeated shutdown closes it only once."""
        agent = RecordingAgent()
        service = self.service(lambda _: agent)
        created = await service.CreateAgent(self.create_request(), FakeContext())
        await service.Start(
            SimpleNamespace(
                agent_id=created.agent_id,
                observation=server._dict_to_struct({"turn": 3}),
            ),
            FakeContext(),
        )
        await service.ObserveMessage(
            SimpleNamespace(
                agent_id=created.agent_id, sender="A", text="hello"
            ),
            FakeContext(),
        )
        await service.Finish(
            SimpleNamespace(
                agent_id=created.agent_id,
                completion=server._dict_to_struct({"winner": "A", "score": 7}),
            ),
            FakeContext(),
        )
        request = SimpleNamespace(agent_id=created.agent_id)
        await service.Shutdown(request, FakeContext())
        await service.Shutdown(request, FakeContext())
        self.assertEqual(agent.states, [{"turn": 3}])
        self.assertEqual(agent.messages, [("A", "hello")])
        self.assertEqual(agent.completions, [{"winner": "A", "score": 7.0}])
        self.assertEqual(agent.shutdowns, 1)

    async def test_factory_receives_only_agent_owned_context(self) -> None:
        """Initialization excludes room, participant, transport, and presentation details."""
        contexts: list[server.Context] = []

        def factory(context: server.Context) -> RecordingAgent:
            """Records the immutable initialization context."""
            contexts.append(context)
            return RecordingAgent()

        service = self.service(factory)
        await service.CreateAgent(self.create_request(), FakeContext())
        self.assertEqual(
            contexts,
            [
                server.Context(
                    role="B",
                    seed=0,
                    settings={"difficulty": 2.0},
                    secrets=server.SecretValues({"config.token": "sentinel"}),
                )
            ],
        )

    async def test_unknown_agent_ids_abort(self) -> None:
        """Observation RPCs reject unknown capability identifiers."""
        service = self.service(lambda _: RecordingAgent())
        with self.assertRaises(AbortedRpc):
            await service.Start(
                SimpleNamespace(
                    agent_id="missing", observation=Struct()
                ),
                FakeContext(),
            )


class ProtocolDriftTests(unittest.TestCase):
    """Keeps the Python package protocol byte-identical to the Rust server source."""

    def test_bundled_proto_matches_rust_server(self) -> None:
        """The package copy changes only alongside the authoritative server proto."""
        sdk_proto = PACKAGE_SRC / "parlando_agent_sdk/protos/parlando_agent_v3.proto"
        rust_proto = Path(__file__).resolve().parents[3] / "proto/parlando_agent_v3.proto"
        self.assertEqual(sdk_proto.read_bytes(), rust_proto.read_bytes())


if __name__ == "__main__":
    unittest.main()
