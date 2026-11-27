"""LLM-backed Night Freight partner, served to Parlando over remote gRPC.

The agent plays either role (Freight or Routing) against a human. A WASM
policy cannot call a model, so this process holds the model credential and
Parlando connects to it with the `remote_grpc` agent factory.

Division of labour
------------------
Code owns everything deterministic:

* `ReplayFinished` after a replay and `Continue` at the end of a scenario are
  sent as soon as they are legal, with no model call.
* Model output is turned into game actions by `to_action`, edits are sent one
  per decision, and `SetReady` is always moved behind the edits (an edit
  withdraws readiness).
* A rejected action is fed back to the model with its rejection code, at most
  `MAX_RETRIES` times per human turn; after that the human is told.

The model owns judgement: what to say, which notes to write, when the plan is
agreed. It sees a system prompt with its own role's briefing and notes, the
last run, the chat so far and tools that map one-to-one to `NoteEdit`,
`SetReady`.

Information boundary
--------------------
Parlando delivers every accepted action to both seats, including the human's
`Edit` actions (design note 15.1). `NightFreightAgent.observe_transition`
therefore keeps only the resulting view and discards the actor and the action.
The human's notes are never in the view and never reach the prompt.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from datetime import timedelta
from typing import Any, Protocol

from parlando_agent_sdk import (
    Agent,
    Context,
    Decision,
    Reason,
    WakeupRequest,
    serve,
)

#: Default model; override with the `model` setting or `NIGHT_FREIGHT_MODEL`.
DEFAULT_MODEL = "claude-sonnet-5-5"
#: Rejected edits the model may retry before the human is told.
MAX_RETRIES = 2
#: Delay before the next queued action in live play.
QUEUE_DELAY = timedelta(milliseconds=300)
#: Cap on trace events shown to the model.
MAX_TRACE_LINES = 90


# ---------------------------------------------------------------------------
# Model interface
# ---------------------------------------------------------------------------


@dataclass
class ToolCall:
    """One tool invocation requested by the model."""

    name: str
    input: dict[str, Any]


@dataclass
class LlmReply:
    """The model's answer: words for the human and tool calls for the game."""

    text: str = ""
    calls: list[ToolCall] = field(default_factory=list)


class Llm(Protocol):
    """Anything that can answer one planning turn. Tests supply a stub."""

    async def complete(self, system: str, user: str, tools: list[dict[str, Any]]) -> LlmReply:
        """Returns the reply to `user` under `system` with `tools` available."""
        ...


class AnthropicLlm:
    """Calls the Anthropic Messages API (key from `ANTHROPIC_API_KEY`)."""

    def __init__(self, model: str, max_tokens: int = 1500) -> None:
        """Creates the client lazily so tests need not install the SDK."""
        import anthropic  # imported here: only the live agent needs it

        self._client = anthropic.AsyncAnthropic()
        self._model = model
        self._max_tokens = max_tokens

    async def complete(self, system: str, user: str, tools: list[dict[str, Any]]) -> LlmReply:
        """Sends one stateless turn and collects its text and tool calls."""
        response = await self._client.messages.create(
            model=self._model,
            max_tokens=self._max_tokens,
            system=system,
            tools=tools,
            messages=[{"role": "user", "content": user}],
        )
        reply = LlmReply()
        for block in response.content:
            if block.type == "text":
                reply.text += block.text
            elif block.type == "tool_use":
                reply.calls.append(ToolCall(block.name, dict(block.input)))
        reply.text = reply.text.strip()
        return reply


# ---------------------------------------------------------------------------
# Tools and their mapping to game actions
# ---------------------------------------------------------------------------

_DEPART = {
    "type": "object",
    "description": "When the engine leaves this stop.",
    "properties": {
        "type": {"enum": ["immediately", "at", "after"]},
        "hour": {"type": "integer", "description": "For 'at': the hour of the day."},
        "engine": {"type": "string", "description": "For 'after': the engine to wait for."},
        "plus_hours": {"type": "integer", "description": "For 'after': hours after it is present."},
    },
    "required": ["type"],
}

_LOAD_FILTER = {
    "type": "object",
    "description": (
        "couple/uncouple: which cars of the kind qualify by load. {type: any} (the default "
        "when omitted) takes any car, empty ones first; {type: empty} only empty cars; "
        "{type: loaded, commodity} only cars loaded with that commodity."
    ),
    "properties": {
        "type": {"enum": ["any", "empty", "loaded"]},
        "commodity": {"enum": ["oranges", "bananas", "juice"], "description": "For 'loaded'."},
    },
    "required": ["type"],
}

