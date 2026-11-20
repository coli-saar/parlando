//! Selects the next agent from the preceding response and rejection.
//!
//! The decision loop asks the schedule for an initial role and the next role after each
//! response. A schedule can hand control to the partner, keep the current role, or end the
//! session when neither role will act.

use crate::agents::AgentResponse;
use crate::game::{ActionRejection, Game, PlayerRole};
use anyhow::{bail, Result};
use serde_json::Value;

/// Chooses which software agent receives each headless decision opportunity.
pub trait AgentSchedule<G: Game>: Send {
    /// Returns the role selected for the first call to `respond`.
    fn first(&mut self) -> PlayerRole;

    /// Returns the next role, or `None` when the session is quiescent.
    fn next(
        &mut self,
        previous: PlayerRole,
        response: Option<&AgentResponse<G::Action>>,
        rejection: Option<&ActionRejection>,
    ) -> Option<PlayerRole>;
}

/// Alternates after every response and stops after both agents yield consecutively.
pub struct AlternateEveryResponse {
    first: PlayerRole,
    previous_yield: Option<PlayerRole>,
}

impl AlternateEveryResponse {
    /// Creates the schedule with one deterministic first role.
    pub fn new(first: PlayerRole) -> Self {
        Self {
            first,
            previous_yield: None,
        }
    }
}

impl<G: Game> AgentSchedule<G> for AlternateEveryResponse {
    /// Returns the configured first role.
    fn first(&mut self) -> PlayerRole {
        self.first
    }

    /// Alternates after output and recognizes two consecutive yields as quiescence.
    fn next(
        &mut self,
        previous: PlayerRole,
        response: Option<&AgentResponse<G::Action>>,
        _rejection: Option<&ActionRejection>,
    ) -> Option<PlayerRole> {
        if response.is_none() {
            if self.previous_yield == Some(other_role(previous)) {
                return None;
            }
            self.previous_yield = Some(previous);
        } else {
            self.previous_yield = None;
        }
        Some(other_role(previous))
    }
}

/// Keeps control after messages and rejections, and alternates after actions or yields.
pub struct AlternateAfterAction {
    first: PlayerRole,
    previous_yield: Option<PlayerRole>,
}

impl AlternateAfterAction {
    /// Creates the schedule with one deterministic first role.
    pub fn new(first: PlayerRole) -> Self {
        Self {
            first,
            previous_yield: None,
        }
    }
}

impl<G: Game> AgentSchedule<G> for AlternateAfterAction {
    /// Returns the configured first role.
    fn first(&mut self) -> PlayerRole {
        self.first
    }

    /// Applies the action-oriented handoff rule and detects two-agent yielding.
    fn next(
        &mut self,
        previous: PlayerRole,
        response: Option<&AgentResponse<G::Action>>,
        rejection: Option<&ActionRejection>,
    ) -> Option<PlayerRole> {
        match response {
            None => {
                if self.previous_yield == Some(other_role(previous)) {
                    return None;
                }
                self.previous_yield = Some(previous);
                Some(other_role(previous))
            }
            Some(AgentResponse::Message(_)) => {
                self.previous_yield = None;
                Some(previous)
            }
            Some(AgentResponse::Action(_)) | Some(AgentResponse::ActionAndMessage { .. }) => {
                self.previous_yield = None;
                if rejection.is_some() {
                    Some(previous)
                } else {
                    Some(other_role(previous))
                }
            }
        }
    }
}

/// Returns the other role in Parlando's fixed two-player model.
pub(super) fn other_role(role: PlayerRole) -> PlayerRole {
    match role {
        PlayerRole::A => PlayerRole::B,
        PlayerRole::B => PlayerRole::A,
    }
}

/// Rejects parameters for built-in schedules which deliberately have none.
pub(super) fn require_empty_parameters(parameters: &Value) -> Result<()> {
    if parameters
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        Ok(())
    } else {
        bail!("this agent schedule accepts no parameters")
    }
}
