# Night Freight

Night Freight is a two-player Parlando dialogue game modelled on the TRAINS
corpus. Two dispatchers plan a day of freight movements on a five-station
railway. **Freight** knows the orders, the cars and the handling times.
**Routing** knows the travel times, the track restrictions and the engines'
capacities. Each keeps private, structured notes on the whole plan, and the
only way to align them is to talk. When both press *Ready*, the game merges
the notes, simulates the day and plays it back. Where the players misunderstood
each other, the playback shows a train leaving without its cargo, and a caption states
what the player's own notes expected.

The first time a plan would have succeeded, the day is interrupted by a
disruption (an engine failure, a closed track or a late order). Only the
affected role is told the details, and the pair replans the rest of the day.

## How a stop works

Routing decides where and when the trains go: each stop in its strip has a
departure condition. Freight's notes say what happens at each place, as a list
of ops per place; they attach to Routing's visits by station and visit number
(the n-th time the engine is at that station), and the game fills in
the shortest path between non-adjacent places. The simulation applies these rules:

- **An engine does one op at a time and picks the first op that is possible.**
  When it is free, it starts the first op in its list whose precondition holds
  now, not necessarily the head of the list. Ops that take no time follow one
  another within the hour. List order only breaks ties, so "Load, then
  Couple" works: the engine couples first and then loads.
- **Departure comes last.** An engine never leaves while one of its ops at the
  stop is in progress, and never while an op there could still start. When its
  departure condition (`leave now`, `leave at`, `leave after`) holds, it
  finishes the op in progress, starts every remaining op that is possible, and
  only then leaves. Before the condition holds, ops that are not yet possible
  are retried every hour, so a hand-over from another engine can be awaited
  with `leave at` or `leave after`.
- **Ops that never became possible are reported with a reason**, for example
  "the yard has 1 empty boxcar" for a `couple` of oranges-loaded boxcars, and the
  engine leaves without them.
- **`couple` and `uncouple` filter cars by load**: `any` (the default; empty
  cars are taken first, then the lowest car number), `empty`, or loaded with a
  given commodity.
- **A missed order says what came closest**: no loaded car reached the
  destination, one arrived after the deadline, one arrived but was never
  delivered, or the order was delivered late.

The exact event fields are in [`server/WIRE-FORMAT.md`](server/WIRE-FORMAT.md).

## Configuration

```json
{ "scenarios": ["tutorial", "corning-juice", "bath-bananas"], "freight_seat": "a", "max_runs": 6 }
```

All keys are optional. `freight_seat` chooses which Parlando seat plays Freight.
`max_runs` limits the joint runs per scenario, both acts together.

## Scenarios

| Id | Content |
| --- | --- |
| `tutorial` | One engine, one order, one boxcar, all at Corning; couple, load and take the oranges to Dansville. No trap and no disruption. |
| `corning-juice` | Oranges to Bath and a tanker of juice to Avon; the classic map. |
| `bath-bananas` | Bananas to Bath and Corning with a closed track, two-hour loading and a spare engine. |

Scenarios are YAML files in `server/scenarios/`, compiled into the component.

## Build and test

```sh
cd server && cargo test            # unit, scenario and component-contract tests
cd .. && parlando test             # native tests plus the compiled component scenarios
./build.sh                         # writes dist/night-freight-<version>.zip
parlando check --package dist/night-freight-v2-0.1.5.zip
```

`scenarios.yaml` (the component scenarios for `parlando test`) is generated:

```sh
cd server && GENERATE_SCENARIOS=1 cargo test --test component_scenarios
```

## Play against a partner agent

You take one role and an agent plays the other. Which Parlando seat plays
Freight is the game setting `freight_seat` (default `a`). To play Freight
yourself, give yourself seat A; to play Routing, set `freight_seat` to `b`
if the agent is in seat B, or swap the seats. The agent plays whichever role
its seat has.

There are two partners. Both read only their own role's briefing, notes and
the chat. Parlando also delivers your `Edit` actions to the agent; neither
partner reads them, and your notes never reach the model. Both press
`ReplayFinished` after each replay and `Continue` at the end of a scenario
by themselves.

### LLM partner (plans and talks)