_OP = {
    "type": "object",
    "description": "A cargo step at a stop.",
    "properties": {
        "type": {"enum": ["couple", "uncouple", "load", "process", "deliver"]},
        "kind": {"enum": ["boxcar", "tanker"], "description": "couple/uncouple."},
        "load": _LOAD_FILTER,
        "count": {"type": "integer", "description": "1 to 9 (couple, uncouple, load, process)."},
        "commodity": {"enum": ["oranges", "bananas", "juice"], "description": "load."},
        "order": {"type": "string", "description": "deliver: the order id, e.g. O1."},
    },
    "required": ["type"],
}


def _tool(name: str, description: str, **properties: Any) -> dict[str, Any]:
    """Builds one tool definition whose properties are all required."""
    return {
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": list(properties),
        },
    }


_ENGINE = {"type": "string", "description": "Engine id such as E1."}
_STOP = {"type": "integer", "description": "Zero-based stop index in the engine's strip."}

#: The tools offered to the model; each maps to one game action.
TOOLS: list[dict[str, Any]] = [
    _tool(
        "append_stop",
        "Add a stop to the end of an engine's strip. The station must be adjacent to the last stop.",
        engine=_ENGINE,
        station={"type": "string", "description": "Station id such as corning."},
    ),
    _tool(
        "truncate_stops",
        "Remove stops from_index.. of an engine's strip (from_index >= 1).",
        engine=_ENGINE,
        from_index={"type": "integer"},
    ),
    _tool("set_depart", "Set when the engine leaves a stop.", engine=_ENGINE, stop=_STOP, depart=_DEPART),
    _tool(
        "insert_op",
        "Insert a cargo step at a stop; index is its position among the stop's steps.",
        engine=_ENGINE,
        stop=_STOP,
        index={"type": "integer"},
        op=_OP,
    ),
    _tool("remove_op", "Remove a cargo step.", engine=_ENGINE, stop=_STOP, index={"type": "integer"}),
    _tool(
        "set_heard_arrival",
        "Jot an arrival hour the partner told you (annotation only; null clears).",
        engine=_ENGINE,
        stop=_STOP,
        hour={"type": ["integer", "null"]},
    ),
    _tool(
        "set_ready",
        "Declare (true) or withdraw (false) readiness to run. Only when the human has "
        "said they are ready or the plan is agreed, and your notes match it.",
        ready={"type": "boolean"},
    ),
]


#: Fields each cargo step carries on the wire, besides its `type`.
_OP_FIELDS = {
    "couple": ("kind", "load", "count"),
    "uncouple": ("kind", "load", "count"),
    "load": ("commodity", "count"),
    "process": ("count",),
    "deliver": ("order",),
}
#: Fields each departure rule carries on the wire, besides its `type`.
_DEPART_FIELDS = {"immediately": (), "at": ("hour",), "after": ("engine", "plus_hours")}
#: Fields each load filter carries on the wire, besides its `type`.
_LOAD_FIELDS = {"any": (), "empty": (), "loaded": ("commodity",)}


def clean(value: dict[str, Any], fields: dict[str, tuple[str, ...]]) -> dict[str, Any]:
    """Keeps exactly the fields the wire format defines for `value["type"]`.

    A missing `load` means the filter `{"type": "any"}`; any other missing field
    raises `KeyError`. Extra fields the model sent are dropped.
    """
    kind = value["type"]
    if kind not in fields:
        raise ValueError(f"unknown type {kind!r}")
    out: dict[str, Any] = {"type": kind}
    for f in fields[kind]:
        if f == "load":
            out[f] = clean(value.get(f) or {"type": "any"}, _LOAD_FIELDS)
        else:
            out[f] = value[f]
    return out


def to_action(call: ToolCall) -> dict[str, Any]:
    """Maps one model tool call to a game action in the wire format.

    Raises `ValueError` for an unknown tool or missing field, so a malformed
    call is dropped and reported to the model instead of reaching the game.
    """
    i = call.input
    try:
        if call.name == "set_ready":
            return {"type": "set_ready", "ready": bool(i["ready"])}
        edit: dict[str, Any]
        if call.name == "append_stop":
            edit = {"type": "append_stop", "engine": i["engine"], "station": i["station"]}
        elif call.name == "truncate_stops":
            edit = {"type": "truncate_stops", "engine": i["engine"], "from": int(i["from_index"])}
        elif call.name == "set_depart":
            edit = {
                "type": "set_depart",
                "engine": i["engine"],
                "stop": int(i["stop"]),
                "depart": clean(i["depart"], _DEPART_FIELDS),
            }
        elif call.name == "insert_op":
            edit = {
                "type": "insert_op",
                "engine": i["engine"],
                "stop": int(i["stop"]),
                "index": int(i["index"]),
                "op": clean(i["op"], _OP_FIELDS),
            }
        elif call.name == "remove_op":
            edit = {
                "type": "remove_op",
                "engine": i["engine"],
                "stop": int(i["stop"]),
                "index": int(i["index"]),
            }
        elif call.name == "set_heard_arrival":
            hour = i["hour"]
            edit = {
                "type": "set_heard_arrival",
                "engine": i["engine"],
                "stop": int(i["stop"]),
                "hour": None if hour is None else int(hour),
            }
        else:
            raise ValueError(f"unknown tool {call.name}")
    except KeyError as missing:
        raise ValueError(f"{call.name}: missing field {missing}") from None
    return {"type": "edit", "edit": edit}


