//! Component adapter for the shared native fixture mechanics.

use super::{apply_action_and_project, completion, initial_state, observation};
use super::{StressAction, StressCompletion, StressObservation, StressRole, StressState};
use parlando_game_sdk::{decode_declared, encode_compact, GameSchema, GameValue, Schema};
use std::cell::RefCell;

wit_bindgen::generate!({
    path: "../../rust-server/wit",
    world: "game",
});

use exports::parlando::game::mechanics::{Frame, Guest, Role};

thread_local! {
    /// Owns one component instance's checkpoint for its entire session.
    static STATE: RefCell<Option<StressState>> = const { RefCell::new(None) };
}

/// Exports the shared fixture rules through Parlando's component world.
struct Fixture;

impl Guest for Fixture {
    /// Publishes the public names and compact layout for this game build.
    fn describe() -> String {
        GameSchema {
            config: Schema::Dynamic.into(),
            action: StressAction::schema().into(),
            view_a: StressObservation::schema().into(),
            view_b: StressObservation::schema().into(),
            completion: StressCompletion::schema().into(),
            metadata: Schema::Dynamic.into(),
        }
        .to_description()
        .expect("static fixture schema is valid")
    }

    /// Starts the fixture with a fresh checkpoint.
    fn init(_: Vec<u8>, _: u64) -> Result<Frame, String> {
        STATE.with_borrow_mut(|state| commit(state, initial_state()))
    }

    /// Restores a checkpoint before replaying actions against this instance.
    fn restore(checkpoint: Vec<u8>) -> Result<(), String> {
        let restored = serde_json::from_slice(&checkpoint).map_err(|error| error.to_string())?;
        STATE.with_borrow_mut(|state| *state = Some(restored));
        Ok(())
    }

    /// Applies one action and derives every outward projection before committing.
    fn step(actor: Role, action: Vec<u8>) -> Result<Frame, String> {
        let actor = match actor {
            Role::A => StressRole::A,
            Role::B => StressRole::B,
        };
        let action: StressAction =
            decode_declared(&action).map_err(|_| "invalid_fixture_action")?;
        #[cfg(feature = "test-traps")]
        if !matches!(action, StressAction::Mark { .. }) {
            return test_failure(action);
        }
        STATE.with_borrow_mut(|state| {
            let current = state.as_mut().ok_or("missing_fixture_checkpoint")?;
            apply_action_and_project(current, &action, actor, frame)
        })
    }
}

/// Injects failures only in the component built for host recovery tests.
#[cfg(feature = "test-traps")]
fn test_failure(action: StressAction) -> Result<Frame, String> {
    match action {
        StressAction::Trap => {
            STATE.with_borrow_mut(|state| *state = None);
            core::arch::wasm32::unreachable();
        }
        StressAction::ExhaustFuel => {
            STATE.with_borrow_mut(|state| *state = None);
            loop {
                std::hint::spin_loop();
            }
        }
        StressAction::EmptyView => STATE.with_borrow_mut(|state| {
            let mut frame = commit(state, initial_state())?;
            frame.view_b.clear();
            Ok(frame)
        }),
        StressAction::MalformedView => STATE.with_borrow_mut(|state| {
            let mut frame = commit(state, initial_state())?;
            frame.view_b = vec![0x81];
            Ok(frame)
        }),
        StressAction::MalformedMetadata => STATE.with_borrow_mut(|state| {
            let mut frame = commit(state, initial_state())?;
            frame.metadata = Some(vec![0x81]);
            Ok(frame)
        }),
        StressAction::Mark { .. } => Err("expected_test_action".into()),
    }
}

/// Encodes the checkpoint and outward projections before replacing guest state.
fn commit(state: &mut Option<StressState>, next: StressState) -> Result<Frame, String> {
    let frame = frame(&next)?;
    *state = Some(next);
    Ok(frame)
}

/// Derives role-specific views and the terminal outcome from a single checkpoint.
fn frame(state: &StressState) -> Result<Frame, String> {
    Ok(Frame {
        // The fixture normally exercises replay. Take a periodic snapshot to
        // also exercise restoring a component checkpoint in the production host.
        // Keep the fixture's existing JSON persistence representation for restore.
        checkpoint: (state.counts.iter().sum::<u64>() % 32 == 0 && !state.history.is_empty())
            .then(|| serde_json::to_vec(state))
            .transpose()
            .map_err(|error| error.to_string())?,
        view_a: encode_compact(&observation(state, StressRole::A))?,
        view_b: encode_compact(&observation(state, StressRole::B))?,
        completion: completion(state).as_ref().map(encode_compact).transpose()?,
        metadata: None,
    })
}

export!(Fixture);
