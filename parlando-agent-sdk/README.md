# Parlando Agent SDK for Python

This package lets a Python policy participate in a Parlando game through the
remote-agent gRPC interface. The policy receives the same role-specific views as
other agent implementations. The SDK dispatches requests and converts responses.

Implement the async `start`, `observe`, and `decide` callbacks to play. Read legal
moves from the view when the game includes them. Implement `finish` to receive the
game result. The factory receives a `Context` with the player's role, deterministic
seed, normalized agent settings, and session logger. The host delivers the initial
view through `start` after the factory returns.

Before running the SDK from a checkout, generate the Python protobuf modules:

```sh
python -m parlando_agent_sdk.generate_protos
```

Minimal agent:

```python
from parlando_agent_sdk import Agent, Context, Decision, serve

class FirstActionAgent(Agent):
    async def start(self, observation):
        self.observation = observation

    async def observe(self, observation):
        if observation.transition is not None:
            self.observation = observation.transition.view

    async def decide(self, reason):
        legal_actions = self.observation.get("available_actions", [])
        if legal_actions:
            return Decision(action=legal_actions[0])
        return Decision()

def create_agent(context: Context) -> Agent:
    # Expensive model loading may happen here. The initial observation is sent
    # only after this function returns.
    return FirstActionAgent()

serve(create_agent, host="127.0.0.1", port=50051)
```

An observation can contain an accepted transition, a message from the other player,
or both. The transition contains the complete role-specific view after the move.
The decision reason identifies live session start, observation delivery, headless
scheduling, timer expiry, or rejection of a proposed action. Rejection reasons include
the game's rejection code.

Live agents can return a `Decision` with `wakeup=WakeupRequest.after(delay)`, where
`delay` is a `datetime.timedelta`. This replaces any pending timer. Return
`WakeupRequest.cancel()` to cancel it. Headless experiments use scheduled decision
opportunities and reject timer requests.

See [Attach a remote agent](https://coli-saar.github.io/parlando/manual/build/agents-and-voice/) for configuration and deployment.

Run the SDK unit and protocol-drift suite from an environment containing the package dependencies:

```sh
python -m unittest discover -s tests -v
```