def order_actions(calls: list[ToolCall]) -> tuple[list[dict[str, Any]], list[str]]:
    """Converts calls to actions, edits first and the last `set_ready` at the end.

    Returns the ordered actions and one error text per dropped call. Readiness
    goes last because every accepted edit withdraws it.
    """
    edits: list[dict[str, Any]] = []
    ready: dict[str, Any] | None = None
    errors: list[str] = []
    for call in calls:
        try:
            action = to_action(call)
        except (ValueError, TypeError) as error:
            errors.append(str(error))
            continue
        if action["type"] == "set_ready":
            ready = action
        else:
            edits.append(action)
    return edits + ([ready] if ready else []), errors


# ---------------------------------------------------------------------------
# Prompt
# ---------------------------------------------------------------------------

RULES = """\
You are playing Night Freight with a human partner. Two dispatchers plan one day
(hours 0 to 24) of freight movements on a small single-track railway. Each of you
knows half of the facts and keeps private notes on the WHOLE plan. You cannot see
your partner's notes or briefing; the only way to align is to talk. When both of
you are ready the game merges the notes, simulates the day and plays it back.

Roles. FREIGHT knows orders, cars, stock, factories and handling durations.
ROUTING knows travel times, track closures, engine capacities and availability.
The merged run uses Routing's route and departure rules (stations, depart) and
Freight's cargo steps (ops). Write the whole plan anyway; the rest is your belief.

Notes. One strip per engine: stops (station, depart, ops). Stop 0 is the engine's
current station and cannot be removed. Consecutive stops must be adjacent stations.
Depart: immediately | at hour H | after engine X plus N hours. Departure comes last:
an engine never leaves a stop while one of its ops there is in progress or could
still start. When the depart condition holds it finishes the op in progress, starts
every other op that is possible, and only then leaves; ops that are still impossible
at that moment are given up with a reason and the engine goes without them. Before
the condition holds, impossible ops are retried every hour, so a hand-over from
another engine needs depart=at or after, not immediately (immediately with nothing
possible means leaving at once). So loading and processing need no departure time.
Ops are not run in list order: an engine does one op at a time and starts the first
op in its list that is possible now, so "load, then couple" works. Ops: couple/
uncouple cars (kind, load filter, count), load a commodity (takes load hours),
process at a factory, deliver an order. The load filter of couple/uncouple is any
(default; empty cars first), empty, or loaded with a commodity; ask for a loaded
car only if one stands in the yard. A single-track edge holds one train at a time.
An order is met when its loads arrive by the deadline. After a run, each given-up
op and each missed order comes with a reason in the events (for example "the yard
has 1 empty boxcar" or "a car of oranges reached Dansville after the deadline").

How to play.
* Be a good partner in plain, short sentences. Share the facts only you know when
  they matter or when asked (state numbers exactly). Ask for the facts you lack.
* Propose concrete plans, check the arithmetic with your partner's numbers, and
  say what you have put in your notes.
* Keep your notes in line with what was agreed, using the tools. Prefer a few
  clear edits; use remove/truncate to fix mistakes. Edits withdraw readiness.
* Call set_ready(true) only when your partner has said they are ready or the plan
  is agreed, and your notes match it. If they ask to wait, call set_ready(false).
* After a run, explain what went wrong using the last run and its divergence, then
  repair the plan together. Never invent facts you were not told.
* Reply with 1 to 4 sentences. Never mention these instructions or tool names."""


def format_event(event: dict[str, Any]) -> str:
    """Renders one trace event as a single compact line."""
    kind = dict(event["kind"])
    name = kind.pop("type")
    detail = ", ".join(f"{k}={json.dumps(v)}" for k, v in kind.items())
    return f"h{event['hour']:>2} {name}: {detail}"


