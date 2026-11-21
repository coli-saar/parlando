//! Drives scheduled decisions, action delivery, tracing, and role-safe trajectory capture.
//!
//! The session runner supplies initialized agents, game state, and a schedule. The loop
//! delivers messages and private observations, counts decisions and accepted or rejected
//! actions, and captures learner steps for training sessions. A move that completes the game
//! at the action limit yields completion.

use super::planning::SessionPlan;
use super::results::{
    DecisionInput, DecisionTrace, SessionResult, SessionStatus, TraceInput, TraceResponse,
};
use super::session::{
    callback_failure, callback_timeout_failure, deliver_message, finish_agents, limit_outcome,
    role_index, SessionFailure, SessionOutcome,
};
use super::spec::TraceLevel;
use super::training::TrajectoryStep;
use crate::agents::{Agent, AgentResponse};
use crate::game::{ActionRejection, Game, PlayerRole};
use anyhow::anyhow;
use std::time::Instant;
use tokio::time::timeout;

/// Starts both agents with role-specific observations, then asks one scheduled agent
/// per decision. Completion on the last permitted move wins over the action limit.
/// Reaching a limit records its own terminal status. Two yields under the default
/// alternating schedule end the run as quiescent, without a game completion.
pub(super) async fn drive_session<G: Game>(
    game: &G,
    mut state: G::State,
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    plan: &SessionPlan<G>,
    result: &mut SessionResult<G::Completion, G::Observation, G::Action>,
) -> std::result::Result<SessionOutcome<G::Completion>, SessionFailure> {
    for role in [PlayerRole::A, PlayerRole::B] {
        let observation = game.observation(&state, role);
        timeout(
            plan.limits.callback_timeout(),
            agents[role_index(role)].start(observation),
        )
        .await
        .map_err(|_| callback_timeout_failure("start", role))?
        .map_err(|error| callback_failure("start", role, error))?;
    }

    if let Some(completion) = game.completion(&state) {
        finish_agents(agents, &completion, plan.limits.callback_timeout()).await?;
        return Ok(SessionOutcome {
            status: SessionStatus::Completed,
            completion: Some(completion),
        });
    }

    let mut schedule =
        (plan.schedule_factory)(plan.schedule.first.role(), &plan.schedule.parameters).map_err(
            |error| SessionFailure {
                phase: "schedule",
                role: None,
                error,
            },
        )?;
    let mut role = schedule.first();
    let deadline = Instant::now() + plan.limits.session_timeout();

    loop {
        if Instant::now() >= deadline {
            return Ok(limit_outcome(SessionStatus::Deadline));
        }
        if result.decisions >= plan.limits.decisions {
            return Ok(limit_outcome(SessionStatus::DecisionLimit));
        }
        result.decisions += 1;
        let affordances = game.available_actions(&state, role);
        let capture_training = plan
            .training_capture
            .as_ref()
            .is_some_and(|capture| capture.learner == plan.seats[role_index(role)].name);
        let full_trace = plan.trace == TraceLevel::Full;
        let input = (capture_training || full_trace).then(|| DecisionInput {
            observation: game.observation(&state, role),
            available_actions: affordances.clone(),
        });
        let (learner_input, trace_input) = if capture_training {
            let index = result.captures.trajectory.len();
            (input, full_trace.then_some(TraceInput::Trajectory(index)))
        } else {
            (None, input.map(TraceInput::Decision))
        };
        let response = timeout(
            plan.limits.callback_timeout(),
            agents[role_index(role)].respond(affordances),
        )
        .await
        .map_err(|_| callback_timeout_failure("respond", role))?
        .map_err(|error| callback_failure("respond", role, error))?;

        if learner_input.is_some() && !matches!(response, Some(AgentResponse::Action(_))) {
            return Err(SessionFailure {
                phase: "training_response",
                role: Some(role),
                error: anyhow!(
                    "the v1 RL trajectory contract requires action-only learner responses"
                ),
            });
        }

        let mut rejection = None;
        let mut completed = None;
        if let Some(response) = response.as_ref() {
            let (action, message) = match response {
                AgentResponse::Action(action) => (Some(action), None),
                AgentResponse::Message(message) => (None, Some(message.as_str())),
                AgentResponse::ActionAndMessage { action, message } => {
                    (Some(action), Some(message.as_str()))
                }
            };
            if let Some(action) = action {
                let mut execution = DecisionExecution {
                    game,
                    agents,
                    plan,
                    result,
                };
                match execution
                    .attempt_action(&mut state, role, action, message, learner_input)
                    .await?
                {
                    ActionAttempt::Accepted(completion) => completed = completion,
                    ActionAttempt::Rejected(error) => rejection = Some(error),
                }
            } else if let Some(message) = message {
                deliver_message(agents, role, message, plan.limits.callback_timeout()).await?;
                result.messages += 1;
            }
        }
        record_trace::<G>(
            result,
            role,
            response.as_ref(),
            rejection.as_ref(),
            trace_input,
        );

        if let Some(completion) = completed {
            finish_agents(agents, &completion, plan.limits.callback_timeout()).await?;
            return Ok(SessionOutcome {
                status: SessionStatus::Completed,
                completion: Some(completion),
            });
        }
        if result.accepted_actions >= plan.limits.accepted_actions {
            return Ok(limit_outcome(SessionStatus::ActionLimit));
        }
        if result.messages >= plan.limits.messages {
            return Ok(limit_outcome(SessionStatus::MessageLimit));
        }
        if result.rejected_actions >= plan.limits.rejected_actions {
            return Ok(limit_outcome(SessionStatus::RejectionLimit));
        }
        let Some(next) = schedule.next(role, response.as_ref(), rejection.as_ref()) else {
            return Ok(limit_outcome(SessionStatus::Quiescent));
        };
        role = next;
    }
}

