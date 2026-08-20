# Parlando Rust game reference

Consume the published `parlando` crate. The supported author API is `Game`, `GameMetadata`, `PlayerRole`, `ActionRejection`, `Server`, and the optional `agent` namespace.

Use the same discovered Parlando release as the browser client. Do not substitute the obsolete `parlando-server` package, which implements the removed 0.2 API.

## Contract

```rust
pub trait Game: Send + Sync + 'static {
    type Config;
    type State;
    type Action;
    type Observation;
    type Completion;

    fn validate_config(&self, config: &Self::Config) -> anyhow::Result<()>;
    fn initial_state(&self, context: GameInitializationContext<'_, Self::Config>) -> anyhow::Result<Self::State>;
    fn apply_action(&self, state: &Self::State, action: &Self::Action, actor: PlayerRole)
        -> Result<Self::State, ActionRejection>;
    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation;
    fn available_actions(&self, state: &Self::State, role: PlayerRole)
        -> Option<Vec<Self::Action>>;
    fn transition_metadata(&self, before: &Self::State, after: &Self::State,
        action: &Self::Action, actor: PlayerRole) -> Option<serde_json::Value>;
    fn completion(&self, state: &Self::State) -> Option<Self::Completion>;
}
```

Associated types must satisfy the serialization and clone bounds in the actual trait. `State` is authoritative and runtime-private. `Observation` is complete for one role. `Action` is deserialized automatically and, once accepted, delivered to both player roles alongside their role-specific observations. `apply_action` combines authorization, validation, and transition and performs no I/O. `available_actions` is optional guidance. `transition_metadata` is structured, role-neutral analysis data. `completion` atomically expresses terminal detection and a shared result safe for both roles.

Read configuration and seed from `GameInitializationContext`; game-owned experiment secrets are available through its redacting `secrets.get(key)` lookup. Never copy a value into state, logs, or serialized configuration.

Do not define `Event`, expose state, split validation from application, or render human prose in mechanics.

## Server

```rust
let metadata = GameMetadata {
    id: "<game-slug>".into(),
    name: "<Game Name>".into(),
    version: env!("CARGO_PKG_VERSION").parse()?,
    build_manifest: serde_json::json!({"version": env!("CARGO_PKG_VERSION")}),
};

Server::new(MyGame, metadata)?
    .database_url(cli.database_url)
    .participant_app(cli.client_dist)
    .agent(DemoAgentFactory)?
    .serve(SocketAddr::new(cli.host, cli.port))
    .await?;
```

Omit `.agent` when not needed. The game binary must not import experiment configuration, pairing, session limits, providers, storage, routers, or `test_support`.

## Tests

Test seeded initialization, config rejection, every action and actor combination, A/B observation privacy, `available_actions`, every terminal result, metadata serialization, and determinism. When feasible, add a smoke test through the public protocol.
