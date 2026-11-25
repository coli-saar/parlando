"""Check role-private Verifiers prompts, replies, and SDK trace semantics."""

import asyncio
import json
from pathlib import Path

from importlib.util import find_spec
from unittest import SkipTest

if find_spec("verifiers") is None or find_spec("pytest") is None:
    raise SkipTest("requires Verifiers and pytest")

import pytest

pytest.importorskip("verifiers.v1")
import verifiers.v1 as vf
from aiohttp import web
from parlando_agent_sdk import Decision, Message, Observation, Reason, Transition
from parlando_rl import PolicyInput
from parlando_rl.verifiers import (
    ParlandoModelAgent,
    decision_prompt,
    parse_decision,
)
from verifiers.v1.configs.cli.run import RunnerConfig
from verifiers.v1.runtimes.backends import subprocess as vf_subprocess
from verifiers.v1.utils.loaders import load_environment


ROOT = Path(__file__).resolve().parents[2]
EXAMPLE = ROOT / "parlando-rl/examples/last_screening"


@pytest.fixture(autouse=True)
def runtime_cache(monkeypatch, tmp_path_factory):
    """Keep Verifiers subprocesses and script bootstrap files in a test cache."""
    cache = tmp_path_factory.getbasetemp() / "verifiers"
    monkeypatch.setattr(vf_subprocess, "CACHE_DIR", cache)
    monkeypatch.setattr(
        vf_subprocess.SubprocessBackend, "scripts_dir", str(cache / "scripts")
    )


@pytest.mark.parametrize(
    "reply",
    [
        "speech",
        "[]",
        '{"action":1}',
        '{"other":0}',
        '{"action":{"position":NaN}}',
        '{"action":{"position":1e999}}',
    ],
)
def test_rejects_malformed_envelopes(reply):
    """Only a JSON object with typed action and message fields is a decision."""
    with pytest.raises(ValueError):
        parse_decision(reply)


def test_checkpoint_receives_rollout_frames():
    """Deployment preserves opening and subsequent private callbacks in chat history."""
    view = {"role": "A", "cue": "opening"}
    after = {"role": "A", "cue": "updated"}
    opening_event = Observation(message=Message(sender="B", text="opening"))
    combined_event = Observation(
        transition=Transition("B", {"op": "update"}, after),
        message=Message(sender="B", text="next"),
    )
    reason = Reason("after_observation")
    requests = []

    async def complete(messages):
        """Supply one action-plus-message and one yield to the SDK agent."""
        requests.append(messages)
        return (
            '{"action":{"op":"play"},"message":"hello"}' if len(requests) == 1 else "{}"
        )

    async def play():
        """Deliver the identical callbacks available from a headless rollout."""
        agent = ParlandoModelAgent(complete, instructions="game instructions")
        await agent.start(view)
        await agent.observe(opening_event)
        assert await agent.decide(reason) == Decision(
            action={"op": "play"}, message="hello"
        )
        await agent.observe(combined_event)
        assert await agent.decide(reason) == Decision()

    asyncio.run(play())
    assert requests[0] == [
        {"role": "system", "content": "game instructions"},
        {
            "role": "user",
            "content": decision_prompt(
                PolicyInput(view, reason, (opening_event,), view)
            ),
        },
    ]
    assert requests[1][:-2] == requests[0]
    assert requests[1][-1]["content"] == decision_prompt(
        PolicyInput(after, reason, (combined_event,), None)
    )


