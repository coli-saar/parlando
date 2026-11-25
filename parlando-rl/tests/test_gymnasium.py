"""Check Gymnasium's decision contract and both SDK policy bridges."""

from pathlib import Path
import unittest

try:
    from gymnasium import spaces
    from gymnasium.error import ResetNeeded
    from gymnasium.utils.env_checker import check_env
except ModuleNotFoundError as error:
    if error.name != "gymnasium":
        raise
    raise unittest.SkipTest("requires the Gymnasium extra") from error

import numpy as np
from parlando_agent_sdk import Agent, Decision, Message, Observation, Reason, Transition

from parlando_rl import AgentPolicy, ParlandoAction, ParlandoEpisode, ParlandoObservation, SessionInfo
from parlando_rl.gymnasium import GymAgentPolicy, GymPolicyAgent, ParlandoGymEnv

DIST = Path(__file__).resolve().parents[2] / "experiments" / "cue-choice-rl" / "dist"
GAME = DIST / "cue-choice-0.1.0.zip"
DEALER = DIST / "agent-cue_choice.dealer-1.0.0.zip"


def number(frame):
    """Encode an integer private-view field."""
    return frame.view["number"]


def number_with_chat(frame):
    """Encode incoming chat or a rejection distinctly even if the view stays fixed."""
    if frame.reason.kind == "action_rejected":
        return 0
    if any(event.message for event in frame.events):
        return 1
    return number(frame)


def number_action(action, view):
    """Use the numeric action along with the view at this decision."""
    return ParlandoAction(action={"number": int(action), "previous": view["number"]})


class StubEpisode:
    """Record decisions and emit deterministic learner frames with configurable status."""

    def __init__(self, status=None, reset_status=None):
        """Set the first step's status and optional early reset stopping status."""
        self.status = status
        self.reset_status = reset_status
        self.resets = []
        self.decisions = []
        self.closed = False

    def reset(self, *, seed, config, episode_id):
        """Record Parlando reset inputs and expose the initial decision."""
        self.resets.append((seed, config, episode_id))
        return ParlandoObservation(
            initial_view={"number": 0},
            view={"number": 1},
            events=[Observation(transition=Transition("B", {}, {"number": 1}))],
            reason=Reason("scheduled"),
            done=self.reset_status is not None,
            reward=0.25,
            metadata={"status": self.reset_status, "counters": {"decisions": 0}},
        )

    def step(self, decision):
        """Record the decoded decision and expose its next view and reward."""
        self.decisions.append(decision)
        metadata = {"status": self.status, "counters": {"decisions": 1}}
        if self.status == "completed":
            metadata["completion"] = {"score": 1}
        return ParlandoObservation(
            view={"number": decision.action["number"]},
            events=[Observation(message=Message("B", "hello"))],
            reason=Reason("scheduled"),
            done=self.status is not None,
            reward=0.75,
            metadata=metadata,
        )

    def close(self):
        """Record closure of the episode owned by the environment."""
        self.closed = True


def make_env(episode):
    """Wrap the test episode with scalar spaces, codecs, and a scenario sampler."""
    return ParlandoGymEnv(
        episode,
        observation_space=spaces.Discrete(3),
        action_space=spaces.Discrete(3),
        encode_observation=number,
        decode_action=number_action,
        sample_config=lambda rng: {"sample": int(rng.integers(3))},
    )


class RecordingAgent(Agent):
    """Use the view to decide and record callback ordering for an SDK trajectory."""

    def __init__(self, callbacks):
        """Record observations in a list shared with the test."""
        self.callbacks = callbacks
        self.view = None

    async def start(self, view):
        """Record the initial private view before events."""
        self.view = view
        self.callbacks.append(("start", view))

    async def observe(self, observation):
        """Record transitions and messages in delivery order."""
        if observation.transition:
            self.view = observation.transition.view
            self.callbacks.append(("transition", self.view))
        if observation.message:
            self.callbacks.append(("message", observation.message.text))

    async def decide(self, reason):
        """Use the accumulated view to choose a numeric action."""
        self.callbacks.append(("decide", reason.kind))
        return Decision(action={"number": self.view["number"] + 1})

    async def finish(self, completion):
        """Receive the shared game completion."""
        self.callbacks.append(("finish", completion))

    async def shutdown(self):
        """Record SDK cleanup."""
        self.callbacks.append(("shutdown",))


