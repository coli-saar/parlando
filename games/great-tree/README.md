# The Great Tree

One player warms limbs as Crown. The other opens thawed roots as Root. The
component keeps the mapping between limbs and roots private and reports success
when three limbs flower.

## Start the game

Install Rust's `wasm32-unknown-unknown` target, `wasm-tools`, and Node.js.
From this directory, run:

```sh
./run.sh
```

Open the dashboard at `http://127.0.0.1:8080/admin` and create an experiment.
Select two humans or choose one of the authored agents, **Idle**, **Root Bot (no LLM)**,
and **LLM (learns as it plays)**. The LLM agent needs an OpenAI credential referenced by the
experiment. The game settings give Crown to seat A by default. Set
`crownSeat: b` in the game YAML to give Crown to seat B.

## Build or run a headless experiment

`./run.sh --build-only` builds the portable package in `dist/great-tree-0.1.0.zip`. It also pins the game in each agent manifest and writes the **Idle** and **Root Bot** ZIPs under `agents/idle/dist/` and `agents/root-bot/dist/`. Upload the game first, followed by the agents you want available on the shared host. The
project's headless launcher registers the authored agents and runs them against
the same built component.

```sh
cargo run --manifest-path server/Cargo.toml --features bundled-agents \
  --bin agent_experiment -- agent-experiment.example.yaml
```

The live launcher defaults to `.local/component.sqlite`. Set
`PARLANDO_DATABASE_URL` to select another component installation, or `PORT`
to change the listener port.
