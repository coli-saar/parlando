# Parlando Agent SDK for Python

This package is the Python-side wrapper for Parlando remote gRPC agents.

Agent authors can implement async observation callbacks such as `observe_state`, `observe_action`, and `observe_message`, then respond from `maybe_act(available_actions)`. The SDK owns gRPC request handling and response conversion.

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
