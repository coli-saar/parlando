"""Check native activation ordering and shutdown across the Python boundary."""

import asyncio
import json
import unittest
from pathlib import Path

import grpc
from parlando_agent_sdk import Agent, Decision, add_agent_service
from parlando_rl import _native


DIST = Path(__file__).resolve().parents[2] / "experiments/cue-choice-rl/dist"
GAME = DIST / "cue-choice-0.1.0.zip"
DEALER = DIST / "agent-cue_choice.dealer-1.0.0.zip"


async def close_session(session):
    """Await the native cleanup future from unittest's coroutine-based cleanup."""
    await session.close()


@unittest.skipUnless(GAME.is_file() and DEALER.is_file(), "build experiments/cue-choice-rl first")
class NativeSessionTests(unittest.IsolatedAsyncioTestCase):
    """Exercise activation and cleanup that depend on the native scheduler."""

    async def test_live_delivery_replaces_pending_start_reason(self):
        """A delivery before the learner's first decision takes activation precedence."""
        engine = _native.Engine(
            str(GAME), json.dumps({"factory": "wasm", "path": str(DEALER)})
        )
        session = engine.session('{"level":"dax"}', 5, "A", "live", "{}", "B")
        self.addAsyncCleanup(close_session, session)
        initial = json.loads(await session.reset())
        self.assertEqual(initial["reason"], "after_observation")
        self.assertEqual(initial["events"][0]["transition"]["actor"], "B")
        self.assertEqual(initial["view"]["cue"], "dax")

        rejected = json.loads(await session.step('{"action":{"type":"deal"},"message":"discarded"}'))
        self.assertEqual(rejected["reason"], "action_rejected")
        self.assertEqual(rejected["rejection_code"], "wrong_role")
        self.assertFalse(rejected["events"])
        self.assertFalse(rejected["transitions"])

    async def test_cancelled_reset_shuts_down_remote_opponent(self):
        """Release a gRPC agent when its Python reset waiter is cancelled."""
        started, release, stopped = asyncio.Event(), asyncio.Event(), asyncio.Event()

        class WaitingDealer(Agent):
            """Wait inside the opening callback so the native operation is in flight."""

            async def decide(self, reason):
                """Record activation, then wait for the test to cancel the waiter."""
                started.set()
                await release.wait()
                return Decision(action={"type": "deal"})

            async def shutdown(self):
                """Record the actual remote shutdown callback."""
                stopped.set()

        server = grpc.aio.server()
        add_agent_service(server, WaitingDealer)
        port = server.add_insecure_port("127.0.0.1:0")
        await server.start()
        self.addAsyncCleanup(server.stop, 0)
        engine = _native.Engine(str(GAME), json.dumps({
            "factory": "remote_grpc",
            "endpoint": f"http://127.0.0.1:{port}",
            "auth_token": "",
        }))
        session = engine.session('{"level":"dax"}', 5, "A", "live", "{}", "B")
        self.addAsyncCleanup(close_session, session)
        pending = asyncio.ensure_future(session.reset())
        try:
            await asyncio.wait_for(started.wait(), 10)
            pending.cancel()
            with self.assertRaises(asyncio.CancelledError):
                await pending
        finally:
            release.set()
        await asyncio.wait_for(stopped.wait(), 10)