class GymnasiumTests(unittest.TestCase):
    """Exercise the trainer API and the SDK-to-Gymnasium direction."""

    def test_gymnasium_api_and_seeded_resets(self):
        """Pass Gymnasium's checker and keep reset streams reproducible."""
        episode = StubEpisode()
        with make_env(episode) as env:
            check_env(env, skip_render_check=True, skip_close_check=True)
            env.reset(seed=15)
            env.reset()
            expected = episode.resets[-2:]
            env.reset(seed=15)
            env.reset()
            self.assertEqual(episode.resets[-2:], expected)
            self.assertEqual(expected[0][0], 15)
            env.reset(seed=16, options={"config": "level", "episode_id": "sample"})
            self.assertEqual(episode.resets[-1], (16, "level", "sample"))
        self.assertTrue(episode.closed)

    def test_completion_and_limits(self):
        """Map game completion and runtime limits to distinct Gymnasium flags."""
        for status, expected in (("completed", (True, False)), ("deadline", (False, True))):
            with self.subTest(status=status):
                episode = StubEpisode(status)
                with make_env(episode) as env:
                    with self.assertRaises(ResetNeeded):
                        env.step(0)
                    initial, info = env.reset(seed=19)
                    self.assertEqual(initial, 1)
                    self.assertEqual(info["reward"], 0.25)
                    self.assertIsNone(info["status"])
                    info["counters"]["decisions"] = 99
                    self.assertEqual(env.parlando_observation.metadata["counters"]["decisions"], 0)
                    with self.assertRaisesRegex(ValueError, "outside Discrete"):
                        env.step(3)
                    self.assertEqual(episode.decisions, [])
                    result = env.step(np.array(2))
                    self.assertEqual(result[:4], (2, 0.75, *expected))
                    self.assertEqual(result[4]["status"], status)
                    self.assertEqual(episode.decisions[-1].action, {"number": 2, "previous": 1})
                    with self.assertRaises(ResetNeeded):
                        env.step(0)

    def test_episode_that_ends_on_reset_has_no_gym_decision(self):
        """Raise with the native status when the learner never gets a turn."""
        episode = StubEpisode(reset_status="quiescent")
        with make_env(episode) as env:
            with self.assertRaisesRegex(ResetNeeded, "quiescent, opening reward 0.25"):
                env.reset(seed=19)
            self.assertTrue(episode.closed)
            with self.assertRaises(ResetNeeded):
                env.step(0)

    def test_reset_closes_episode_on_encoder_failure(self):
        """Close the native session if the encoder cannot produce its initial observation."""
        episode = StubEpisode()
        with ParlandoGymEnv(
            episode,
            observation_space=spaces.Discrete(3),
            action_space=spaces.Discrete(3),
            encode_observation=lambda frame: frame.view["missing"],
            decode_action=number_action,
        ) as env:
            with self.assertRaises(KeyError):
                env.reset()
            self.assertTrue(episode.closed)

    def test_encoder_distinguishes_messages_without_a_new_view(self):
        """Provide the full role-private frame to the numeric encoder."""
        with ParlandoGymEnv(
            StubEpisode(),
            observation_space=spaces.Discrete(3),
            action_space=spaces.Discrete(3),
            encode_observation=lambda frame: int(any(event.message for event in frame.events)),
            decode_action=number_action,
        ) as env:
            self.assertEqual(env.reset()[0], 0)
            self.assertEqual(env.parlando_observation.view, {"number": 1})
            self.assertEqual(env.step(1)[0], 1)
            self.assertEqual(env.parlando_observation.view, {"number": 1})

    def test_sdk_agent_selects_gym_actions_from_callbacks(self):
        """Replay the opening and terminal events around the encoded action."""
        callbacks = []
        policy = AgentPolicy(lambda _context: RecordingAgent(callbacks), SessionInfo("A", 19))
        with make_env(StubEpisode("completed")) as env, GymAgentPolicy(
            policy, encode_action=lambda decision, _view: int(decision.action["number"])
        ) as gym_policy:
            env.reset(seed=19)
            frame = env.parlando_observation
            gym_policy.reset(frame)
            self.assertEqual(env.step(gym_policy(frame))[:4], (2, 0.75, True, False))
            gym_policy.finish(env.parlando_observation)
        self.assertEqual(callbacks, [
            ("start", {"number": 0}), ("transition", {"number": 1}), ("decide", "scheduled"),
            ("message", "hello"), ("finish", {"score": 1}), ("shutdown",),
        ])

    @unittest.skipUnless(GAME.is_file() and DEALER.is_file(), "build experiments/cue-choice-rl first")
    def test_real_cue_choice_decision_reward(self):
        """Exercise the native WASM dealer, learner view, reward, and game completion."""
        episode = ParlandoEpisode(
            game=GAME,
            opponent={"factory": "wasm", "path": DEALER},
            schedule={"kind": "alternate_after_action", "first": "player_b"},
            reward=lambda frame: float(bool(frame.completion and frame.completion["correct"])),
        )
        with ParlandoGymEnv(
            episode,
            observation_space=spaces.Discrete(2),
            action_space=spaces.Discrete(4),
            encode_observation=lambda frame: int(frame.view.get("cue") == "dax"),
            decode_action=lambda action, view: ParlandoAction(action=view["available_actions"][action]),
        ) as env:
            initial, _ = env.reset(seed=7, options={"config": {"level": "dax"}})
            self.assertEqual(initial, 1)
            actions = env.parlando_observation.view["available_actions"]
            index = next(index for index, action in enumerate(actions) if action["choice"] == "oak")
            final, reward, terminated, truncated, info = env.step(index)
            self.assertEqual((final, reward, terminated, truncated), (0, 1.0, True, False))
            self.assertTrue(info["completion"]["correct"])


