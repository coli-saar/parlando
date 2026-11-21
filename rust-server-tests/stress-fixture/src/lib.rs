//! Deterministic game mechanics shared by the native and WebAssembly stress backends.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[cfg(target_arch = "wasm32")]
mod guest;

/// Admitted player role used to select a sequence counter and label projections.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum StressRole {
    /// First participant.
    A,
    /// Second participant.
    B,
}

impl StressRole {
    /// Returns the position of the role's counter and last sequence.
    fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }

    /// Returns the label carried by observations and action history.
    fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// Complete session state, including bounded action history and representative padding.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StressState {
    counts: [u64; 2],
    last_sequence: [u64; 2],
    complete: bool,
    history: VecDeque<(StressRole, u64)>,
    padding: String,
}

/// Action issued by either a participant or the stress agent.
#[cfg_attr(target_arch = "wasm32", derive(parlando_game_sdk::GameValue))]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StressAction {
    /// Records one attempt and optionally ends the session.
    Mark {
        /// Must exceed the acting role's previous sequence number.
        sequence: u64,
        /// Marks this transition as terminal when true.
        finish: bool,
    },
    /// Exercises trap handling in the component contract tests.
    #[cfg(all(feature = "test-traps", target_arch = "wasm32"))]
    #[serde(rename = "trap")]
    Trap,
    /// Exercises exhaust_fuel handling in the component contract tests.
    #[cfg(all(feature = "test-traps", target_arch = "wasm32"))]
    #[serde(rename = "exhaust_fuel")]
    ExhaustFuel,
    /// Exercises empty_view handling in the component contract tests.
    #[cfg(all(feature = "test-traps", target_arch = "wasm32"))]
    #[serde(rename = "empty_view")]
    EmptyView,
    /// Exercises malformed_view handling in the component contract tests.
    #[cfg(all(feature = "test-traps", target_arch = "wasm32"))]
    #[serde(rename = "malformed_view")]
    MalformedView,
    /// Exercises malformed_metadata handling in the component contract tests.
    #[cfg(all(feature = "test-traps", target_arch = "wasm32"))]
    #[serde(rename = "malformed_metadata")]
    MalformedMetadata,
}

/// Role-specific projection carrying the fields used to verify reconnect snapshots.
#[cfg_attr(target_arch = "wasm32", derive(parlando_game_sdk::GameValue))]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StressObservation {
    role: String,
    counts: [u64; 2],
    last_sequence: [u64; 2],
    complete: bool,
    own_history: Vec<u64>,
    padding: String,
}

impl StressObservation {
    /// Returns the last accepted action sequences for seats A and B.
    pub fn last_sequence(&self) -> [u64; 2] {
        self.last_sequence
    }
}

/// Terminal result carrying the combined action count for both roles.
#[cfg_attr(target_arch = "wasm32", derive(parlando_game_sdk::GameValue))]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StressCompletion {
    /// Number of accepted actions from both players.
    pub total_actions: u64,
}

/// Creates a fixed-shape initial state with untouched counters and empty history.
pub fn initial_state() -> StressState {
    StressState {
        counts: [0, 0],
        last_sequence: [0, 0],
        complete: false,
        history: VecDeque::new(),
        padding: "p".repeat(2048),
    }
}

const HISTORY_LIMIT: usize = 32;

/// Advances the actor's counter and retains at most 32 history entries.
///
/// Returns the fixture rejection reason for a post-terminal action or a sequence
/// that does not exceed the actor's last accepted sequence.
pub fn apply_action(
    state: &StressState,
    action: &StressAction,
    actor: StressRole,
) -> Result<StressState, &'static str> {
    let (sequence, finish) = validate_mark(state, action, actor)?;
    let mut next = state.clone();
    drop(trim_for_mark(&mut next.history));
    record_mark(&mut next, actor, sequence, finish);
    Ok(next)
}

/// Returns the validated sequence and completion flag before any state changes.
fn validate_mark(
    state: &StressState,
    action: &StressAction,
    actor: StressRole,
) -> Result<(u64, bool), &'static str> {
    if state.complete {
        return Err("session_complete");
    }
    let (sequence, finish) = match action {
        StressAction::Mark { sequence, finish } => (sequence, finish),
        #[cfg(all(feature = "test-traps", target_arch = "wasm32"))]
        _ => return Err("test_action_outside_component"),
    };
    if *sequence <= state.last_sequence[actor.index()] {
        return Err("stale_or_duplicate_sequence");
    }
    Ok((*sequence, *finish))
}