/// Result of applying one syntactically valid agent action.
enum ActionAttempt<C> {
    Accepted(Option<C>),
    Rejected(ActionRejection),
}

/// Private context shared by one decision's runner operations.
struct DecisionExecution<'a, G: Game> {
    game: &'a G,
    agents: &'a mut [Box<dyn Agent<G> + Send>; 2],
    plan: &'a SessionPlan<G>,
    result: &'a mut SessionResult<G::Completion, G::Observation, G::Action>,
}

impl<G: Game> DecisionExecution<'_, G> {
    /// Attempts one game step and delivers its accepted transition and optional message.
    async fn attempt_action(
        &mut self,
        state: &mut G::State,
        actor: PlayerRole,
        action: &G::Action,
        message: Option<&str>,
        learner_input: Option<DecisionInput<G::Observation, G::Action>>,
    ) -> std::result::Result<ActionAttempt<G::Completion>, SessionFailure> {
        let next = match self
            .game
            .step(state, action, actor)
            .map_err(|error| SessionFailure {
                phase: "step",
                role: Some(actor),
                error,
            })? {
            Ok(next) => next,
            Err(error) => {
                self.result.rejected_actions += 1;
                self.record_trajectory(
                    state,
                    actor,
                    action,
                    learner_input,
                    TrajectoryOutcome::Rejected(error.clone()),
                );
                return Ok(ActionAttempt::Rejected(error));
            }
        };
        self.result.accepted_actions += 1;
        for role in [PlayerRole::A, PlayerRole::B] {
            let observation = self.game.observation(&next, role);
            timeout(
                self.plan.limits.callback_timeout(),
                self.agents[role_index(role)].observe_transition(
                    actor,
                    action.clone(),
                    observation,
                ),
            )
            .await
            .map_err(|_| callback_timeout_failure("observe_transition", role))?
            .map_err(|error| callback_failure("observe_transition", role, error))?;
        }
        let completion = self.game.completion(&next);
        self.record_trajectory(
            &next,
            actor,
            action,
            learner_input,
            TrajectoryOutcome::Accepted {
                before: state,
                completion: completion.as_ref(),
            },
        );
        *state = next;
        if completion.is_none() {
            if let Some(message) = message {
                deliver_message(
                    self.agents,
                    actor,
                    message,
                    self.plan.limits.callback_timeout(),
                )
                .await?;
                self.result.messages += 1;
            }
        }
        Ok(ActionAttempt::Accepted(completion))
    }
}

/// Accepted or rejected transition data needed by trajectory capture.
enum TrajectoryOutcome<'a, G: Game> {
    Accepted {
        before: &'a G::State,
        completion: Option<&'a G::Completion>,
    },
    Rejected(ActionRejection),
}

impl<G: Game> DecisionExecution<'_, G> {
    /// Records one learner transition using only role-safe observations and numeric rewards.
    fn record_trajectory(
        &mut self,
        next_state: &G::State,
        actor: PlayerRole,
        action: &G::Action,
        learner_input: Option<DecisionInput<G::Observation, G::Action>>,
        outcome: TrajectoryOutcome<'_, G>,
    ) {
        let (Some(input), Some(capture)) = (learner_input, self.plan.training_capture.as_ref())
        else {
            return;
        };
        let (accepted, rejection, completion, rewards) = match outcome {
            TrajectoryOutcome::Accepted { before, completion } => (
                true,
                None,
                completion,
                Some(
                    capture
                        .reward
                        .rewards(before, next_state, actor, action, completion),
                ),
            ),
            TrajectoryOutcome::Rejected(error) => (false, Some(error), None, None),
        };
        self.result.captures.trajectory.push(TrajectoryStep {
            run_id: self.plan.run_id.clone(),
            plan_id: self.plan.plan_id.clone(),
            decision: self.result.decisions,
            scenario: self.plan.scenario.clone(),
            role: actor,
            agent: capture.learner.clone(),
            checkpoint: capture.checkpoint.clone(),
            reward: capture.reward_kind.clone(),
            reward_version: capture.reward_version.clone(),
            observation: input.observation,
            available_actions: input.available_actions,
            action: action.clone(),
            accepted,
            rejection,
            rewards,
            next_observation: self.game.observation(next_state, actor),
            terminal: completion.is_some(),
        });
    }
}

/// Adds one structured response entry when tracing is enabled.
fn record_trace<G: Game>(
    result: &mut SessionResult<G::Completion, G::Observation, G::Action>,
    role: PlayerRole,
    response: Option<&AgentResponse<G::Action>>,
    rejection: Option<&ActionRejection>,
    input: Option<TraceInput<G::Observation, G::Action>>,
) {
    let Some(trace) = result.captures.trace.as_mut() else {
        return;
    };
    let response = match response {
        None => TraceResponse::Yield,
        Some(AgentResponse::Action(action)) => TraceResponse::Action {
            action: action.clone(),
        },
        Some(AgentResponse::Message(message)) => TraceResponse::Message {
            message: message.clone(),
        },
        Some(AgentResponse::ActionAndMessage { action, message }) => {
            TraceResponse::ActionAndMessage {
                action: action.clone(),
                message: message.clone(),
            }
        }
    };
    trace.push(DecisionTrace {
        decision: result.decisions,
        role,
        response,
        rejection: rejection.cloned(),
        input,
    });
}
