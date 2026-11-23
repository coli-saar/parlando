//! Exports the typed session through bindings generated from the packaged WIT.

/// Exports a [`crate::Game`] as a Parlando game component.
///
/// Invoke once at the crate root. Build the crate as a `cdylib` for
/// `wasm32-unknown-unknown`, then wrap the resulting module with
/// `wasm-tools component new`. On native targets the macro checks the
/// supplied type implements the game trait, allowing ordinary Rust tests.
#[macro_export]
macro_rules! export {
    ($game:ty $(,)?) => {
        const _: fn() = {
            fn check<G: $crate::Game>() {}
            check::<$game>
        };

        #[cfg(target_arch = "wasm32")]
        type __ParlandoGame = $game;

        #[cfg(target_arch = "wasm32")]
        mod __parlando_game_component {
            use super::__ParlandoGame as Game;

            use $crate::__bindings::exports::parlando::game::mechanics::{Frame, Guest, Role};
            use $crate::__session::{Frame as SessionFrame, Session};

            ::std::thread_local! {
                /// Owns the game instance from initialization through the last action.
                static SESSION: ::std::cell::RefCell<Session<Game>> =
                    const { ::std::cell::RefCell::new(Session::new()) };
            }

            /// Connects the generated ABI entry points to the typed game session.
            struct Component;

            impl Guest for Component {
                /// Describes the game's six payload types without constructing a session.
                fn describe() -> String {
                    Session::<Game>::describe()
                }

                /// Starts a new session using the supplied configuration and seed.
                fn init(config: Vec<u8>, seed: u64) -> Result<Frame, String> {
                    SESSION.with_borrow_mut(|session| session.init(&config, seed).map(lower))
                }

                /// Recovers the session from the game's opaque checkpoint.
                fn restore(checkpoint: Vec<u8>) -> Result<(), String> {
                    SESSION.with_borrow_mut(|session| session.restore(&checkpoint))
                }

                /// Translates the participant role and forwards the compact action.
                fn step(actor: Role, action: Vec<u8>) -> Result<Frame, String> {
                    let actor = match actor {
                        Role::A => $crate::Role::A,
                        Role::B => $crate::Role::B,
                    };
                    SESSION.with_borrow_mut(|session| session.step(actor, &action).map(lower))
                }
            }

            /// Moves each encoded field directly into its canonical ABI record.
            fn lower(frame: SessionFrame) -> Frame {
                Frame {
                    checkpoint: frame.checkpoint,
                    view_a: frame.view_a,
                    view_b: frame.view_b,
                    completion: frame.completion,
                    metadata: frame.metadata,
                }
            }

            $crate::__bindings::export!(Component with_types_in $crate::__bindings);
        }
    };
}
