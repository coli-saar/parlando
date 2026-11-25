"""Exercise the real cue-choice components, SDK gRPC service, and Parlando runner."""

import asyncio
import json
import os
import shutil
import sys
import tempfile
import unittest
from importlib.util import find_spec
from pathlib import Path

if find_spec("openenv") is None:
    raise unittest.SkipTest("requires the OpenEnv extra")

import grpc
import yaml
from parlando_agent_sdk import Decision, Reason, add_agent_service

from parlando_rl.openenv import AgentPolicy, ParlandoEnvironment, SessionInfo

EXAMPLE = Path(__file__).resolve().parents[1] / "examples" / "cue_choice"
sys.path.insert(0, str(EXAMPLE))
from agent import CueAgent
from train import correctness, train

DIST = EXAMPLE.parents[2] / "experiments" / "cue-choice-rl" / "dist"
GAME = DIST / "cue-choice-0.1.0.zip"
DEALER = DIST / "agent-cue_choice.dealer-1.0.0.zip"
CLI = os.environ.get("PARLANDO_BIN", shutil.which("parlando"))


class RecordingCueAgent(CueAgent):
    """Exercise messages and rejections before making the same learned choice."""

    def __init__(self, context, trace):
        """Share a test trace across instances, keyed by the runner's seed."""
        super().__init__(context)
        self._trace = trace.setdefault(context.seed, [])
        self._decisions = 0

    async def start(self, view):
        """Record the initial private view."""
        self._trace.append(("start", view))
        await super().start(view)

    async def observe(self, observation):
        """Record accepted transitions and incoming chat, including their ordering."""
        self._trace.append(("observe", observation))
        await super().observe(observation)

    async def decide(self, reason):
        """Send one message and one illegal action before consulting the table."""
        self._decisions += 1
        if self._decisions == 1:
            decision = Decision(message="I have the cue")
        elif self._decisions == 2:
            decision = Decision(action={"type": "deal"}, message="Undelivered")
        else:
            decision = await super().decide(reason)
        self._trace.append(("decide", reason, decision))
        return decision

    async def finish(self, completion):
        """Record the terminal payload before shutdown."""
        self._trace.append(("finish", completion))

    async def shutdown(self):
        """Record teardown after the terminal callback."""
        self._trace.append(("shutdown",))


@unittest.skipUnless(GAME.is_file() and DEALER.is_file(), "build experiments/cue-choice-rl first")
class CueChoiceIntegration(unittest.IsolatedAsyncioTestCase):
    """Run packaged fixtures through the real transport and engine."""

    async def test_train_and_match_headless_callbacks(self):
        """Train a table and compare the same agent's whole callback trace over gRPC."""
        if CLI is None:
            self.skipTest("install parlando or set PARLANDO_BIN")

        table, estimate = await train(GAME, DEALER, 1000)
        self.assertEqual(estimate, 1.0)
        self.assertEqual(table, {"dax": "oak", "wug": "pine", "kiki": "birch", "zorp": "elm"})

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            table_file = root / "table.json"
            table_file.write_text(json.dumps(table), encoding="utf-8")
            settings = {"table": str(table_file)}
            run = yaml.safe_load((EXAMPLE / "eval.yaml").read_text(encoding="utf-8"))
            scenarios = run["sessions"]["scenarios"]
            # Give each (scenario, seed) pair a distinct key in the test's callback recorder.
            for index, scenario in enumerate(scenarios):
                scenario["seeds"] = [10001 + index]
            run["output"]["directory"] = str(root / "results")
            training_trace = {}
            env = ParlandoEnvironment(
                game=GAME,
                opponent={"factory": "wasm", "path": DEALER},
                schedule=run["schedule"],
                reward=correctness,
            )
            self.addAsyncCleanup(env.close_async)
            rewards = []
            for scenario in scenarios:
                seed = scenario["seeds"][0]
                policy = AgentPolicy(
                    lambda context: RecordingCueAgent(context, training_trace),
                    SessionInfo(role="A", seed=seed, settings=settings),
                )
                self.addAsyncCleanup(policy.close)
                obs = await env.reset_async(seed=seed, config=scenario["config"])
                await policy.reset(obs)
                for step in range(3):
                    obs = await env.step_async(await policy.act(obs))
                    if step < 2:
                        self.assertFalse(obs.done, obs)
                        self.assertFalse(obs.events)
                self.assertTrue(obs.done, obs)
                rewards.append(obs.reward)
                await policy.finish(obs)

            for trace in training_trace.values():
                self.assertEqual(
                    [row[1] for row in trace if row[0] == "decide"],
                    [Reason("scheduled"), Reason("scheduled"), Reason("action_rejected", "wrong_role")],
                )

            deployment_trace = {}
            server = grpc.aio.server()
            add_agent_service(server, lambda context: RecordingCueAgent(context, deployment_trace))
            port = server.add_insecure_port("127.0.0.1:0")
            run["agents"]["learner"]["settings"] = {
                "endpoint": f"http://127.0.0.1:{port}",
                "config_yaml": yaml.safe_dump(settings),
            }
            spec = root / "eval.yaml"
            spec.write_text(yaml.safe_dump(run), encoding="utf-8")
            await server.start()
            try:
                process = await asyncio.create_subprocess_exec(
                    str(Path(CLI).resolve()),
                    "run",
                    str(spec),
                    "--package",
                    str(GAME),
                    "--agent",
                    str(DEALER),
                    "--json",
                    cwd=root,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                )
                try:
                    stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=60)
                except asyncio.TimeoutError:
                    process.kill()
                    await process.communicate()
                    raise
                self.assertEqual(process.returncode, 0, (stdout.decode(), stderr.decode()))
                self.assertEqual(json.loads(stdout)["result"]["completed"], len(scenarios))
            finally:
                await server.stop(0)

            results = [
                json.loads(path.read_text(encoding="utf-8"))
                for path in (root / "results" / "results").glob("*.json")
            ]
            # The runner derives its agent context seed separately from the game seed.
            deployment_by_game_seed = {
                result["game_seed"]: deployment_trace[
                    next(agent["seed"] for agent in result["agents"] if agent["role"] == "A")
                ]
                for result in results
            }
            self.assertEqual(training_trace, deployment_by_game_seed)
            held_out_rewards = [
                float(row[1]["correct"])
                for trace in deployment_trace.values()
                for row in trace
                if row[0] == "finish"
            ]
            self.assertEqual(rewards, held_out_rewards)
            self.assertEqual(sum(held_out_rewards) / len(held_out_rewards), estimate)
