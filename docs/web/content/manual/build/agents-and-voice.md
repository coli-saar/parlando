---
title: Attach an automated player
description: Run a Python agent service, select it in an inactive experiment, and verify one live human-agent session.
---

# Attach an automated player

An agent is another implementation of a player. It occupies seat `A` or `B`, receives only that
seat's observation, observes accepted actions and partner messages, and proposes the same actions a
human can propose. It does not receive authoritative state, browser events, or the other role's
private observation.

This symmetry is the central agent principle in Parlando. The game defines what a player may know
and do; the controller may be a browser, compiled Rust policy, or Python service. Replacing a human
with an agent therefore changes the experimental condition without creating a second version of the
task rules.

{{< figure src="manual/images/agent-modes.svg" alt="Human–agent and agent–agent modes share the same game rules but use different study workflows" caption="Both modes use the same task boundary. Live participant studies use the dashboard; headless batches use YAML and result files." >}}

## Separate the controller from the execution mode

Two decisions are independent. First choose where the policy runs. A Rust agent is compiled into the
game server; a Python agent runs as a separate service. Then choose how the policy
is studied. A live human–agent experiment is configured in the dashboard and recorded with
participant sessions. A [headless agent–agent study](agent-agent/) is expanded from YAML and writes
batch results outside the live database.

Use an in-process Rust agent for a small deterministic policy with no separate service needs. Use a
Python agent when the policy depends on Python libraries, model infrastructure, or independent
deployment. This choice changes construction and operations, not the observation or action
semantics.

## Understand the agent lifecycle

Parlando creates a fresh agent for each automated participant. It supplies the seat, a deterministic
seed, and the configured non-secret settings. The initial game observation arrives after the agent
starts successfully.

The policy then participates through five events:

| Event | Meaning |
| --- | --- |
| `start(observation)` | The initial role-specific information state is available. |
| `observe_transition(actor, action, observation)` | An action was accepted and produced a new observation for this agent. |
| `observe_message(sender, text)` | The other player communicated. |
| `respond(available_actions)` | The policy may return an action, a message, both, or yield. |
| `finish(completion)` | The shared terminal result is known; cleanup follows. |

A yield means that the policy has nothing to do until another event arrives. It is represented by
`None`, not by an empty response. Messages communicate but do not change task state. Actions always
pass through the game's authoritative validation, including actions returned from a supposedly
trusted compiled policy.

Agent startup and responses are bounded by the experiment's action timeout. Repeated invalid actions
are bounded separately. When either limit is exceeded, Parlando records the failure and ends the
session rather than changing policies or inventing an action. Choose limits that permit ordinary
model latency while still giving participants a finite failure path.

## Configure a live human–agent condition

In a live human–agent session, Parlando assigns the human to seat `A` and the selected agent to seat
`B`. A game may map those seats onto domain roles. For example, a configuration might assign seat
`A` to a director in one condition and to a matcher in another. Record that mapping in game
configuration so that it remains part of the session's provenance.

Under **Players and agents**, choose **Human vs agent**, select the registered implementation, and
set its action timeout, invalid-action limit, and optional seed. The seed controls only the agent
unless the agent explicitly uses it; it does not replace the separate recorded game seed.

{{< figure src="manual/images/experiment-agent.png" alt="Players and agents section with the participant pairing selector in the experiment configuration" caption="Switch Participant pairing to Human vs agent to reveal the registered agent implementations and their settings." >}}

Give every policy implementation a stable ID and version. Put model name, prompt variant, decoding
parameters, and similar condition variables in the experiment's agent settings. Put credentials in
a secret store owned by the agent service or installation, never in those settings.

## Implement and attach a Python policy

Python policies run as a service that Parlando can select as a player. You need Python 3.10 or
newer. From the repository root:

```sh
python3 -m venv .venv-agent
.venv-agent/bin/python -m pip install --editable parlando-agent-sdk
```

A minimal policy subclasses `Agent`, retains its latest observation, and returns a structured
response:

```python
from parlando_agent_sdk import Agent, Context, Response, serve

class Policy(Agent):
    async def start(self, observation):
        self.observation = observation

    async def observe_transition(self, actor, action, observation):
        self.observation = observation

    async def respond(self, available_actions):
        if available_actions:
            return Response.action(available_actions[0])
        return None

def create_agent(context: Context) -> Agent:
    return Policy()

serve(create_agent, host="127.0.0.1", port=50051)
```

This policy acts only when the game enumerates available actions. A game with a non-enumerated
action space requires policy code that derives a valid typed action from its observation.

Before writing a model-backed policy, attach the included deterministic Great Tree implementation.
It exercises the same agent interface with fewer possible causes of failure:

```sh
.venv-agent/bin/python docs/web/examples/great_tree_python_agent.py
```

Leave that process running. In another terminal, start Great Tree. Then:

1. Create an inactive experiment and open **Configuration → Players and agents**.
2. Choose **Human vs agent**, select **Remote gRPC agent**, and enter
   `http://127.0.0.1:50051` as the endpoint.
3. Leave **Remote configuration** empty for the included policy. For your own policy, enter a YAML
   mapping here; Parlando passes it as `Context.settings`. Do not place credentials in this mapping.
4. Save a new revision and start **Local preview**.
5. Open the participant URL once. The human occupies seat `A`; Parlando constructs the agent in seat
   `B`, so no second browser is needed.
6. Perform a task action and wait for the policy response. Inspect the session timeline for the
   agent identity and response.

The attachment works when the participant reaches the game, the session contains one human and one
agent, and the agent returns an action or message without a startup or response timeout. A
failure before play usually means that the Python service cannot be reached. A failure after an
observation belongs to policy execution; inspect the Python terminal and the recorded agent event.

Once this check passes, replace the included policy body with the model or decision procedure you
need. Keep the lifecycle methods and typed `Response` values. A policy must derive an action from its
observation when the game does not enumerate `available_actions`.

## Move the Python service off the game host

The local HTTP address above is only for a service on the same machine. Across machines, expose the
Python service through HTTPS, list its hostname in `PARLANDO_REMOTE_AGENT_ALLOWED_HOSTS`, and set the
same `PARLANDO_REMOTE_AGENT_TOKEN` in both processes. These are deployment settings, not experiment
settings.

Keep model-provider credentials in the Python process's environment or secret manager. Start with
low session capacity and measure startup time, response latency, error rate, and model-provider
limits before increasing recruitment. Set the Python service and Parlando capacity limits to the
tested load.

## Preserve an interpretable condition

An agent's code, settings, model service, and external model weights may evolve independently.
Record the policy version, model version, deployment identifier, and prompt materials with the
study; Parlando cannot prove that a remote provider served unchanged weights or behavior.

Test role-specific observations for leaks before testing policy quality. Then test construction
failure, timeout, invalid actions, yield, normal completion, and shutdown. Finally pilot the complete
live path, including any communication or speech services, from a participant network.

The [agent API reference](../reference/agents/) documents the Rust and Python interfaces after the
experimental and operational choices above are settled. The next chapter removes the human browser
and uses the same policies in a reproducible [agent-only study](agent-agent/).
