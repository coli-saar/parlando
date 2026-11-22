//! Typed session mechanics and the bridge to the component wire contract.

#[cfg(any(test, target_arch = "wasm32"))]
use crate::{decode_declared, encode_compact, GameSchema};
use crate::{DecodeGameValue, GameValue};

/// Identifies which of the two fixed participants submitted an action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    /// The first participant.
    A,
    /// The second participant.
    B,
}

impl Role {
    /// Returns the seat identifier used by participant protocols.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// The stable, client-visible reason that an action could not be applied.
///
/// A rejected action leaves the game unchanged. Use this for expected rule
/// violations. Unexpected failures may panic, which traps the component and
/// lets the host recover the last accepted state in a fresh instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rejection {
    code: String,
}

impl Rejection {
    /// Creates a rejection with a game-defined reason code such as `wrong_turn`.
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }

    /// Returns the stable reason code sent to the host.
    pub fn code(&self) -> &str {
        &self.code
    }
}

/// Owns the mutable state of one deterministic game session.
///
/// Implement this trait on the state object and invoke [`crate::export!`]
/// once to export the game. The same methods can be called from native Rust
/// tests. Given the same configuration, seed, and accepted actions, the game
/// must produce the same state and projections. Store random state in the game
/// and advance it only on accepted actions.
///
/// Configuration and actions have owned Rust types. Views and optional
/// projections can borrow data from the game, which the SDK encodes before
/// returning to the host. Declare distinct view types to expose different data
/// to the two participants. Use `()` for unused completion or metadata types.
pub trait Game: Sized + 'static {
    /// Configuration decoded once at session initialization.
    type Config: for<'de> DecodeGameValue<'de>;

    /// Action decoded before each call to [`Self::step`].
    type Action: for<'de> DecodeGameValue<'de>;

    /// Public observation available to participant A.
    type ViewA<'a>: GameValue;

    /// Public observation available to participant B.
    type ViewB<'a>: GameValue;

    /// Terminal outcome returned to the host and participants.
    type Completion<'a>: GameValue;

    /// Additional data for the host about an accepted transition.
    type Metadata<'a>: GameValue;

    /// Creates a session from typed configuration and the recorded seed.
    ///
    /// Return an error if the configuration violates the game's constraints.
    fn init(config: Self::Config, seed: u64) -> Result<Self, String>;

    /// Applies an action for the given participant.
    ///
    /// Check rejection conditions before mutating state. The host may reuse
    /// this same instance immediately after a rejection. An accepted action
    /// must update the game before its views and outcome are read.
    fn step(&mut self, actor: Role, action: Self::Action) -> Result<(), Rejection>;

    /// Projects the current state for participant A.
    fn view_a(&self) -> Self::ViewA<'_>;

    /// Projects the current state for participant B.
    fn view_b(&self) -> Self::ViewB<'_>;

    /// Returns the terminal outcome when the game is complete.
    fn completion(&self) -> Option<Self::Completion<'_>> {
        None
    }

    /// Projects metadata for the most recent accepted action.
    ///
    /// Update or clear the data on every accepted action. The SDK emits
    /// metadata only after a step, so initialization needs no metadata.
    fn metadata(&self) -> Option<Self::Metadata<'_>> {
        None
    }

    /// Optionally snapshots all state needed to resume, including random state.
    ///
    /// Return `None` to skip the checkpoint and let the host recover by
    /// deterministic replay. Implement [`Self::restore`] if this can return
    /// `Some`. Checkpoint bytes are private and their format is up to the game.
    fn checkpoint(&self) -> Option<Vec<u8>> {
        None
    }

    /// Reconstructs a session from opaque bytes returned by [`Self::checkpoint`].
    ///
    /// Reject incompatible bytes with an error. The host calls this on a fresh
    /// component instance before replaying any subsequent actions.
    fn restore(_checkpoint: &[u8]) -> Result<Self, String> {
        Err("game does not support checkpoints".into())
    }
}

/// Encodes an accepted state for transfer through the component ABI.
#[doc(hidden)]
#[cfg(any(test, target_arch = "wasm32"))]
pub struct Frame {
    /// Optional complete checkpoint.
    pub checkpoint: Option<Vec<u8>>,
    /// Compact participant A observation.
    pub view_a: Vec<u8>,
    /// Compact participant B observation.
    pub view_b: Vec<u8>,
    /// Compact optional outcome.
    pub completion: Option<Vec<u8>>,
    /// Compact optional transition data.
    pub metadata: Option<Vec<u8>>,
}

#[cfg(any(test, target_arch = "wasm32"))]
impl Frame {
    /// Encodes all projections while the guest state is borrowed.
    fn from_game<G: Game>(game: &G, metadata: bool) -> Self {
        Self {
            checkpoint: game.checkpoint(),
            view_a: encode_compact(&game.view_a()).expect("encoding participant A observation"),
            view_b: encode_compact(&game.view_b()).expect("encoding participant B observation"),
            completion: game
                .completion()
                .map(|value| encode_compact(&value).expect("encoding completion")),
            metadata: metadata
                .then(|| game.metadata())
                .flatten()
                .map(|value| encode_compact(&value).expect("encoding transition metadata")),
        }
    }
}

/// Holds the game instance and applies the ABI's state and error rules.
#[doc(hidden)]
#[cfg(any(test, target_arch = "wasm32"))]
pub struct Session<G>(Option<G>);

#[cfg(any(test, target_arch = "wasm32"))]
impl<G: Game> Session<G> {
    /// Creates an empty session slot owned by the component instance.
    pub const fn new() -> Self {
        Self(None)
    }

    /// Describes the six wire payloads without constructing a game instance.
    pub fn describe() -> String {
        GameSchema::new(
            G::Config::schema(),
            G::Action::schema(),
            G::ViewA::<'static>::schema(),
            G::ViewB::<'static>::schema(),
            G::Completion::<'static>::schema(),
            G::Metadata::<'static>::schema(),
        )
        .to_description()
        .expect("validating game schema")
    }

    /// Decodes configuration and replaces the slot after successful initialization.
    pub fn init(&mut self, config: &[u8], seed: u64) -> Result<Frame, String> {
        let game = G::init(decode_declared(config)?, seed)?;
        let frame = Frame::from_game(&game, false);
        self.0 = Some(game);
        Ok(frame)
    }

    /// Replaces the slot after successfully decoding a checkpoint.
    pub fn restore(&mut self, checkpoint: &[u8]) -> Result<(), String> {
        self.0 = Some(G::restore(checkpoint)?);
        Ok(())
    }

    /// Decodes an action and returns an accepted frame or a client-facing reason.
    ///
    /// Projection failures trap rather than returning a rejection after mutation.
    pub fn step(&mut self, actor: Role, action: &[u8]) -> Result<Frame, String> {
        let game = self
            .0
            .as_mut()
            .expect("game must be initialized or restored");
        let action = decode_declared(action).map_err(|_| "invalid_component_action".to_owned())?;
        game.step(actor, action)
            .map_err(|rejection| rejection.code)?;
        Ok(Frame::from_game(game, true))
    }
}

#[cfg(test)]
mod tests;
