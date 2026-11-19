//! Drives scheduled decisions, action delivery, tracing, and role-safe trajectory capture.

use super::planning::SessionPlan;
use super::results::{SessionResult, SessionStatus};
use super::session::{
    callback_failure, callback_timeout_failure, deliver_message, finish_agents, limit_outcome,
    role_index, SessionFailure, SessionOutcome,
};
use super::spec::TraceLevel;
use super::training::TrajectoryStep;
use crate::agents::{Agent, AgentResponse};
use crate::game::{ActionRejection, Game, PlayerRole};
use anyhow::anyhow;
use serde_json::{json, Value};
use std::time::Instant;
use tokio::time::timeout;

/// Runs initialization callbacks and the ordered decision loop.
pub(super) async fn drive_session<G: Game>(
    game: &G,
    mut state: G::State,
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    plan: &SessionPlan<G>,
    result: &mut SessionResult<G::Completion>,
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
        let decision_input = if capture_training || plan.trace == TraceLevel::Full {
            let phase = if capture_training {
                "trajectory"
            } else {
                "trace"
            };
            Some(DecisionInput {
                observation: serde_json::to_value(game.observation(&state, role)).map_err(
                    |error| SessionFailure {
                        phase,
                        role: Some(role),
                        error: error.into(),
                    },
                )?,
                available_actions: serde_json::to_value(&affordances).map_err(|error| {
                    SessionFailure {
                        phase,
                        role: Some(role),
                        error: error.into(),
                    }
                })?,
            })
        } else {
            None
        };
        let learner_input = capture_training.then(|| {
            decision_input
                .as_ref()
                .expect("training capture constructs a decision input")
        });
        let trace_context = (plan.trace == TraceLevel::Full).then(|| {
            let input = decision_input
                .as_ref()
                .expect("full tracing constructs a decision input");
            json!({
                "observation": &input.observation,
                "available_actions": &input.available_actions,
            })
        });
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
                    .apply_action(&mut state, role, action, message, learner_input)
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
            trace_context,
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

/// Serialized decision input shared by tracing and trajectory capture.
struct DecisionInput {
    observation: Value,
    available_actions: Value,
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
    result: &'a mut SessionResult<G::Completion>,
}

impl<G: Game> DecisionExecution<'_, G> {
    /// Applies one action and delivers its accepted transition and optional message.
    async fn apply_action(
        &mut self,
        state: &mut G::State,
        actor: PlayerRole,
        action: &G::Action,
        message: Option<&str>,
        learner_input: Option<&DecisionInput>,
    ) -> std::result::Result<ActionAttempt<G::Completion>, SessionFailure> {
        let next = match self.game.apply_action(state, action, actor) {
            Ok(next) => next,
            Err(error) => {
                self.result.rejected_actions += 1;
                self.record_trajectory(
                    state,
                    actor,
                    action,
                    learner_input,
                    TrajectoryOutcome::Rejected(error.clone()),
                )?;
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
        )?;
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
        learner_input: Option<&DecisionInput>,
        outcome: TrajectoryOutcome<'_, G>,
    ) -> std::result::Result<(), SessionFailure> {
        let (Some(input), Some(capture)) = (learner_input, self.plan.training_capture.as_ref())
        else {
            return Ok(());
        };
        let (accepted, rejection, completion, rewards) = match outcome {
            TrajectoryOutcome::Accepted { before, completion } => (
                true,
                None,
                completion,
                Some(capture.reward.rewards(
                    before,
                    next_state,
                    actor,
                    action,
                    completion,
                    &capture.parameters,
                )),
            ),
            TrajectoryOutcome::Rejected(error) => (false, Some(error), None, None),
        };
        let next_observation = serde_json::to_value(self.game.observation(next_state, actor))
            .map_err(|error| SessionFailure {
                phase: "trajectory",
                role: Some(actor),
                error: error.into(),
            })?;
        let action = serde_json::to_value(action).map_err(|error| SessionFailure {
            phase: "trajectory",
            role: Some(actor),
            error: error.into(),
        })?;
        self.result.trajectory.push(TrajectoryStep {
            run_id: self.plan.run_id.clone(),
            plan_id: self.plan.plan_id.clone(),
            decision: self.result.decisions,
            scenario: self.plan.scenario.clone(),
            role: actor,
            agent: capture.learner.clone(),
            checkpoint: capture.checkpoint.clone(),
            reward: capture.reward_kind.clone(),
            reward_version: capture.reward_version.clone(),
            observation: input.observation.clone(),
            available_actions: input.available_actions.clone(),
            action,
            accepted,
            rejection,
            rewards,
            next_observation,
            terminal: completion.is_some(),
        });
        Ok(())
    }
}

/// Adds one structured response entry when tracing is enabled.
fn record_trace<G: Game>(
    result: &mut SessionResult<G::Completion>,
    role: PlayerRole,
    response: Option<&AgentResponse<G::Action>>,
    rejection: Option<&ActionRejection>,
    context: Option<Value>,
) {
    let Some(trace) = result.trace.as_mut() else {
        return;
    };
    let response = match response {
        None => json!({"type": "yield"}),
        Some(AgentResponse::Action(action)) => json!({"type": "action", "action": action}),
        Some(AgentResponse::Message(message)) => json!({"type": "message", "message": message}),
        Some(AgentResponse::ActionAndMessage { action, message }) => {
            json!({"type": "action_and_message", "action": action, "message": message})
        }
    };
    let mut entry = json!({
        "decision": result.decisions,
        "role": role,
        "response": response,
        "rejection": rejection,
    });
    if let (Some(entry), Some(context)) = (entry.as_object_mut(), context) {
        entry.insert("input".to_string(), context);
    }
    trace.push(entry);
}
