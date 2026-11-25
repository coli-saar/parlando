"""Check learner callback ownership for copied observations and headless wakeups."""

import unittest
from importlib.util import find_spec

if find_spec("openenv") is None:
    raise unittest.SkipTest("requires the OpenEnv extra")

from parlando_agent_sdk import Agent, Decision, Message, Observation, Reason, WakeupRequest
from parlando_rl.openenv import AgentPolicy, HeadlessWakeupError, ParlandoObservation, SessionInfo


class PolicyTests(unittest.IsolatedAsyncioTestCase):
    """Trainers may copy an opening observation before handing it to the policy."""

    async def test_copied_opening_is_delivered_once_and_wakeup_is_rejected(self):
        """Preserve callback order and session logs even when a copied frame requests a timer."""
        calls = []

        class Learner(Agent):
            """Record the callback sequence and request a timer on the first decision."""

            async def observe(self, event):
                """Record the opening delivery."""
                calls.append(event.message.text)

            async def decide(self, reason):
                """Exercise the headless rule even for a timer cancellation."""
                calls.append(reason.kind)
                return Decision(wakeup=WakeupRequest.cancel())

        def factory(context):
            """Write one session log during construction."""
            context.logger.log("starting")
            return Learner()

        policy = AgentPolicy(factory, SessionInfo("A", 5))
        self.addAsyncCleanup(policy.close)
        opening = ParlandoObservation(
            initial_view={}, events=[Observation(message=Message("B", "ready"))],
            view={}, reason=Reason("scheduled"),
        )
        await policy.reset(opening)
        with self.assertRaises(HeadlessWakeupError):
            await policy.act(opening.model_copy(deep=True))
        self.assertEqual(calls, ["ready", "scheduled"])
        self.assertEqual(opening.metadata["logs"], ["starting"])