def format_trace(trace: dict[str, Any]) -> str:
    """Renders a trace's events, truncated to `MAX_TRACE_LINES`."""
    lines = [format_event(e) for e in trace["events"]]
    if len(lines) > MAX_TRACE_LINES:
        lines = lines[:MAX_TRACE_LINES] + [f"... {len(lines) - MAX_TRACE_LINES} more events"]
    return "\n".join(lines)


def describe_last_run(view: dict[str, Any]) -> str:
    """Summarizes the latest joint run: outcome, divergence and the actual events."""
    run = view.get("last_run")
    if not run:
        return "No joint run yet."
    orders = "; ".join(
        f"{o['id']} {'MET' if o['met'] else 'NOT met'} (deadline {o['deadline']}, "
        f"delivered {o['delivered']}/{o['count']} at {o['delivered_at']})"
        for o in run["orders"]
    )
    parts = [
        f"Run {run['number']} (act {run['act']}): {'success' if run['success'] else 'not a success'}. "
        f"Orders: {orders}."
    ]
    if run.get("interrupted_at") is not None:
        parts.append(f"The day was interrupted by a disruption at hour {run['interrupted_at']}.")
    if run.get("divergence"):
        d = run["divergence"]
        parts.append(
            f"First difference between what happened and YOUR notes' prediction: hour {d['hour']}, "
            f"engine {d['engine']}, actual={json.dumps(d['actual'])}, expected={json.dumps(d['expected'])}."
        )
    else:
        parts.append("What happened matched your notes' prediction.")
    parts.append("Events of what actually happened:\n" + format_trace(run["actual"]))
    return "\n".join(parts)


def build_system_prompt(view: dict[str, Any]) -> str:
    """Builds the system prompt from the agent's own role view only.

    Receives the latest view (its briefing, notes, map, flags, scores, last
    run). Nothing from the partner's notes can appear because the view does
    not contain them and `observe_transition` stores nothing else.
    """
    role = view["role"].upper()
    state = {
        "phase": view["phase"],
        "act": view["act"],
        "scenario": f"{view['scenario_title']} ({view['scenario_id']}): {view['scenario_summary']}",
        "runs_used": f"{view['runs_used']} of {view['max_runs']}",
        "ready": view["ready"],
        "map": view["map"],
        "scores": view["scores"],
    }
    return "\n\n".join(
        [
            RULES,
            f"YOU ARE {role}.",
            "Your private briefing (your partner cannot see it):\n"
            + json.dumps(view["briefing"], indent=1),
            "Your current notes:\n" + json.dumps(view["notes"], indent=1),
            "Game state:\n" + json.dumps(state, indent=1),
            "Last run:\n" + describe_last_run(view),
        ]
    )


# ---------------------------------------------------------------------------
# The agent
# ---------------------------------------------------------------------------


def normalize(value: Any) -> Any:
    """Turns integral floats back into ints, recursively.

    The remote-agent protocol carries every number as a double, so an hour
    arrives as `5.0`. Restoring ints keeps prompts readable and indices exact.
    """
    if isinstance(value, float) and value.is_integer():
        return int(value)
    if isinstance(value, list):
        return [normalize(v) for v in value]
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in value.items()}
    return value


