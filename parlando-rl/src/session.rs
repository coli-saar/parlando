//! Runs one game against a session-local opponent until the learner can respond.

use anyhow::{ensure, Context as _, Result};
use parlando::agent::{
    grpc::action_from_wire_json, with_callback_timeout, Agent, Context, Decision, Factory, Message,
    Observation, Reason, Response,
};
use parlando::component::{ComponentValue, GameFrame, GameSession, LoadedGame};
use parlando::{
    ActionRejection, AgentSchedule, AlternateAfterAction, AlternateEveryResponse, PlayerRole,
    SessionLimits, SessionLogCapture, SessionLogger, SessionStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

/// Learner responses contain only action and chat. Both absent means yield.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Action {
    action: Option<Value>,
    message: Option<String>,
}

impl Action {
    /// Builds the same compact action consumed by the game's host adapter.
    pub(super) fn decision(self) -> Result<Decision> {
        let action = self.action.map(action_from_wire_json).transpose()?;
        Ok(Decision {
            action,
            message: self.message,
            wakeup: None,
        })
    }
}

/// The scheduler and pending activation state for both seats.
enum Schedule {
    Headless(Box<dyn AgentSchedule>),
    Live(VecDeque<(PlayerRole, Reason)>),
}

impl Schedule {
    /// Selects the normal runner scheduler or queues both seats' initial live activation.
    fn new(name: &str, first: PlayerRole) -> Result<(Self, (PlayerRole, Reason))> {
        let mut headless: Box<dyn AgentSchedule> = match name {
            "alternate_after_action" => Box::new(AlternateAfterAction::new(first)),
            "alternate_every_response" => Box::new(AlternateEveryResponse::new(first)),
            "live" => {
                let pending = VecDeque::from([(other(first), Reason::SessionStarted)]);
                return Ok((Self::Live(pending), (first, Reason::SessionStarted)));
            }
            _ => anyhow::bail!("unknown schedule {name:?}"),
        };
        let first = headless.first();
        Ok((Self::Headless(headless), (first, Reason::Scheduled)))
    }

    /// Selects the next activation using Parlando's headless handoff or live delivery triggers.
    fn next(
        &mut self,
        role: PlayerRole,
        learner: PlayerRole,
        response: Option<&Response>,
        rejection: Option<&ActionRejection>,
        delivered: [bool; 2],
    ) -> Option<(PlayerRole, Reason)> {
        match self {
            Self::Headless(schedule) => {
                let next = schedule.next(role, response, rejection)?;
                let reason = rejection
                    .filter(|_| role == next)
                    .map_or(Reason::Scheduled, |error| {
                        Reason::ActionRejected(error.code.clone())
                    });
                Some((next, reason))
            }
            Self::Live(pending) => {
                if let Some(error) = rejection {
                    return Some((role, Reason::ActionRejected(error.code.clone())));
                }
                // When both seats received a delivery, the recipient acts before the producer.
                for receiver in [other(role), role] {
                    if !delivered[index(receiver)] {
                        continue;
                    }
                    if let Some((_, reason)) =
                        pending.iter_mut().find(|(seat, _)| *seat == receiver)
                    {
                        // The live inbox drains observations before deciding. A delivery
                        // supersedes activation but cannot supersede a game rejection.
                        if !matches!(reason, Reason::ActionRejected(_)) {
                            *reason = Reason::AfterObservation;
                        }
                    } else {
                        pending.push_back((receiver, Reason::AfterObservation));
                    }
                }
                // A human could act again when no agent has a pending callback.
                pending.pop_front().or(Some((learner, Reason::Scheduled)))
            }
        }
    }
}

/// Unstarted rollout. Models, configuration, and seed are fixed before callbacks begin.
pub(super) struct Setup {
    game: LoadedGame,
    factory: Arc<dyn Factory>,
    context: Context,
    config: ComponentValue,
    learner: PlayerRole,
    schedule: Schedule,
    next: (PlayerRole, Reason),
    limits: SessionLimits,
}

