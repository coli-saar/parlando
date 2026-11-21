# Parlando Agent SDK for Python

This package lets a Python policy participate in a Parlando game through the
remote-agent gRPC interface. The policy receives the same role-specific views as
other agent implementations. The SDK dispatches requests and converts responses.

Agent authors implement async observation callbacks such as `start`,
`observe_transition`, `observe_message`, and `finish`, then respond from
`respond(available_actions)`. Parlando component hosts pass `None` for that
protocol argument. Read legal moves from the observation when the game includes
them. A factory may receive a small immutable `Context`
containing the player's role, deterministic seed, normalized agent settings, and
the session logger.
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
    async def start(self, observation):
        self.observation = observation

    async def observe_transition(self, actor, action, observation):
        self.observation = observation

    async def respond(self, _available_actions):
        legal_actions = self.observation.get("available_actions", [])
        if legal_actions:
            return Response.action(legal_actions[0])
        return None

def create_agent(context: Context) -> Agent:
    # Expensive model loading may happen here. The initial observation is sent
    # only after this function returns.
    return FirstActionAgent()

serve(create_agent, host="127.0.0.1", port=50051)
```

See [Agents](../docs/agents.md) for experiment configuration and deployment.

Run the SDK unit and protocol-drift suite from an environment containing the package dependencies:

```sh
python -m unittest discover -s tests -v
```
