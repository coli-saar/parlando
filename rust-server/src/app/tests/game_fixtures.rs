use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct TinyState {
    pub(super) done: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct TinyAction {
    pub(super) finish: bool,
    #[serde(default)]
    pub(super) invalid: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct TinyObservation {
    pub(super) done: bool,
    pub(super) role: String,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct TinySummary {
    pub(super) done: bool,
    pub(super) outcome: String,
    pub(super) dyad_score: i64,
    pub(super) player_scores: BTreeMap<String, i64>,
}

#[derive(Clone)]
pub(super) struct TinyAdapter;

#[derive(Clone)]
pub(super) struct NoAvailableActionsAdapter;

#[derive(Clone)]
pub(super) struct LossSummaryAdapter;

impl crate::GameFactory for TinyAdapter {
    type Game = TinyAdapter;

    /// Creates one stateless tiny game for a test session.
    fn create(&self, _context: crate::GameSessionContext) -> Result<TinyAdapter> {
        Ok(TinyAdapter)
    }
}

impl crate::GameFactory for NoAvailableActionsAdapter {
    type Game = NoAvailableActionsAdapter;

    /// Creates one stateless affordance game for a test session.
    fn create(&self, _context: crate::GameSessionContext) -> Result<NoAvailableActionsAdapter> {
        Ok(NoAvailableActionsAdapter)
    }
}

impl crate::GameFactory for LossSummaryAdapter {
    type Game = LossSummaryAdapter;

    /// Creates one stateless loss-summary game for a test session.
    fn create(&self, _context: crate::GameSessionContext) -> Result<LossSummaryAdapter> {
        Ok(LossSummaryAdapter)
    }
}

impl Game for TinyAdapter {
    type Config = Value;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinySummary;

    fn initial_state(
        &self,
        _context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        Ok(TinyState { done: false })
    }

    fn apply_action(
        &self,
        _state: &Self::State,
        action: &Self::Action,
        _actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        if action.invalid {
            return Err(ActionRejection::new("invalid_tiny_action"));
        }
        Ok(TinyState {
            done: action.finish,
        })
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyObservation {
            done: state.done,
            role: player.as_str().to_string(),
        }
    }

    fn available_actions(
        &self,
        _state: &Self::State,
        _player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(vec![TinyAction {
            finish: true,
            invalid: false,
        }])
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        state.done.then(|| TinySummary {
            done: state.done,
            outcome: "success".to_string(),
            dyad_score: 10,
            player_scores: BTreeMap::from([("A".to_string(), 6), ("B".to_string(), 4)]),
        })
    }
}

impl Game for NoAvailableActionsAdapter {
    type Config = Value;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinySummary;

    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        TinyAdapter.initial_state(context)
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        TinyAdapter.apply_action(state, action, actor)
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyAdapter.observation(state, player)
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        TinyAdapter.completion(state)
    }
}

impl Game for LossSummaryAdapter {
    type Config = Value;
    type State = TinyState;
    type Action = TinyAction;
    type Observation = TinyObservation;
    type Completion = TinySummary;

    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State> {
        TinyAdapter.initial_state(context)
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection> {
        TinyAdapter.apply_action(state, action, actor)
    }

    fn observation(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        TinyAdapter.observation(state, player)
    }

    fn available_actions(
        &self,
        state: &Self::State,
        player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        TinyAdapter.available_actions(state, player)
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        TinyAdapter.completion(state).map(|mut completion| {
            completion.outcome = "loss".to_string();
            completion.dyad_score = 0;
            completion
                .player_scores
                .extend([("A".to_string(), 0), ("B".to_string(), 0)]);
            completion
        })
    }
}
