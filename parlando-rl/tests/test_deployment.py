"""Exercise a trained decision callable through the ordinary SDK policy path."""

import unittest

from parlando_agent_sdk import Message, Observation, Reason, Transition

from parlando_rl.episode import ParlandoAction, PolicyAgent, PolicyInput


class PolicyAgentTests(unittest.IsolatedAsyncioTestCase):
    """Check the shape and freshness of role-private live decision inputs."""

    async def test_decision_consumes_only_new_events(self):
        """Replay an opening transition and a message, then retry without events."""
        calls: list[PolicyInput] = []

        async def trained_policy(inputs: PolicyInput) -> ParlandoAction:
            """Record the decision input and act from the latest private view."""
            calls.append(inputs)
            return ParlandoAction(action={"choice": inputs.view["cue"]}, message="ready")

        agent = PolicyAgent(trained_policy)
        opening = {"cue": "unknown"}
        view = {"cue": "right"}
        transition = Observation(transition=Transition("B", {"give_cue": True}, view))
        message = Observation(message=Message("B", "hello"))
        await agent.start(opening)
        await agent.observe(transition)
        await agent.observe(message)
        first = Reason("after_observation")
        decision = await agent.decide(first)
        self.assertEqual(decision.action, {"choice": "right"})
        self.assertEqual(decision.message, "ready")
        self.assertEqual(calls, [PolicyInput(view, first, (transition, message), opening)])

        rejection = Reason("action_rejected", "retry")
        await agent.decide(rejection)
        self.assertEqual(calls[1], PolicyInput(view, rejection, (), None))

    async def test_sync_policy_can_yield(self):
        """A synchronous empty action yields through the normal SDK decision."""
        agent = PolicyAgent(lambda inputs: ParlandoAction())
        await agent.start({})
        decision = await agent.decide(Reason("session_started"))
        self.assertIsNone(decision.action)
        self.assertIsNone(decision.message)