`agent/night_freight_agent.py` is a Python remote agent. It uses Claude
(default `claude-sonnet-5-5`) to talk with you, share the facts only its role
knows, propose plans, keep its own notes in line with what you agree, and
press *Ready* when you say you are ready or the plan is agreed. Rejected
edits are retried with the rejection code and, failing that, reported to you.
The model key stays in the agent process.

1. Start the agent, in a terminal on the same machine as Parlando:

   ```sh
   cd games/night-freight/agent
   python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
   ANTHROPIC_API_KEY=... .venv/bin/python night_freight_agent.py
   ```

   It listens on `127.0.0.1:50051` (`NIGHT_FREIGHT_AGENT_PORT` changes the
   port; `NIGHT_FREIGHT_MODEL` or the `model` setting changes the model).

2. Build and start a local host, and deploy the game:

   ```sh
   cd games/night-freight
   ./build.sh
   parlando serve                       # prints the dashboard link
   parlando login --server http://127.0.0.1:8000
   ./deploy.sh --host http://127.0.0.1:8000
   ```

3. In the dashboard open **Night Freight**, create an experiment, set
   **Participant pairing** to **Human vs agent**, choose **remote_grpc** as
   the agent factory, enter `http://127.0.0.1:50051` as its endpoint, and set
   the game configuration to `{ "freight_seat": "a" }` (or `"b"`). Start a
   **Local preview** and open the participant link.

Say what you know ("I have two orders ..."), ask what it knows ("what are the
travel times?"), agree a plan, and say "ready". Use typed chat.

### Scripted partner (no model)

`night_freight.partner` is a WASM policy packaged by `./build.sh` as
`dist/agent-night_freight.partner-1.0.1.zip` and uploaded by `./deploy.sh`.
It states its private facts when the phase starts and whenever you ask ("what
do you know?", "tell me the travel times"), presses *Ready* when you say
"ready" and withdraws it when you say "wait". It does not plan, but you can
steer its notes with short typed commands, so you do not have to write the
whole plan yourself. Choose it as the **Agent factory** in step 3 instead of
`remote_grpc`.

Commands ignore case and extra words. Stops and ops are numbered from 0 (stop 0
is the engine's starting station), as in `show notes`. Separate several
commands with `;`. Without an engine id a command applies to the only engine;
without `stop` an op goes to the engine's last stop.

| Say | Effect |
| --- | --- |
| `E1 to Dansville`, `E1 go Dansville Corning Bath` | append stops |
| `E1 cut from 2` | remove stop 2 and later |
| `E1 stop 2 leave at 5`, `E1 stop 2 leave now`, `E1 stop 2 leave 1 after E2` | set a departure |
| `E1 stop 0 couple 2 boxcar`, `uncouple 1 boxcar oranges`, `load 2 oranges`, `process 1`, `deliver O1` | add an op at the end of the stop |
| `E1 stop 1 remove op 0` | remove an op |
| `E1 stop 1 heard 6`, `E1 stop 1 heard none` | jot or clear an arrival hour |
| `show notes`, `help` | list its notes, list the commands |

The partner answers each command with a short confirmation, or with the
game's rejection in plain words (it then drops the rest of your commands).
Either role accepts every command, because a player may edit every field of
their own notes, but a run uses only the stations and departures of Routing
and the ops of Freight; the reply says when a field is not used by its role.
Edits withdraw its readiness, so say "ready" afterwards. Example, with the
partner as Routing in the tutorial:

```text
you:      E1 to Dansville; E1 stop 0 leave at 1
partner:  Done: E1 now goes on to dansville.
          Done: E1 stop 0, leave at 1.
you:      show notes
partner:  My notes:
          Strip E1: [0 corning leave at 1] > [1 dansville leave now]
you:      ready
```

To watch two scripted partners cycle through ready, replay and continue
headlessly (they take no note commands, so every run fails):

```sh
parlando run agent-experiment.yaml --package dist/night-freight-v2-0.1.5.zip \
  --agent dist/agent-night_freight.partner-1.0.1.zip
```

Results are written to `results/partner-pairs/`; remove it before re-running.
The LLM partner's tests need no key: `cd agent && python -m pytest -q`
(with `parlando-agent-sdk` installed).

The JSON the browser client exchanges with the game is described in
[`server/WIRE-FORMAT.md`](server/WIRE-FORMAT.md).
