# Parlando Agent SDK for Python

This package lets a Python policy participate in a Parlando game through the
remote-agent gRPC interface. The policy receives the same role-specific
observations and available actions as other agent implementations, while the SDK
handles request dispatch and response conversion.

Agent authors implement async observation callbacks such as `observe_state`,
`observe_action`, and `observe_message`, then respond from
`maybe_act(available_actions)`.

Before running the SDK from a checkout, generate the Python protobuf modules:

```sh
python -m parlando_agent_sdk.generate_protos
```

Minimal agent:

```python
from parlando_agent_sdk import AgentResponse, GameAgent, serve_agent

class FirstActionAgent(GameAgent):
    async def maybe_act(self, available_actions):
        if available_actions:
            return AgentResponse.action(available_actions[0])
        return None

serve_agent(FirstActionAgent, host="127.0.0.1", port=50051)
```

See [Agents](../../../docs/agents.md) for experiment configuration, deployment,
authentication, and agent identity/version recording.
