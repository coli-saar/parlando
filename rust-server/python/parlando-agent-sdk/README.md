# Parlando Agent SDK for Python

This package lets a Python policy participate in a Parlando game through the
remote-agent gRPC interface. The policy receives the same role-specific
observations and available actions as other agent implementations, while the SDK
handles request dispatch and response conversion.

Agent authors implement async observation callbacks such as `start`,
`observe_transition`, and `observe_message`, then respond from
`respond(available_actions)`. A factory may receive a small immutable `Context`
containing the player's role, deterministic seed, and agent-specific settings.
It receives no room identity, transport details, frontend data, or initial game
observation.

Before running the SDK from a checkout, generate the Python protobuf modules:

```sh
python -m parlando_agent_sdk.generate_protos
```

Minimal agent:

```python
from parlando_agent_sdk import Agent, Context, Response, serve

class FirstActionAgent(Agent):
    async def respond(self, available_actions):
        if available_actions:
            return Response.action(available_actions[0])
        return None

def create_agent(context: Context) -> Agent:
    # Expensive model loading may happen here. The initial observation is sent
    # only after this function returns.
    return FirstActionAgent()

serve(create_agent, host="127.0.0.1", port=50051)
```

See [Agents](../../../docs/agents.md) for experiment configuration, deployment,
authentication, and agent identity/version recording.

Run the SDK unit and protocol-drift suite from an environment containing the package dependencies:

```sh
python -m unittest discover -s tests -v
```
