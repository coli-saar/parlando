# Agent integration

Build the requested policy against the action and observation types of the
game package it will play. An agent occupies one seat and receives that
seat's view. Keep the policy in its own per-session instance. Use the latest
observation delivered by `start` or `observe_transition` in `respond`.
Returned actions go through the game's normal validation.

Implement `observe_message(sender, text)` for policies that respond to dialogue. The server delivers typed messages and finalized speech transcripts through this callback. Return text replies in either mode. The experiment owns speech recognition and synthesis.

For a shared host, run the agent as a remote gRPC service and select
`remote_grpc` in the experiment configuration. Use the public
`parlando_agent_sdk` Python API for a Python service and follow the
[remote-agent guide](https://coli-saar.github.io/parlando/manual/build/agents-and-voice/)
for the endpoint configuration and transport authentication. Model-provider
secrets stay in the agent process. Keep agent settings compatible with the
selected game package. Use `Response.action_with_message` to send both a
move and dialogue in a single Python response.

For an explicitly requested in-process Rust policy, use the public
`parlando::agent::Agent` and `Factory` APIs with a custom host that loads
the same game component. The Rust response constructors are
`Response::action`, `Response::message`, and
`Response::action_and_message`. In either integration, return no response
when the policy declines to act, and release per-session resources in
`shutdown` as needed.

Test the agent against the pinned game interface. Use
[`parlando run`](https://coli-saar.github.io/parlando/manual/build/agent-agent/)
for agent-only evaluations when those are requested.
