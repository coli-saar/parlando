# Parlando Agent SDK for Python

This package is the Python-side wrapper for Parlando remote gRPC agents.

Agent authors should implement an async `GameAgent.act(observation, available_actions)` method and start the service with `serve_agent(...)`. The SDK owns gRPC request handling and result conversion.

Before running the SDK from a checkout, generate the Python protobuf modules:

```sh
python -m parlando_agent_sdk.generate_protos
```

Minimal agent:

```python
from parlando_agent_sdk import AgentResult, GameAgent, serve_agent

class FirstActionAgent(GameAgent):
    async def act(self, observation, available_actions):
        if available_actions:
            return AgentResult.action(available_actions[0])
        return AgentResult.none()

serve_agent(FirstActionAgent, host="127.0.0.1", port=50051)
```