def test_sdk_agent_scores_dialogue_in_verifiers(monkeypatch):
    """An SDK stagehand earns escape reward in a regular Verifiers episode."""
    game = ROOT / "games/last-screening/dist/last-screening-0.1.0.zip"
    opponent = game.with_name("agent-last_screening.operator-1.0.0.zip")
    if not game.is_file() or not opponent.is_file():
        pytest.skip("build The Last Screening game and operator archives")
    monkeypatch.syspath_prepend(str(EXAMPLE))
    config = RunnerConfig.model_validate(
        {
            "env": {
                "game": game,
                "opponent": {"factory": "wasm", "path": str(opponent)},
                "learner_role": "B",
                "schedule": {"kind": "alternate_every_response", "first": "A"},
                "taskset": {"id": "last_screening_verifiers", "seeds": [1]},
                "sdk_agent": "last_screening_verifiers:Stagehand",
            },
            "model": "scripted-stagehand",
        }
    )
    env = load_environment(config.env)

    async def run():
        """Collect the SDK transcript in a genuine, non-trainable Verifiers trace."""
        async with env.serving():
            task = next(iter(env.taskset))
            return await env.run_episode(
                task,
                vf.ModelContext(
                    model=config.model, client=config.client, sampling=config.sampling
                ),
            )

    episode = asyncio.run(run())
    assert episode.ok, episode.errors or [trace.errors for trace in episode.traces]
    (trace,) = episode.traces
    assert trace.reward == 1
    assert not trace.agent.trainable and not trace.calls
    assert trace.stop_condition == "parlando_done"
    assert trace.info["parlando_final_frame"]["metadata"]["completion"]["escaped"]
    views = [json.loads(node.message.content) for node in trace.nodes[::2]]
    actions = [json.loads(node.message.content) for node in trace.nodes[1::2]]
    assert all(frame["view"]["role"] == "stagehand" for frame in views)
    assert any(action.get("message", "").startswith("window ") for action in actions)
    assert any(
        (message := event.get("message")) and message["text"].startswith("tower ")
        for frame in views
        for event in frame["events"]
    )


def test_last_sampled_turn_is_applied_to_game():
    """Apply the last sampled reply at the turn limit and score a genuine model trace."""
    dist = ROOT / "experiments/cue-choice-rl/dist"
    game = dist / "cue-choice-0.1.0.zip"
    opponent = dist / "agent-cue_choice.dealer-1.0.0.zip"
    if not game.is_file() or not opponent.is_file():
        pytest.skip("build Cue Choice game and dealer archives")

    async def complete(request):
        """Answer as an OpenAI-compatible streaming completion provider."""
        body = await request.json()
        frame = json.loads(body["messages"][-1]["content"])
        assert frame["view"]["cue"] == "dax"
        action = next(
            a for a in frame["view"]["available_actions"] if a["choice"] == "oak"
        )
        chunks = [
            {
                "choices": [
                    {
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "content": json.dumps(
                                {"action": action, "message": "I saw dax"}
                            ),
                        },
                    }
                ]
            },
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
            {
                "choices": [],
                "usage": {
                    "prompt_tokens": 32,
                    "completion_tokens": 8,
                    "total_tokens": 40,
                },
            },
        ]
        result = "".join(
            "data: "
            + json.dumps(
                {
                    "id": "test-completion",
                    "model": body["model"],
                    "created": 1,
                    "object": "chat.completion.chunk",
                    **chunk,
                }
            )
            + "\n\n"
            for chunk in chunks
        )
        return web.Response(
            text=result + "data: [DONE]\n\n", content_type="text/event-stream"
        )

    async def run():
        """Let Verifiers drive one sampled game turn through its stock chat harness."""
        provider = web.Application()
        provider.router.add_post("/v1/chat/completions", complete)
        runner = web.AppRunner(provider)
        await runner.setup()
        try:
            site = web.TCPSite(runner, "127.0.0.1", 0)
            await site.start()
            port = runner.addresses[0][1]
            config = RunnerConfig.model_validate(
                {
                    "env": {
                        "game": game,
                        "opponent": {"factory": "wasm", "path": str(opponent)},
                        "schedule": {"kind": "alternate_after_action", "first": "B"},
                        "taskset": {
                            "id": "parlando_verifiers",
                            "seeds": [1],
                            "scenarios": [{"level": "dax"}],
                        },
                        "player": {"max_turns": 1},
                    },
                    "model": "test-cue-policy",
                    "client": {
                        "base_url": f"http://127.0.0.1:{port}/v1",
                        "api_key_var": "VF_TEST_KEY",
                    },
                }
            )
            env = load_environment(config.env)
            async with env.serving():
                return await env.run_episode(
                    next(iter(env.taskset)),
                    vf.ModelContext(
                        model=config.model,
                        client=config.client,
                        sampling=config.sampling,
                    ),
                )
        finally:
            await runner.cleanup()

    episode = asyncio.run(run())
    assert episode.ok, episode.errors or [trace.errors for trace in episode.traces]
    (trace,) = episode.traces
    assert trace.reward == 1
    assert trace.agent.trainable and trace.num_turns == 1
    assert len(trace.calls) == 1
