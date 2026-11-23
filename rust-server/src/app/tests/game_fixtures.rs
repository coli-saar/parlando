use super::*;

/// JSON input emitted by scripted agents.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct TinyAction {
    pub(super) finish: bool,
    #[serde(default)]
    pub(super) invalid: bool,
}

#[derive(Deserialize)]
pub(super) struct TinyObservation {
    pub(super) done: bool,
    pub(super) role: String,
}

#[derive(Deserialize)]
pub(super) struct TinySummary {
    pub(super) outcome: String,
    pub(super) dyad_score: i64,
}

/// Compiles the independently built component once and clones the shared handle.
pub(super) fn live_game() -> LoadedGame {
    static GAME: std::sync::OnceLock<LoadedGame> = std::sync::OnceLock::new();
    GAME.get_or_init(|| {
        LoadedGame::from_bytes(include_bytes!(
            "../../../tests/fixtures/live-game/fixture.wasm"
        ))
        .expect("load live game fixture")
    })
    .clone()
}

/// Encodes scripted input using the same JSON-shaped component transport.
pub(super) fn tiny_action(action: &TinyAction) -> ComponentValue {
    ComponentValue::from_typed(action).expect("encode fixture action")
}