/// Removes the oldest entries needed to make room for one new history entry.
fn trim_for_mark(
    history: &mut VecDeque<(StressRole, u64)>,
) -> std::collections::vec_deque::Drain<'_, (StressRole, u64)> {
    let evictions = history.len().saturating_sub(HISTORY_LIMIT - 1);
    history.drain(..evictions)
}

/// Records a validated mark after the caller has made room in the history.
fn record_mark(state: &mut StressState, actor: StressRole, sequence: u64, finish: bool) {
    let index = actor.index();
    state.counts[index] += 1;
    state.last_sequence[index] = sequence;
    state.complete = finish;
    state.history.push_back((actor, sequence));
}

/// Projects an accepted mark in place and restores the prior state if projection fails.
#[cfg(any(test, target_arch = "wasm32"))]
fn apply_action_and_project<T, E: From<&'static str>>(
    state: &mut StressState,
    action: &StressAction,
    actor: StressRole,
    project: impl FnOnce(&StressState) -> Result<T, E>,
) -> Result<T, E> {
    let (sequence, finish) = validate_mark(state, action, actor)?;
    let before = (state.counts, state.last_sequence, state.complete);
    let mut evictions = trim_for_mark(&mut state.history);
    let first = evictions.next();
    // A history produced by this fixture can evict at most one entry.
    let additional: Vec<_> = evictions.collect();
    record_mark(state, actor, sequence, finish);
    let result = project(state);
    if result.is_err() {
        (state.counts, state.last_sequence, state.complete) = before;
        state.history.pop_back();
        for entry in first.into_iter().chain(additional).rev() {
            state.history.push_front(entry);
        }
    }
    result
}

/// Projects the counters, completion flag, and padding for the recipient role.
pub fn observation(state: &StressState, player: StressRole) -> StressObservation {
    StressObservation {
        role: player.as_str().into(),
        counts: state.counts,
        last_sequence: state.last_sequence,
        complete: state.complete,
        own_history: state
            .history
            .iter()
            .filter(|(actor, _)| *actor == player)
            .map(|(_, sequence)| *sequence)
            .collect(),
        padding: state.padding.clone(),
    }
}

/// Summarizes both players' accepted actions once the state is terminal.
pub fn completion(state: &StressState) -> Option<StressCompletion> {
    state.complete.then_some(StressCompletion {
        total_actions: state.counts.iter().sum(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises rejection, a failed terminal projection at capacity, and retry.
    #[test]
    fn guest_projection_preserves_state_on_failure() {
        let mut state = initial_state();
        for sequence in 1..=32 {
            let action = StressAction::Mark {
                sequence,
                finish: false,
            };
            apply_action_and_project(&mut state, &action, StressRole::A, |_| Ok::<_, &str>(()))
                .unwrap();
        }
        let checkpoint = serde_json::to_vec(&state).unwrap();
        let repeated = StressAction::Mark {
            sequence: 32,
            finish: true,
        };
        let error: Result<(), &str> =
            apply_action_and_project(&mut state, &repeated, StressRole::A, |_| {
                panic!("rejected marks must not project")
            });
        assert_eq!(error, Err("stale_or_duplicate_sequence"));
        assert_eq!(serde_json::to_vec(&state).unwrap(), checkpoint);

        let terminal = StressAction::Mark {
            sequence: 1,
            finish: true,
        };
        let expected = apply_action(&state, &terminal, StressRole::B).unwrap();
        let expected_checkpoint = serde_json::to_vec(&expected).unwrap();
        assert_eq!(expected.history.front(), Some(&(StressRole::A, 2)));
        let error: Result<(), &str> =
            apply_action_and_project(&mut state, &terminal, StressRole::B, |candidate| {
                assert_eq!(serde_json::to_vec(candidate).unwrap(), expected_checkpoint);
                Err("projection_failed")
            });
        assert_eq!(error, Err("projection_failed"));
        assert_eq!(serde_json::to_vec(&state).unwrap(), checkpoint);

        apply_action_and_project(&mut state, &terminal, StressRole::B, |_| Ok::<_, &str>(()))
            .unwrap();
        assert_eq!(serde_json::to_vec(&state).unwrap(), expected_checkpoint);
        let error: Result<(), &str> =
            apply_action_and_project(&mut state, &terminal, StressRole::B, |_| {
                panic!("terminal state must reject further marks")
            });
        assert_eq!(error, Err("session_complete"));
        assert_eq!(serde_json::to_vec(&state).unwrap(), expected_checkpoint);
    }
}