impl Setup {
    /// Resolves the first activation and stores the freshly configured episode.
    pub(super) fn new(
        game: LoadedGame,
        factory: Arc<dyn Factory>,
        context: Context,
        config: ComponentValue,
        learner: PlayerRole,
        schedule: &str,
        first: PlayerRole,
        limits: SessionLimits,
    ) -> Result<Self> {
        let (schedule, next) = Schedule::new(schedule, first)?;
        Ok(Self {
            game,
            factory,
            context,
            config,
            learner,
            schedule,
            next,
            limits,
        })
    }

    /// Creates one guest and starts one newly constructed opponent on its private view.
    pub(super) async fn start(mut self) -> Result<Episode> {
        let game = self.game.session();
        let state = game.init(&self.config, self.context.seed)?;
        let (logger, logs) = SessionLogger::capture_headless();
        self.context.logger = logger;
        let timeout = self.limits.callback_timeout();
        let mut opponent = callback(timeout, "create", self.factory.create(self.context)).await?;
        if let Err(error) = callback(
            timeout,
            "start",
            opponent.start(state.observation(other(self.learner))),
        )
        .await
        {
            let _ = callback(
                self.limits.shutdown_timeout(),
                "shutdown",
                opponent.shutdown(),
            )
            .await;
            return Err(error);
        }
        let deadline = Instant::now() + self.limits.session_timeout();
        let completion = state.completion();
        let mut episode = Episode {
            game,
            initial_view: Some(state.observation(self.learner)),
            state,
            learner: self.learner,
            opponent: Some(opponent),
            deadline,
            schedule: self.schedule,
            next: Some(self.next),
            limits: self.limits,
            counters: Counters::default(),
            events: Vec::new(),
            transitions: Vec::new(),
            logs,
            pending_rejections: [None, None],
            awaiting_learner: false,
            status: None,
        };
        if let Some(completion) = completion {
            episode.finish(completion).await?;
        }
        Ok(episode)
    }
}

/// A nonpublic reward input containing both role views for one committed move.
#[derive(Serialize)]
struct RewardTransition {
    before: RoleViews,
    after: RoleViews,
    actor: PlayerRole,
    action: ComponentValue,
    completion: Option<ComponentValue>,
}

/// Named game perspectives used only by server-side reward calculation.
#[derive(Serialize)]
struct RoleViews {
    #[serde(rename = "A")]
    a: ComponentValue,
    #[serde(rename = "B")]
    b: ComponentValue,
}

impl RoleViews {
    /// Shares the game's cached projections for the supplied frame.
    fn from_frame(frame: &GameFrame) -> Self {
        Self {
            a: frame.observation(PlayerRole::A),
            b: frame.observation(PlayerRole::B),
        }
    }
}

/// Decision and delivery totals over both seats for this episode.
#[derive(Default, Serialize)]
struct Counters {
    decisions: u64,
    accepted_actions: u64,
    rejected_actions: u64,
    messages: u64,
}

/// Session-local game, activation state, agent, and pending observations.
pub(super) struct Episode {
    game: GameSession,
    state: GameFrame,
    learner: PlayerRole,
    opponent: Option<Box<dyn Agent + Send>>,
    deadline: Instant,
    schedule: Schedule,
    next: Option<(PlayerRole, Reason)>,
    limits: SessionLimits,
    counters: Counters,
    initial_view: Option<ComponentValue>,
    events: Vec<Observation>,
    transitions: Vec<RewardTransition>,
    logs: SessionLogCapture,
    pending_rejections: [Option<String>; 2],
    awaiting_learner: bool,
    status: Option<SessionStatus>,
}

impl Episode {
    /// Runs the selected opponent callbacks until the learner is eligible or the episode ends.
    pub(super) async fn advance(&mut self) -> Result<String> {
        let result = self.pump().await;
        if result.is_err() {
            self.status = Some(SessionStatus::Failed);
            self.shutdown().await;
        }
        result
    }