class StubModel:
    """Mimic a Discrete SB3 model including recurrent-prediction arguments."""

    def __init__(self):
        """Record the observation, previous state, and episode-start flag."""
        self.records = []

    def predict(self, observation, *, state, episode_start, deterministic):
        """Choose the view's number and advance a simple recurrent state."""
        self.records.append((observation, state, episode_start.tolist(), deterministic))
        return np.array(observation), len(self.records)


class GymPolicyAgentTests(unittest.IsolatedAsyncioTestCase):
    """Exercise a trained model through its SDK lifecycle."""

    async def test_model_gets_latest_view_and_session_local_state(self):
        """Yield before the model is needed, retain view across chat, and reset state."""
        model = StubModel()
        agent = GymPolicyAgent(
            model,
            action_space=spaces.Discrete(3),
            encode_observation=number_with_chat,
            decode_action=number_action,
            should_act=lambda view, reason: "number" in view and reason.kind != "wakeup_due",
        )
        await agent.start({})
        self.assertEqual(await agent.decide(Reason("session_started")), Decision())
        await agent.observe(Observation(transition=Transition("B", {}, {"number": 2})))
        self.assertEqual((await agent.decide(Reason("scheduled"))).action, {"number": 2, "previous": 2})
        await agent.observe(Observation(message=Message("B", "still the same view")))
        self.assertEqual(
            (await agent.decide(Reason("after_observation"))).action,
            {"number": 1, "previous": 2},
        )
        self.assertEqual(
            (await agent.decide(Reason("action_rejected", "wrong_choice"))).action,
            {"number": 0, "previous": 2},
        )
        await agent.start({"number": 1})
        await agent.decide(Reason("session_started"))
        self.assertEqual(model.records, [
            (2, None, [True], True), (1, 1, [False], True), (0, 2, [False], True),
            (1, None, [True], True),
        ])
