---
title: Attach a remote agent
---

# Attach a remote agent

An agent occupies one player role. It receives that role's observations and sends the same actions a human can submit. Use a service that implements the action and observation types of the exact game build you are studying.

This page uses the Python gRPC SDK. For a Rust policy running inside the
Parlando host, [build an agent ZIP](packages/#build-an-agent-zip) and upload
it after the game. The [Rust policy example](https://github.com/coli-saar/parlando/blob/main/docs/agents.md#write-a-typed-rust-agent)
plays the same Beacon starter used below.

A dialogue-aware agent implements `observe_message(sender, text)` for both communication modes. Typed messages and final speech transcripts reach the same callback. Return a message response for the participant to read in typed mode or hear when speech synthesis is configured. The experiment operator [configures communication](voice/).

## Try a Beacon Relay agent

Install the [Parlando Agent SDK](https://pypi.org/project/parlando-agent-sdk/) in a Python environment.

```sh
python3 -m venv agent-env
agent-env/bin/python -m pip install parlando-agent-sdk==0.7.0
```

Save this as `beacon_agent.py`. It adds one tick whenever its role has the turn, so it can reach any of the starter's targets exactly.

```python
from parlando_agent_sdk import Agent, Response, serve


class BeaconAgent(Agent):
    async def start(self, observation):
        self.observation = observation

    async def observe_transition(self, actor, action, observation):
        self.observation = observation

    async def respond(self, _available_actions=None):
        if self.observation.get("your_turn"):
            return Response.action({"ticks": 1})
        return None


serve(BeaconAgent, host="127.0.0.1", port=50051)
```

Run `agent-env/bin/python beacon_agent.py` on the same computer as Parlando.
In the local dashboard, select a **Human vs agent** condition, choose the remote gRPC factory, and enter `http://127.0.0.1:50051` as its endpoint. Start a local preview and play with one browser. Make sure the role taken by the agent can act and that the game reaches completion.

## Connect from a shared host

Loopback refers to the host machine. For a remote endpoint, the operator must allow its hostname using `PARLANDO_REMOTE_AGENT_ALLOWED_HOSTS`. Use HTTPS and configure `PARLANDO_REMOTE_AGENT_TOKEN` on both Parlando and the agent service. Keep model credentials in the service that calls the model.

Create a new hosted run with the agent condition selected in its configuration. For example, use these fields with the Beacon package after replacing the endpoint with your service.

```yaml
agents:
  mode: human_vs_agent
  human_vs_agent:
    factory: remote_grpc
    config:
      endpoint: https://beacon-agent.example.edu:50051
```

Test both normal play and service failure before opening intake. Record the agent's version and settings with the study. See [run an agent-only evaluation](agent-agent/) to test without browsers.