    /// Starts decision opportunities in runner order and returns at the learner boundary.
    async fn pump(&mut self) -> Result<String> {
        while self.status.is_none() {
            if Instant::now() >= self.deadline {
                self.status = Some(SessionStatus::Deadline);
                break;
            }
            if self.counters.decisions >= self.limits.decisions {
                self.status = Some(SessionStatus::DecisionLimit);
                break;
            }
            let Some((role, reason)) = self.next.take() else {
                self.status = Some(SessionStatus::Quiescent);
                break;
            };
            self.counters.decisions += 1;
            let reason = self.pending_rejections[index(role)]
                .take()
                .map_or(reason, Reason::ActionRejected);
            if role == self.learner {
                self.awaiting_learner = true;
                return self.observation(reason);
            }
            let timeout = self.callback_budget();
            let opponent = self
                .opponent
                .as_mut()
                .expect("active rollout owns its opponent");
            let decision = callback(timeout, "decide", opponent.decide(reason)).await?;
            self.apply(role, decision).await?;
        }
        self.shutdown().await;
        self.observation(Reason::Scheduled)
    }

    /// Applies one learner decision before resuming opponent activations.
    pub(super) async fn step(&mut self, decision: Decision) -> Result<String> {
        ensure!(
            self.awaiting_learner,
            "the session is not waiting for a learner decision"
        );
        self.awaiting_learner = false;
        if Instant::now() >= self.deadline {
            self.status = Some(SessionStatus::Deadline);
        } else if let Err(error) = self.apply(self.learner, decision).await {
            self.status = Some(SessionStatus::Failed);
            self.shutdown().await;
            return Err(error);
        }
        self.advance().await
    }

    /// Applies one agent decision, updating the game, delivery queues, and limits.
    async fn apply(&mut self, role: PlayerRole, decision: Decision) -> Result<()> {
        ensure!(
            decision.wakeup.is_none(),
            "wakeup requests are unsupported in OpenEnv rollouts"
        );
        let response = Response::from_decision(decision);
        let (action, message) = response
            .clone()
            .map(Response::into_parts)
            .unwrap_or_default();
        let mut delivered = [false; 2];
        let mut rejection = None;
        if let Some(action) = action {
            match self.game.step(&self.state, &action, role)? {
                Ok(state) => {
                    self.counters.accepted_actions += 1;
                    let completion = state.completion();
                    let message = message.filter(|_| completion.is_none());
                    self.transitions.push(RewardTransition {
                        before: RoleViews::from_frame(&self.state),
                        after: RoleViews::from_frame(&state),
                        actor: role,
                        action: action.clone(),
                        completion: completion.clone(),
                    });
                    self.state = state;
                    for receiver in [PlayerRole::A, PlayerRole::B] {
                        let event = self.state.transition_observation(
                            receiver,
                            role,
                            &action,
                            message.as_deref(),
                        );
                        self.deliver(receiver, event).await?;
                        delivered[index(receiver)] = true;
                    }
                    self.counters.messages += u64::from(message.is_some());
                    if let Some(completion) = completion {
                        return self.finish(completion).await;
                    }
                }
                Err(error) => {
                    self.counters.rejected_actions += 1;
                    self.pending_rejections[index(role)] = Some(error.code.clone());
                    rejection = Some(error);
                }
            }
        } else if let Some(text) = message {
            let receiver = other(role);
            self.deliver(
                receiver,
                Observation {
                    transition: None,
                    message: Some(Message { sender: role, text }),
                },
            )
            .await?;
            self.counters.messages += 1;
            delivered[index(receiver)] = true;
        }
        self.status = self.limits.reached_after_response(
            self.counters.accepted_actions,
            self.counters.messages,
            self.counters.rejected_actions,
        );
        if self.status.is_none() {
            self.next = self.schedule.next(
                role,
                self.learner,
                response.as_ref(),
                rejection.as_ref(),
                delivered,
            );
            if self.next.is_none() {
                self.status = Some(SessionStatus::Quiescent);
            }
        }
        Ok(())
    }

