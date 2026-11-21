//! Creates and shuts down agents and bounds one headless game session.
//!
//! A worker receives one resolved plan, constructs its game and role-bound agents, and
//! delegates play to the decision loop. The session runner turns the loop outcome or a
//! lifecycle failure into a result. Bounded shutdown covers every constructed agent and
//! records secondary errors separately.

use super::decisions::drive_session;
use super::planning::{ResolvedSeat, SessionPlan};
use super::results::{SessionCaptures, SessionResult, SessionStatus};
use super::schedule::other_role;
use super::spec::TraceLevel;
use crate::agents::{Agent, AgentContext};
use crate::game::{Game, GameFactory, GameInitializationContext, GameSessionContext, PlayerRole};
use crate::SessionLogger;
use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// Single-session driver shared by batch evaluation workers.
pub(super) struct SessionRunner<G: Game> {
    pub(super) game_factory: Arc<dyn GameFactory<Game = G>>,
}

impl<G: Game> SessionRunner<G> {
    /// Constructs, drives, cleans up, and summarizes one session exactly once.
    pub(super) async fn run(
        &self,
        plan: SessionPlan<G>,
    ) -> SessionResult<G::Completion, G::Observation, G::Action> {
        let started = Instant::now();
        let mut result = SessionResult {
            run_id: plan.run_id.clone(),
            plan_id: plan.plan_id.clone(),
            phase: plan.phase.clone(),
            phase_index: plan.phase_index,
            scenario: plan.scenario.clone(),
            game_id: plan.game_id.clone(),
            game_version: plan.game_version.clone(),
            configuration_fingerprint: plan.configuration_fingerprint.clone(),
            game_seed: plan.game_seed,
            agents: plan
                .seats
                .iter()
                .map(ResolvedSeat::result_identity)
                .collect(),
            schedule: plan.schedule.kind.clone(),
            status: SessionStatus::Failed,
            completion: None,
            failure_phase: None,
            failure_role: None,
            diagnostic: None,
            decisions: 0,
            accepted_actions: 0,
            rejected_actions: 0,
            messages: 0,
            elapsed_ms: 0,
            captures: SessionCaptures {
                trace: (plan.trace != TraceLevel::Results).then(Vec::new),
                trajectory: Vec::new(),
            },
            cleanup_diagnostics: Vec::new(),
        };
        let logger = SessionLogger::headless();
        let game = match self.game_factory.create(GameSessionContext {
            logger: logger.clone(),
        }) {
            Ok(game) => game,
            Err(error) => {
                record_failure(&mut result, "game_factory", None, error);
                result.elapsed_ms = elapsed_ms(started);
                return result;
            }
        };

        let mut first = match create_agent(
            &plan.seats[0],
            logger.clone(),
            plan.limits.callback_timeout(),
        )
        .await
        {
            Ok(agent) => agent,
            Err(error) => {
                record_failure(&mut result, "agent_factory", Some(PlayerRole::A), error);
                result.elapsed_ms = elapsed_ms(started);
                return result;
            }
        };
        let second =
            match create_agent(&plan.seats[1], logger, plan.limits.callback_timeout()).await {
                Ok(agent) => agent,
                Err(error) => {
                    record_failure(&mut result, "agent_factory", Some(PlayerRole::B), error);
                    if let Some(error) =
                        shutdown_agent(first.as_mut(), plan.limits.shutdown_timeout()).await
                    {
                        result.cleanup_diagnostics.push(error);
                    }
                    result.elapsed_ms = elapsed_ms(started);
                    return result;
                }
            };
        let mut agents = [first, second];

        let state = match game.initial_state(GameInitializationContext {
            config: &plan.game_config,
            seed: plan.game_seed,
            secrets: &plan.game_secrets,
        }) {
            Ok(state) => state,
            Err(error) => {
                record_failure(&mut result, "initial_state", None, error);
                result.cleanup_diagnostics =
                    shutdown_agents(&mut agents, plan.limits.shutdown_timeout()).await;
                result.elapsed_ms = elapsed_ms(started);
                return result;
            }
        };

        let execution = drive_session(&game, state, &mut agents, &plan, &mut result).await;
        match execution {
            Ok(outcome) => {
                result.status = outcome.status;
                result.completion = outcome.completion;
            }
            Err(failure) => record_failure(&mut result, failure.phase, failure.role, failure.error),
        }
        result.cleanup_diagnostics =
            shutdown_agents(&mut agents, plan.limits.shutdown_timeout()).await;
        result.elapsed_ms = elapsed_ms(started);
        result
    }
}

