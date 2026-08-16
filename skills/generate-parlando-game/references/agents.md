# Parlando agents reference

Agents control A or B with the same observations, actions, and rules as humans. They never receive authoritative state or frontend events.

## Rust

Implement `parlando::agent::Agent<G>` and `Factory<G>`. `Factory::definition` supplies dashboard metadata. `create(Context { role, seed, settings })` is async and constructs one session-local agent before game delivery. `identity(settings)` supplies semantic name/version.

Implement callbacks as needed:

```rust
async fn start(&mut self, initial_observation: Observation) -> Result<()>;
async fn observe_transition(&mut self, actor: PlayerRole, action: Action,
                            observation: Observation) -> Result<()>;
async fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()>;
async fn finish(&mut self, completion: Completion) -> Result<()>;
async fn respond(&mut self, available_actions: Option<Vec<Action>>)
    -> Result<Option<Response<Action>>>;
```

Return `Response::action`, `Response::message`, or `Response::action_and_message`. Return `None` to decline. A message communicates only with the other player and does not change state. Returned actions pass normal validation.

Register factories with `Server::agent`. Register remote support with `agent::grpc::Factory::<G>::new()`; endpoint and identity fields are dashboard-selected settings interpreted by that factory.

## Python

```python
from parlando_agent_sdk import Agent, Response, serve

class MyAgent(Agent):
    async def start(self, initial_observation):
        self.observation = initial_observation

    async def observe_transition(self, actor, action, observation):
        self.observation = observation

    async def observe_message(self, sender, text):
        self.last_message = (sender, text)

    async def finish(self, completion):
        self.completion = completion

    async def respond(self, available_actions):
        if available_actions:
            return Response.action(available_actions[0])
        return None

serve(MyAgent, host="127.0.0.1", port=50051)
```

Generate protobuf bindings from the installed SDK. Keep LLM credentials and prompts in the remote process. If dialogue matters, test `observe_message` and a later response. Do not encode readiness as a player message: explicit initialization readiness is deferred runtime work.
