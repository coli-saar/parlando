# The Great Tree

One player warms limbs as Crown. The other opens thawed roots as Root. The
component keeps the mapping between limbs and roots private and reports success
when three limbs flower.

## Build and deploy

From this directory, build the packages, then upload them to a running Parlando server. The build requires Rust’s `wasm32-unknown-unknown` target, `wasm-tools`, Python 3, and Node.js. Replace `lab` with your server URL or configured alias.

```sh
./build.sh
parlando login --server lab
./deploy.sh --host lab
```

Sign in once per server. The deploy script uploads the game first, followed by its agents. Omit `--host` to use your CLI’s default server. Open the game from **Games** in the server dashboard to configure an experiment and start intake.

The upload includes **Idle** and **Root Bot**. The native experiment runner also supports **LLM (learns as it plays)** with an OpenAI credential. The game settings give Crown to seat A by default. Set
`crownSeat: b` in the game YAML to give Crown to seat B.

## Build or run a headless experiment

After building the packages, run the example experiment from this directory.

```sh
cargo run --manifest-path server/Cargo.toml --features bundled-agents \
  --bin agent_experiment -- agent-experiment.example.yaml
```