/// Successful exit from the decision loop.
pub(super) struct SessionOutcome<C> {
    pub(super) status: SessionStatus,
    pub(super) completion: Option<C>,
}

/// Structured internal failure preserving lifecycle attribution.
pub(super) struct SessionFailure {
    pub(super) phase: &'static str,
    pub(super) role: Option<PlayerRole>,
    pub(super) error: anyhow::Error,
}

/// Delivers one player message only to the opposite role.
pub(super) async fn deliver_message<G: Game>(
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    sender: PlayerRole,
    message: &str,
    callback_timeout: Duration,
) -> std::result::Result<(), SessionFailure> {
    let receiver = other_role(sender);
    timeout(
        callback_timeout,
        agents[role_index(receiver)].observe_message(sender, message.to_string()),
    )
    .await
    .map_err(|_| callback_timeout_failure("observe_message", receiver))?
    .map_err(|error| callback_failure("observe_message", receiver, error))
}

/// Delivers normal completion to both agents in stable role order.
pub(super) async fn finish_agents<G: Game>(
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    completion: &G::Completion,
    callback_timeout: Duration,
) -> std::result::Result<(), SessionFailure> {
    for role in [PlayerRole::A, PlayerRole::B] {
        timeout(
            callback_timeout,
            agents[role_index(role)].finish(completion.clone()),
        )
        .await
        .map_err(|_| callback_timeout_failure("finish", role))?
        .map_err(|error| callback_failure("finish", role, error))?;
    }
    Ok(())
}

/// Shuts down both successfully constructed agents without replacing the primary result.
async fn shutdown_agents<G: Game>(
    agents: &mut [Box<dyn Agent<G> + Send>; 2],
    shutdown_timeout: Duration,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    for (index, agent) in agents.iter_mut().enumerate() {
        if let Some(error) = shutdown_agent(agent.as_mut(), shutdown_timeout).await {
            let role = if index == 0 { "A" } else { "B" };
            diagnostics.push(format!("role {role}: {error}"));
        }
    }
    diagnostics
}

/// Applies the bounded best-effort shutdown contract to one agent.
async fn shutdown_agent<G: Game>(agent: &mut dyn Agent<G>, duration: Duration) -> Option<String> {
    match timeout(duration, agent.shutdown()).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("shutdown failed: {error:#}")),
        Err(_) => Some("shutdown timed out".to_string()),
    }
}

/// Creates one role-bound agent using its resolved settings and secrets.
async fn create_agent<G: Game>(
    seat: &ResolvedSeat<G>,
    logger: SessionLogger,
    callback_timeout: Duration,
) -> Result<Box<dyn Agent<G> + Send>> {
    timeout(
        callback_timeout,
        seat.factory.create(AgentContext {
            role: seat.role,
            seed: seat.seed,
            settings: seat.settings.clone(),
            factory_secrets: seat.factory_secrets.clone(),
            agent_instance_secrets: seat.agent_instance_secrets.clone(),
            logger,
        }),
    )
    .await
    .context("agent factory timed out")?
}

/// Converts one lifecycle error into the compact public result fields.
fn record_failure<C: Serialize, O, A>(
    result: &mut SessionResult<C, O, A>,
    phase: &str,
    role: Option<PlayerRole>,
    error: impl Into<anyhow::Error>,
) {
    result.status = SessionStatus::Failed;
    result.failure_phase = Some(phase.to_string());
    result.failure_role = role;
    let diagnostic = format!("{:#}", error.into());
    result.diagnostic = Some(diagnostic.chars().take(4096).collect());
}

/// Constructs a successful non-completion loop exit.
pub(super) fn limit_outcome<C>(status: SessionStatus) -> SessionOutcome<C> {
    SessionOutcome {
        status,
        completion: None,
    }
}

/// Builds a role-attributed callback error.
pub(super) fn callback_failure(
    phase: &'static str,
    role: PlayerRole,
    error: anyhow::Error,
) -> SessionFailure {
    SessionFailure {
        phase,
        role: Some(role),
        error,
    }
}

/// Constructs the common timeout diagnostic without exposing callback internals.
pub(super) fn callback_timeout_failure(phase: &'static str, role: PlayerRole) -> SessionFailure {
    SessionFailure {
        phase,
        role: Some(role),
        error: anyhow!("agent callback timed out"),
    }
}

/// Returns elapsed wall time using a saturating millisecond conversion.
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Returns the array index assigned to one public player role.
pub(super) fn role_index(role: PlayerRole) -> usize {
    match role {
        PlayerRole::A => 0,
        PlayerRole::B => 1,
    }
}