    /// Buffers the learner's delivery or calls the opponent using its own private projection.
    async fn deliver(&mut self, role: PlayerRole, event: Observation) -> Result<()> {
        if role == self.learner {
            self.events.push(event);
        } else {
            let timeout = self.callback_budget();
            let opponent = self
                .opponent
                .as_mut()
                .expect("active rollout owns its opponent");
            callback(timeout, "observe", opponent.observe(event)).await?;
        }
        Ok(())
    }

    /// Delivers the game's terminal result before the opponent is shut down.
    async fn finish(&mut self, completion: ComponentValue) -> Result<()> {
        self.status = Some(SessionStatus::Completed);
        let timeout = self.callback_budget();
        let opponent = self
            .opponent
            .as_mut()
            .expect("active rollout owns its opponent");
        let result = callback(timeout, "finish", opponent.finish(completion)).await;
        self.shutdown().await;
        result
    }

    /// Shuts down the opponent once without replacing the episode outcome.
    pub(super) async fn shutdown(&mut self) {
        if let Some(mut opponent) = self.opponent.take() {
            let _ = callback(
                self.limits.shutdown_timeout(),
                "shutdown",
                opponent.shutdown(),
            )
            .await;
        }
    }

    /// Caps transport callbacks by both their own timeout and the episode deadline.
    fn callback_budget(&self) -> Duration {
        self.limits
            .callback_timeout()
            .min(self.deadline.saturating_duration_since(Instant::now()))
    }

    /// Takes pending events and reward inputs at a single Python observation boundary.
    fn observation(&mut self, reason: Reason) -> Result<String> {
        let (reason, rejection_code) = match reason {
            Reason::SessionStarted => ("session_started", None),
            Reason::Scheduled => ("scheduled", None),
            Reason::AfterObservation => ("after_observation", None),
            Reason::ActionRejected(code) => ("action_rejected", Some(code)),
            Reason::WakeupDue => unreachable!("headless environments do not schedule wakeups"),
        };
        let events = std::mem::take(&mut self.events).into_iter().map(|event| {
            json!({
                "transition": event.transition.map(|transition| json!({
                    "actor": transition.actor, "action": transition.action, "view": transition.view
                })),
                "message": event.message.map(|message| json!({
                    "from_role": message.sender, "text": message.text
                })),
            })
        }).collect::<Vec<_>>();
        Ok(serde_json::to_string(&json!({
            "initial_view": self.initial_view.take(),
            "view": self.state.observation(self.learner),
            "events": events,
            "reason": reason,
            "rejection_code": rejection_code,
            "done": self.status.is_some(),
            "metadata": {
                "completion": self.state.completion(),
                "counters": self.counters,
                "logs": self.logs.drain(),
                "status": self.status,
            },
            "transitions": std::mem::take(&mut self.transitions),
        }))?)
    }
}

/// Enforces the host callback deadline, including the matching gRPC deadline.
async fn callback<T>(
    budget: Duration,
    phase: &str,
    call: impl Future<Output = Result<T>>,
) -> Result<T> {
    with_callback_timeout(budget, call)
        .await
        .with_context(|| format!("opponent {phase} timed out"))?
        .with_context(|| format!("opponent {phase} failed"))
}

/// Returns the opposing seat in a two-role game.
pub(super) fn other(role: PlayerRole) -> PlayerRole {
    match role {
        PlayerRole::A => PlayerRole::B,
        PlayerRole::B => PlayerRole::A,
    }
}

/// Converts a role into its array slot.
fn index(role: PlayerRole) -> usize {
    usize::from(role == PlayerRole::B)
}
