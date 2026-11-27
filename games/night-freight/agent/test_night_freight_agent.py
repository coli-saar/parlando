"""Tests for the Night Freight LLM partner with a stub model (no network).

Run from this directory: `python -m pytest -q`.
"""

from __future__ import annotations

import asyncio
import copy
import json
from pathlib import Path

from parlando_agent_sdk import Context, Reason, wire_value

from night_freight_agent import (
    MAX_RETRIES,
    LlmReply,
    NightFreightAgent,
    ToolCall,
    build_system_prompt,
    to_action,
)

EXAMPLES = Path(__file__).resolve().parents[1] / "server" / "examples"


def view(name: str) -> dict:
    """Loads a generated example view from the game's tests."""
    return json.loads((EXAMPLES / name).read_text())


class StubLlm:
    """Returns scripted replies and records every prompt it receives."""

    def __init__(self, *replies: LlmReply | Exception) -> None:
        """Queues replies; the last one repeats."""
        self.replies = list(replies)
        self.calls: list[tuple[str, str]] = []

    async def complete(self, system, user, tools):
        """Records the prompt and returns or raises the next scripted reply."""
        self.calls.append((system, user))
        reply = self.replies.pop(0) if len(self.replies) > 1 else self.replies[0]
        if isinstance(reply, Exception):
            raise reply
        return reply


def make(llm: StubLlm, role: str = "B", first_view: str = "view-routing-planning.json"):
    """Creates an agent already started on `first_view`."""
    agent = NightFreightAgent(Context(role=role, seed=1, settings={}), llm=llm)
    asyncio.run(agent.start(view(first_view)))
    return agent


def decide(agent, kind="after_observation", code=None):
    """Runs one decision."""
    return asyncio.run(agent.decide(Reason(kind=kind, rejection_code=code)))


def test_replay_and_continue_are_automatic_without_the_model():
    llm = StubLlm(LlmReply("never used"))
    agent = make(llm, first_view="view-routing-after-run.json")
    assert decide(agent).action == {"type": "replay_finished"}
    done = view("view-freight-scenario-done.json")
    asyncio.run(agent.observe_transition("A", {"type": "continue"}, done))
    assert decide(agent).action == {"type": "continue"}
    assert llm.calls == []


def test_partner_edit_payload_never_reaches_the_model():
    llm = StubLlm(LlmReply("Hello."))
    agent = make(llm)
    secret = {
        "type": "edit",
        "edit": {"type": "insert_op", "engine": "E77", "stop": 0, "index": 0,
                 "op": {"type": "deliver", "order": "O77"}},
    }
    asyncio.run(agent.observe_transition("A", secret, view("view-routing-planning.json")))
    assert decide(agent).message == "Hello."
    system, user = llm.calls[0]
    assert "E77" not in system + user and "O77" not in system + user
    assert agent.queue == [] and agent.pending_event is None


def test_prompt_holds_only_the_own_role():
    freight = build_system_prompt(view("view-freight-planning.json"))
    routing = build_system_prompt(view("view-routing-planning.json"))
    assert "YOU ARE FREIGHT" in freight and "travel_times" not in freight
    assert "YOU ARE ROUTING" in routing and '"orders"' not in routing


def test_tool_calls_become_ordered_actions_with_ready_last():
    reply = LlmReply(
        "Plan is in my notes.",
        [
            ToolCall("set_ready", {"ready": True}),
            ToolCall("append_stop", {"engine": "E1", "station": "dansville"}),
            ToolCall("set_depart", {"engine": "E1", "stop": 0,
                                    "depart": {"type": "at", "hour": 1, "engine": "x"}}),
            ToolCall("bogus", {}),
        ],
    )
    agent = make(StubLlm(reply))
    asyncio.run(agent.observe_message("A", "we agreed, ready?"))
    first = decide(agent)
    assert first.message == "Plan is in my notes."
    assert first.action["edit"]["type"] == "append_stop" and first.wakeup is not None
    second = decide(agent)
    assert second.action["edit"]["depart"] == {"type": "at", "hour": 1}
    third = decide(agent)
    assert third.action == {"type": "set_ready", "ready": True} and third.wakeup is None


def test_every_wire_example_is_reachable_from_a_tool():
    actions = json.loads((EXAMPLES / "actions.json").read_text())
    calls = {
        "edit_append_stop": ToolCall("append_stop", {"engine": "E1", "station": "dansville"}),
        "edit_clear_heard_arrival": ToolCall("set_heard_arrival", {"engine": "E1", "stop": 1, "hour": None}),
        "edit_insert_couple": ToolCall("insert_op", {"engine": "E1", "stop": 1, "index": 0, "op": {
            "type": "couple", "kind": "boxcar", "count": 2}}),
        "edit_insert_uncouple_loaded": ToolCall("insert_op", {"engine": "E1", "stop": 2, "index": 1, "op": {
            "type": "uncouple", "kind": "boxcar", "count": 1,
            "load": {"type": "loaded", "commodity": "oranges", "x": 1}}}),
        "edit_insert_load": ToolCall("insert_op", {"engine": "E1", "stop": 2, "index": 0, "op": {
            "type": "load", "commodity": "oranges", "count": 2, "kind": None}}),
        "edit_insert_deliver": ToolCall("insert_op", {"engine": "E1", "stop": 3, "index": 0, "op": {
            "type": "deliver", "order": "O1"}}),
    }
    for name, call in calls.items():
        assert to_action(call) == actions[name], name


def test_rejections_retry_then_tell_the_human():
    llm = StubLlm(LlmReply("Trying another.", [ToolCall("append_stop", {"engine": "E1", "station": "bath"})]))
    agent = make(llm)
    asyncio.run(agent.observe_message("A", "go to bath"))
    decide(agent)
    for _ in range(MAX_RETRIES):
        retry = decide(agent, "action_rejected", "not_adjacent")
        assert retry.action is not None
        assert "not_adjacent" in llm.calls[-1][1]
    final = decide(agent, "action_rejected", "not_adjacent")
    assert final.action is None and "not_adjacent" in final.message


def test_model_failure_is_reported_once_and_does_not_crash():
    agent = make(StubLlm(RuntimeError("down")))
    asyncio.run(agent.observe_message("A", "hi"))
    assert "can't reach" in decide(agent).message
    asyncio.run(agent.observe_message("A", "hello?"))
    assert decide(agent).message is None


def test_views_survive_the_double_precision_wire():
    agent = make(StubLlm(LlmReply("Hi.")))
    asyncio.run(agent.observe_transition("A", {}, wire_value(view("view-routing-planning.json"))))
    assert agent.view["runs_used"] == 0 and isinstance(agent.view["runs_used"], int)
    assert '"hour": 3.0' not in build_system_prompt(agent.view)
