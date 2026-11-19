use super::*;

pub(super) struct ScriptedAgent {
    pub(super) script: VecDeque<Option<AgentResponse<TinyAction>>>,
}

#[async_trait]
impl Agent<TinyAdapter> for ScriptedAgent {
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(self.script.pop_front().unwrap_or(None))
    }
}

pub(super) struct ScriptedAgentFactory {
    pub(super) created: AtomicUsize,
    pub(super) scripts: Mutex<VecDeque<Vec<Option<AgentResponse<TinyAction>>>>>,
}

impl ScriptedAgentFactory {
    // Creates a factory that hands one script to each fresh agent instance.
    pub(super) fn new(scripts: Vec<Vec<Option<AgentResponse<TinyAction>>>>) -> Self {
        Self {
            created: AtomicUsize::new(0),
            scripts: Mutex::new(scripts.into()),
        }
    }

    // Returns how many agent instances were created by the server runtime.
    pub(super) fn created_count(&self) -> usize {
        self.created.load(Ordering::SeqCst)
    }
}

// Creates a scripted optional response.
pub(super) fn scripted_response(
    message: Option<&str>,
    action: Option<TinyAction>,
) -> Option<AgentResponse<TinyAction>> {
    match (message, action) {
        (Some(message), Some(action)) => Some(AgentResponse::action_and_message(action, message)),
        (Some(message), None) => Some(AgentResponse::message(message)),
        (None, Some(action)) => Some(AgentResponse::action(action)),
        (None, None) => None,
    }
}

#[async_trait]
impl AgentFactory<TinyAdapter> for ScriptedAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        self.created.fetch_add(1, Ordering::SeqCst);
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(ScriptedAgent {
            script: script.into(),
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

pub(super) struct RecordingActionsAgent {
    pub(super) seen_actions: Arc<Mutex<Vec<Option<Vec<TinyAction>>>>>,
}

#[async_trait]
impl Agent<TinyAdapter> for RecordingActionsAgent {
    async fn respond(
        &mut self,
        available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        self.seen_actions.lock().unwrap().push(available_actions);
        Ok(None)
    }
}

pub(super) struct RecordingActionsAgentFactory {
    pub(super) seen_actions: Arc<Mutex<Vec<Option<Vec<TinyAction>>>>>,
}

#[async_trait]
impl AgentFactory<TinyAdapter> for RecordingActionsAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(RecordingActionsAgent {
            seen_actions: self.seen_actions.clone(),
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

pub(super) struct RecordingObservationsAgent {
    pub(super) observations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Agent<TinyAdapter> for RecordingObservationsAgent {
    async fn start(&mut self, current_observation: TinyObservation) -> Result<()> {
        self.observations.lock().unwrap().push(format!(
            "state:{}:{}",
            current_observation.role, current_observation.done
        ));
        Ok(())
    }

    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: TinyAction,
        resulting_observation: TinyObservation,
    ) -> Result<()> {
        self.observations.lock().unwrap().push(format!(
            "action:{}:{}:{}",
            actor.as_str(),
            action.finish,
            resulting_observation.done
        ));
        Ok(())
    }

    async fn observe_message(&mut self, speaker: PlayerRole, text: String) -> Result<()> {
        self.observations
            .lock()
            .unwrap()
            .push(format!("message:{}:{text}", speaker.as_str()));
        Ok(())
    }

    async fn finish(&mut self, completion: TinySummary) -> Result<()> {
        self.observations.lock().unwrap().push(format!(
            "finish:{}:{}",
            completion.outcome, completion.dyad_score
        ));
        Ok(())
    }

    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

pub(super) struct RecordingObservationsAgentFactory {
    pub(super) observations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentFactory<TinyAdapter> for RecordingObservationsAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(RecordingObservationsAgent {
            observations: self.observations.clone(),
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

pub(super) struct SequencedDecisionAgent {
    pub(super) log: Arc<Mutex<Vec<String>>>,
    pub(super) decisions: usize,
}

#[async_trait]
impl Agent<TinyAdapter> for SequencedDecisionAgent {
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        _action: TinyAction,
        _resulting_observation: TinyObservation,
    ) -> Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("observe_transition:{}", actor.as_str()));
        Ok(())
    }

    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        self.decisions += 1;
        self.log
            .lock()
            .unwrap()
            .push(format!("respond:{}", self.decisions));
        if self.decisions == 1 {
            return Ok(scripted_response(
                None,
                Some(TinyAction {
                    finish: false,
                    invalid: false,
                }),
            ));
        }
        Ok(None)
    }
}

pub(super) struct SequencedDecisionAgentFactory {
    pub(super) log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentFactory<TinyAdapter> for SequencedDecisionAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(SequencedDecisionAgent {
            log: self.log.clone(),
            decisions: 0,
        }))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}