class NightFreightAgent(Agent):
    """One seat of one session: deterministic phase steps plus a model for talk and notes."""

    def __init__(self, context: Context, llm: Llm | None = None) -> None:
        """Binds the session context; `llm` defaults to the Anthropic client."""
        self.context = context
        model = context.settings.get("model") or os.environ.get("NIGHT_FREIGHT_MODEL", DEFAULT_MODEL)
        self.llm: Llm = llm if llm is not None else AnthropicLlm(model)
        #: The latest role view; the only game state the agent keeps.
        self.view: dict[str, Any] | None = None
        #: Chat transcript as (speaker, text) pairs, speaker "human" or "me".
        self.chat: list[tuple[str, str]] = []
        #: Actions still to send, one per decision.
        self.queue: list[dict[str, Any]] = []
        #: Why the model should speak at the next planning decision, if at all.
        self.pending_event: str | None = None
        #: The planning phase last announced to the model: (scenario, act, runs).
        self.seen_phase: tuple[Any, ...] | None = None
        #: Rejections since the last human message.
        self.retries = 0
        #: Whether the model is unreachable (reported to the human once).
        self.llm_down_reported = False

    # -- observations -------------------------------------------------------

    async def start(self, observation: dict[str, Any]) -> None:
        """Stores the opening view."""
        self.view = normalize(observation)

    async def observe_transition(
        self, actor: str, action: dict[str, Any], observation: dict[str, Any]
    ) -> None:
        """Keeps the resulting view and discards the actor and action.

        The action may be the human's `Edit`; its payload describes the human's
        notes and must never reach the model.
        """
        del actor, action
        self.view = normalize(observation)

    async def observe_message(self, sender: str, text: str) -> None:
        """Records a human chat message and schedules a reply."""
        self.chat.append(("human", text))
        self.pending_event = "Your partner just spoke. Answer them."
        self.retries = 0

    # -- decisions ----------------------------------------------------------

    async def decide(self, reason: Reason) -> Decision:
        """Chooses the next action and message for this decision opportunity."""
        view = self.view
        if view is None:
            return Decision()
        phase = view["phase"]
        if reason.kind == "action_rejected" and phase != "planning":
            return Decision()  # an automatic step was refused: the view already shows why
        if phase == "replaying" and not view["replay_finished"]["me"]:
            return Decision(action={"type": "replay_finished"})
        if phase == "scenario_done" and not view["continue"]["me"]:
            return Decision(action={"type": "continue"})
        if phase != "planning":
            return Decision()
        if reason.kind == "action_rejected":
            return await self._after_rejection(reason.rejection_code or "unknown")
        if self.queue:
            return self._next_queued()
        key = (view["scenario_index"], view["act"], view["runs_used"])
        if key != self.seen_phase:
            self.seen_phase = key
            event = self._phase_event(view)
            self.pending_event = f"{self.pending_event} Also: {event}" if self.pending_event else event
        if self.pending_event is None:
            return Decision()
        return await self._think(self.pending_event)

    @staticmethod
    def _phase_event(view: dict[str, Any]) -> str:
        """Describes why the agent should speak when a planning phase begins."""
        if view["runs_used"] == 0 and view["act"] == 1:
            return "A new scenario begins. Greet your partner, share what matters of your briefing, and start planning."
        if view["act"] == 2:
            return "A disruption interrupted the day. Tell your partner anything only you were told, then replan."
        return "The run failed and you are back in planning. Explain what you see and propose a repair."

    async def _think(self, event: str) -> Decision:
        """Asks the model for one turn and queues its actions."""
        assert self.view is not None
        self.pending_event = None
        try:
            reply = await self.llm.complete(
                build_system_prompt(self.view), self._user_turn(event), TOOLS
            )
        except Exception as error:  # network, quota, malformed reply: never crash the seat
            self.context.logger.log(f"model call failed: {error!r}")
            if self.llm_down_reported:
                return Decision()
            self.llm_down_reported = True
            return self._say(
                "I can't reach my planning assistant right now. Tell me exactly what to put in my notes."
            )
        actions, errors = order_actions(reply.calls)
        for error in errors:
            self.context.logger.log(f"dropped tool call: {error}")
        self.queue = actions
        text = reply.text
        if text:
            self.chat.append(("me", text))
        first = self._next_queued() if self.queue else Decision()
        if text and not first.message:
            return Decision(action=first.action, message=text, wakeup=first.wakeup)
        return first

    def _user_turn(self, event: str) -> str:
        """Renders the chat so far and the triggering event as the user turn."""
        transcript = "\n".join(
            f"{'Partner' if who == 'human' else 'You'}: {text}" for who, text in self.chat[-30:]
        )
        return f"Chat so far:\n{transcript or '(nothing yet)'}\n\nEvent: {event}"

    def _next_queued(self) -> Decision:
        """Pops one queued action; asks to be woken for the next one."""
        action = self.queue.pop(0)
        wakeup = WakeupRequest.after(QUEUE_DELAY) if self.queue else None
        return Decision(action=action, wakeup=wakeup)

    async def _after_rejection(self, code: str) -> Decision:
        """Drops the rest of the batch and lets the model repair, or tells the human."""
        self.queue.clear()
        self.retries += 1
        if self.retries > MAX_RETRIES:
            self.retries = 0
            return self._say(
                f"I couldn't apply one of my note edits (the game said: {code}). "
                "Tell me what you'd like in my notes."
            )
        return await self._think(
            f"Your last action was rejected with code '{code}'. Your notes are unchanged by it. "
            "Fix the problem with different edits, or explain to your partner."
        )

    def _say(self, text: str) -> Decision:
        """Sends a message and records it in the transcript."""
        self.chat.append(("me", text))
        return Decision(message=text)


def create(context: Context) -> NightFreightAgent:
    """Factory used by `serve`: one agent per session seat."""
    return NightFreightAgent(context)


if __name__ == "__main__":
    serve(
        create,
        host=os.environ.get("NIGHT_FREIGHT_AGENT_HOST", "127.0.0.1"),
        port=int(os.environ.get("NIGHT_FREIGHT_AGENT_PORT", "50051")),
    )
