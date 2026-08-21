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
from parlando_agent_sdk.generated import parlando_agent_v3_pb2, parlando_rl_v1_pb2  # noqa: E402


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
        self.transitions: list[tuple[str, dict[str, object], dict[str, object]]] = []
        self.completions: list[dict[str, object]] = []
        self.shutdowns = 0

    async def start(self, observation: dict[str, object]) -> None:
        """Records a state observation."""
        self.states.append(observation)

    async def observe_message(self, sender: server.PlayerRole, text: str) -> None:
        """Records a converted utterance."""
        self.messages.append((sender, text))

    async def observe_transition(
        self,
        actor: server.PlayerRole,
        action: dict[str, object],
        observation: dict[str, object],
    ) -> None:
        """Records a role-safe accepted transition."""
        self.transitions.append((actor, action, observation))

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

    def test_optional_checkpoint_distinguishes_absent_and_present_values(self) -> None:
        """Generated optional fields and lightweight doubles preserve checkpoint absence."""
        self.assertIsNone(server._optional_checkpoint(SimpleNamespace()))
        absent = SimpleNamespace(checkpoint_id="ignored", HasField=lambda _name: False)
        present = SimpleNamespace(checkpoint_id="checkpoint-7", HasField=lambda _name: True)
        self.assertIsNone(server._optional_checkpoint(absent))
        self.assertEqual(server._optional_checkpoint(present), "checkpoint-7")

    def test_session_logger_enforces_utf8_entry_and_shared_session_limits(self) -> None:
        """Python logging mirrors Rust byte accounting at both exact boundaries."""
        logger = server.SessionLogger()
        exact = "x" * (logger._MAX_ENTRY_BYTES - 4) + "🌳"
        logger.log(exact)
        with self.assertRaisesRegex(ValueError, "entry exceeds"):
            logger.log("x" * (logger._MAX_ENTRY_BYTES - 3) + "🌳")
        for _ in range(logger._MAX_SESSION_BYTES // logger._MAX_ENTRY_BYTES - 1):
            logger.log("x" * logger._MAX_ENTRY_BYTES)
        with self.assertRaisesRegex(ValueError, "session log byte limit"):
            logger.log("one more byte")
        with self.assertRaisesRegex(TypeError, "must be strings"):
            logger.log(7)  # type: ignore[arg-type]


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

    async def test_creation_rejects_protocol_and_role_before_factory_invocation(self) -> None:
        """Protocol and role validation fail closed without reserving agent capacity."""
        factory = AsyncMock(return_value=RecordingAgent())
        service = self.service(factory)
        request = self.create_request()
        request.protocol_version = "parlando-agent-v999"
        with self.assertRaises(AbortedRpc):
            await service.CreateAgent(request, FakeContext())
        request.protocol_version = "parlando-agent-v4"
        request.role = "spectator"
        with self.assertRaises(AbortedRpc):
            await service.CreateAgent(request, FakeContext())
        factory.assert_not_awaited()
        self.assertEqual(service._creating_agents, 0)

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

    async def test_session_logger_drains_arbitrary_text_into_rpc_responses(self) -> None:
        """Constructor and callback logs leave the process on the next matching response."""
        logger: server.SessionLogger | None = None

        def factory(context: server.Context) -> RecordingAgent:
            """Records one constructor log through the injected session capability."""
            nonlocal logger
            logger = context.logger
            logger.log("constructor: 🌳\n{not-json}")
            return RecordingAgent()

        service = self.service(factory)
        created = await service.CreateAgent(self.create_request(), FakeContext())
        self.assertEqual(created.session_logs, ["constructor: 🌳\n{not-json}"])
        assert logger is not None
        logger.log("between callbacks")
        observed = await service.Start(
            SimpleNamespace(
                agent_id=created.agent_id,
                observation=server._dict_to_struct({"turn": 1}),
            ),
            FakeContext(),
        )
        self.assertEqual(observed.session_logs, ["between callbacks"])

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
        await service.ObserveTransition(
            SimpleNamespace(
                agent_id=created.agent_id,
                actor="B",
                action=server._dict_to_struct({"type": "move"}),
                observation=server._dict_to_struct({"turn": 4}),
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
        self.assertEqual(agent.transitions, [("B", {"type": "move"}, {"turn": 4.0})])
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
        self.assertEqual(len(contexts), 1)
        self.assertEqual(contexts[0].role, "B")
        self.assertEqual(contexts[0].seed, 0)
        self.assertEqual(contexts[0].settings, {"difficulty": 2.0})
        self.assertEqual(contexts[0].secrets.get("config.token"), "sentinel")
        self.assertIsInstance(contexts[0].logger, server.SessionLogger)

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

    async def test_respond_preserves_none_and_combined_response_semantics(self) -> None:
        """Respond maps yielding and combined message/action decisions without ambiguity."""

        class RespondingAgent(RecordingAgent):
            """Returns scripted decisions in service order."""

            def __init__(self) -> None:
                """Creates one yield followed by one combined response."""
                super().__init__()
                self.responses = [None, server.Response.action_with_message({"type": "pass"}, "passing")]

            async def respond(self, available_actions: object) -> server.Response | None:
                """Checks optional action conversion and returns the next script item."""
                self.asserted_actions = available_actions
                return self.responses.pop(0)

        agent = RespondingAgent()
        service = self.service(lambda _: agent)
        created = await service.CreateAgent(self.create_request(), FakeContext())
        request = SimpleNamespace(
            agent_id=created.agent_id,
            available_actions_provided=True,
            available_actions=[server._dict_to_struct({"type": "pass"})],
        )
        yielded = await service.Respond(request, FakeContext())
        self.assertFalse(hasattr(yielded, "response"))
        combined = await service.Respond(request, FakeContext())
        self.assertEqual(agent.asserted_actions, [{"type": "pass"}])
        self.assertEqual(combined.response.message, "passing")
        self.assertEqual(server._struct_to_dict(combined.response.action.value), {"type": "pass"})


class ServerValidationTests(unittest.IsolatedAsyncioTestCase):
    """Exercises public server bounds before network startup."""

    async def test_serve_rejects_capacity_port_tls_and_remote_cleartext(self) -> None:
        """Invalid listener policies fail deterministically without starting gRPC."""
        with self.assertRaisesRegex(ValueError, "max_agents"):
            await server._serve_async(RecordingAgent, max_agents=0)
        for port in [-1, 65_536]:
            with self.assertRaisesRegex(ValueError, "port"):
                await server._serve_async(RecordingAgent, port=port)

        fake_server = SimpleNamespace(add_insecure_port=lambda _address: None, add_secure_port=lambda _address, _credentials: None)
        with patch.object(server.grpc.aio, "server", return_value=fake_server), patch.object(
            server, "add_agent_service"
        ):
            with self.assertRaisesRegex(ValueError, "configured together"):
                await server._serve_async(RecordingAgent, certificate_chain=b"certificate")
            with self.assertRaisesRegex(ValueError, "require TLS"):
                await server._serve_async(RecordingAgent, host="agent.example")


class ProtocolSourceTests(unittest.TestCase):
    """Checks that SDK development uses the repository's shared protocol source."""

    def test_shared_agent_proto_exists(self) -> None:
        """The repository-level agent protocol remains available to the generator."""
        shared_proto = Path(__file__).resolve().parents[2] / "proto/parlando_agent_v3.proto"
        self.assertTrue(shared_proto.is_file())

    def test_generated_agent_descriptor_matches_public_contract(self) -> None:
        """Generated agent bindings retain package, methods, fields, and optional presence."""
        descriptor = parlando_agent_v3_pb2.DESCRIPTOR
        self.assertEqual(descriptor.package, "parlando.agent.v4")
        self.assertEqual(
            list(descriptor.services_by_name["AgentService"].methods_by_name),
            ["CreateAgent", "Start", "ObserveTransition", "ObserveMessage", "Finish", "Respond", "Shutdown"],
        )
        request = descriptor.message_types_by_name["CreateAgentRequest"]
        self.assertEqual(
            [(field.name, field.number) for field in request.fields],
            [
                ("protocol_version", 1), ("agent_name", 2), ("agent_version", 3), ("role", 4),
                ("seed", 5), ("config", 6), ("agent_instance_secrets", 7), ("checkpoint_id", 8),
            ],
        )
        self.assertTrue(request.fields_by_name["checkpoint_id"].has_presence)

    def test_generated_learner_descriptor_matches_public_contract(self) -> None:
        """Generated learner bindings retain stable RPC and trajectory field numbers."""
        descriptor = parlando_rl_v1_pb2.DESCRIPTOR
        self.assertEqual(descriptor.package, "parlando.rl.v1")
        self.assertEqual(list(descriptor.services_by_name["LearnerService"].methods_by_name), ["Train"])
        trajectory = descriptor.message_types_by_name["TrajectoryStep"]
        self.assertEqual(
            [(field.name, field.number) for field in trajectory.fields],
            [
                ("run_id", 1), ("plan_id", 2), ("decision", 3), ("scenario", 4), ("role", 5),
                ("agent", 6), ("checkpoint_id", 7), ("reward", 8), ("reward_version", 9),
                ("observation", 10), ("available_actions", 11), ("action", 12), ("accepted", 13),
                ("rejection", 14), ("rewards", 15), ("next_observation", 16), ("terminal", 17),
            ],
        )


if __name__ == "__main__":
    unittest.main()
