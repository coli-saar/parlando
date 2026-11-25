"""Exercise concurrent learners and role-private observations across real transports."""

import asyncio
import json
import socket
import tempfile
import unittest
from importlib.util import find_spec
from pathlib import Path

if find_spec("openenv") is None:
    raise unittest.SkipTest("requires the OpenEnv extra")

import uvicorn
from fastapi.testclient import TestClient
from parlando_agent_sdk import Agent, Decision

from parlando_rl.openenv import ParlandoAction, ParlandoClient, ParlandoEnvironment
from parlando_rl.openenv.server import create_server

DIST = Path(__file__).resolve().parents[2] / "experiments/cue-choice-rl/dist"
GAME = DIST / "cue-choice-0.1.0.zip"
DEALER = DIST / "agent-cue_choice.dealer-1.0.0.zip"


class ErrorPrivacyTests(unittest.TestCase):
    """Verify that diagnostics from failed server operations stay server-side."""

    def test_missing_package_path_is_redacted(self):
        """Return a generic reset error while logging the missing local path."""
        with tempfile.TemporaryDirectory() as directory:
            game = Path(directory) / "private-game-package.zip"
            self._assert_redacted(
                create_server(game=game, opponent={"factory": "wasm", "path": DEALER}),
                {"type": "reset", "data": {"seed": 1}},
                "Parlando episode reset failed",
                str(game),
            )

    @unittest.skipUnless(GAME.is_file() and DEALER.is_file(), "build experiments/cue-choice-rl first")
    def test_reward_exception_is_redacted(self):
        """Keep the private opponent view out of an error returned during an episode."""
        def reward(context):
            """Fail on the learner's choice with a private value in the message."""
            if context.actor == "A":
                raise ValueError(f"reward failed with {context.before['B']['view']}")
            return 0

        app = create_server(
            game=GAME,
            opponent={"factory": "wasm", "path": DEALER},
            schedule={"kind": "alternate_after_action", "first": "B"},
            reward=reward,
        )
        self._assert_redacted(
            app,
            {"type": "step", "data": {"action": {"type": "choose", "choice": "oak"}}},
            "Parlando episode step failed",
            "dealer_waiting",
            reset=True,
        )

    def _assert_redacted(self, app, request, public, private, *, reset=False):
        """Compare the WebSocket error against the server's diagnostic log."""
        with TestClient(app) as client, client.websocket_connect("/ws") as session:
            if reset:
                session.send_json({"type": "reset", "data": {"seed": 1, "config": {"level": "dax"}}})
                self.assertEqual(session.receive_json()["type"], "observation")
            with self.assertLogs("parlando_rl.openenv.server", "ERROR") as logs:
                session.send_json(request)
                response = session.receive_json()
            self.assertEqual(response["type"], "error")
            self.assertEqual(response["data"]["message"], public)
            self.assertNotIn(private, json.dumps(response))
            self.assertIn(private, "\n".join(logs.output))
            session.send_json({"type": "close"})


@unittest.skipUnless(GAME.is_file() and DEALER.is_file(), "build experiments/cue-choice-rl first")
class SessionTests(unittest.IsolatedAsyncioTestCase):
    """Keep separate episodes alive while a slow SDK opponent is in a callback."""

    async def test_websocket_sessions_keep_independent_state(self):
        """Interleave two WebSocket rollouts and inspect their private frames and rewards."""
        app = create_server(
            game=GAME, opponent={"factory": "wasm", "path": DEALER},
            schedule={"kind": "alternate_after_action", "first": "player_b"},
        )
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            host, port = listener.getsockname()
            server = uvicorn.Server(uvicorn.Config(app, log_level="error"))
            serve = asyncio.create_task(server.serve(sockets=[listener]))
            try:
                async def wait_for_server():
                    """Wait until the server is accepting connections or fails startup."""
                    while not server.started:
                        if serve.done():
                            await serve
                        await asyncio.sleep(0.02)
                await asyncio.wait_for(wait_for_server(), 15)
                url = f"http://{host}:{port}"
                async with ParlandoClient(base_url=url) as first, ParlandoClient(base_url=url) as second:
                    a, b = await asyncio.gather(
                        first.reset(seed=1, config={"level": "dax"}, episode_id="first"),
                        second.reset(seed=2, config={"level": "wug"}, episode_id="second"),
                    )
                    self.assertEqual((a.observation.view["cue"], b.observation.view["cue"]), ("dax", "wug"))
                    self.assertEqual(a.observation.events[0].transition.actor, "B")
                    self.assertNotIn("dealer_waiting", json.dumps(a.observation.model_dump()))
                    done = await first.step(ParlandoAction(action={"type": "choose", "choice": "oak"}))
                    self.assertTrue(done.done)
                    self.assertEqual(done.reward, 1.0)
                    self.assertTrue(done.observation.metadata["completion"]["correct"])
                    self.assertEqual((await second.state()).step_count, 0)
                    self.assertEqual((await first.state()).episode_id, "first")
            finally:
                server.should_exit = True
                await serve

    async def test_python_opponent_can_wait_while_another_session_finishes(self):
        """Hold one remote callback open while another game advances to the action limit."""
        entered, release, stopped = asyncio.Event(), asyncio.Event(), asyncio.Event()
        seeds = []

        class WaitingDealer(Agent):
            """Attach a message on dealing and track the actual context and shutdown."""

            async def decide(self, reason):
                """Let the test release this callback after playing another episode."""
                entered.set()
                await release.wait()
                return Decision(action={"type": "deal"}, message="Your turn")

            async def shutdown(self):
                """Record release of the SDK instance when the rollout is closed."""
                stopped.set()

        def factory(context):
            """Capture the seed and produce one independent opponent instance."""
            context.logger.log("private dealer log")
            seeds.append(context.seed)
            return WaitingDealer()

        rewards = []
        def reward(context):
            """Record both server-side role views without changing the learner payload."""
            rewards.append(context)
            return 2.0

        env = ParlandoEnvironment(game=GAME, opponent={"factory": "python", "source": factory},
                                  schedule="live", reward=reward)
        other = ParlandoEnvironment(
            game=GAME, opponent={"factory": "wasm", "path": DEALER},
            schedule={"kind": "alternate_after_action", "first": "player_b"},
            limits={"accepted_actions": 1},
        )
        self.addAsyncCleanup(other.close_async)
        self.addAsyncCleanup(env.close_async)
        first = await env.reset_async(seed=11, config={"level": "dax"})
        self.assertEqual(first.reason.kind, "session_started")
        pending = asyncio.create_task(env.step_async(ParlandoAction()))
        try:
            await asyncio.wait_for(entered.wait(), 60)
            limited = await asyncio.wait_for(other.reset_async(seed=1, config={"level": "wug"}), 60)
            self.assertTrue(limited.done)
            self.assertEqual(limited.metadata["status"], "action_limit")
        finally:
            release.set()
            after = await pending
        self.assertEqual(after.reason.kind, "after_observation")
        self.assertEqual(after.events[0].message.text, "Your turn")
        self.assertEqual(after.events[0].transition.actor, "B")
        self.assertEqual(after.reward, 2.0)
        self.assertEqual(rewards[0].after["B"]["view"], "dealer_waiting")
        self.assertNotIn("dealer_waiting", json.dumps(after.model_dump()))
        self.assertNotIn("private dealer log", json.dumps(first.model_dump()))
        self.assertEqual(env.opponent_logs, ("private dealer log",))
        self.assertEqual(seeds, [11])
        await env.close_async()
        self.assertTrue(stopped.is_set())
