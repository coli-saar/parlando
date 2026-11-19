use super::*;

pub(super) struct NoopAgent;

#[async_trait]
impl Agent<TinyAdapter> for NoopAgent {
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }
}

pub(super) struct NoopAgentFactory;

pub(super) struct GatedAgentFactory {
    pub(super) entered: Arc<Notify>,
    pub(super) release: Arc<Notify>,
}

pub(super) struct FailingAgentFactory;

pub(super) struct SecretAgentFactory;

pub(super) struct StartFailingAgent {
    pub(super) shutdowns: Arc<AtomicUsize>,
    pub(super) stopped: Arc<Notify>,
}

#[async_trait]
impl Agent<TinyAdapter> for StartFailingAgent {
    /// Fails after construction so the runner must execute its cleanup path.
    async fn start(&mut self, _initial_observation: TinyObservation) -> Result<()> {
        anyhow::bail!("intentional agent start failure")
    }

    /// Cannot be reached after the deliberate start failure.
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        unreachable!("respond must not follow a failed start")
    }

    /// Records cleanup of an agent whose start callback failed.
    async fn shutdown(&mut self) -> Result<()> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        self.stopped.notify_one();
        Ok(())
    }
}

pub(super) struct StartFailingAgentFactory {
    pub(super) shutdowns: Arc<AtomicUsize>,
    pub(super) stopped: Arc<Notify>,
}

/// Holds construction or initialization at a known boundary and records delivered input.
pub(super) struct ReadinessAgentFactory {
    pub(super) gate_construction: bool,
    pub(super) entered: Arc<Notify>,
    pub(super) release: Arc<Notify>,
    pub(super) events: mpsc::UnboundedSender<String>,
}

/// Records initialization, observations, and teardown without autonomous actions.
pub(super) struct ReadinessAgent {
    pub(super) gate_start: bool,
    pub(super) entered: Arc<Notify>,
    pub(super) release: Arc<Notify>,
    pub(super) events: mpsc::UnboundedSender<String>,
}

#[async_trait]
impl AgentFactory<TinyAdapter> for ReadinessAgentFactory {
    /// Uses the same public agent definition as the other lifecycle fixtures.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    /// Optionally blocks construction before returning the owned agent.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        if self.gate_construction {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(Box::new(ReadinessAgent {
            gate_start: !self.gate_construction,
            entered: self.entered.clone(),
            release: self.release.clone(),
            events: self.events.clone(),
        }))
    }

    /// Supplies stable identity metadata without depending on initialization.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

#[async_trait]
impl Agent<TinyAdapter> for ReadinessAgent {
    /// Holds initialization open until the test explicitly releases it.
    async fn start(&mut self, _observation: TinyObservation) -> Result<()> {
        if self.gate_start {
            self.entered.notify_one();
            self.release.notified().await;
        }
        let _ = self.events.send("started".into());
        Ok(())
    }

    /// Records each accepted human action in delivery order.
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: TinyAction,
        _observation: TinyObservation,
    ) -> Result<()> {
        let _ = self
            .events
            .send(format!("action:{}:{}", actor.as_str(), action.finish));
        Ok(())
    }

    /// Records each human message in delivery order.
    async fn observe_message(&mut self, speaker: PlayerRole, text: String) -> Result<()> {
        let _ = self
            .events
            .send(format!("message:{}:{text}", speaker.as_str()));
        Ok(())
    }

    /// Waits for external input without changing the game.
    async fn respond(
        &mut self,
        _actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }

    /// Signals that the runtime finished disposing of the agent.
    async fn shutdown(&mut self) -> Result<()> {
        let _ = self.events.send("shutdown".into());
        Ok(())
    }
}

/// Bounds a fixture handshake without using elapsed time to infer ordering.
pub(super) async fn readiness_signal(signal: &Notify) {
    tokio::time::timeout(Duration::from_secs(5), signal.notified())
        .await
        .expect("readiness fixture did not reach its gate");
}

/// Reads one recorded callback with a diagnostic timeout for stalled execution.
pub(super) async fn readiness_event(events: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("agent did not deliver the expected callback")
        .expect("agent callback recorder closed")
}

#[async_trait]
impl AgentFactory<TinyAdapter> for StartFailingAgentFactory {
    /// Describes the start-failure cleanup fixture.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    /// Creates an agent that fails only when the runner starts it.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(StartFailingAgent {
            shutdowns: self.shutdowns.clone(),
            stopped: self.stopped.clone(),
        }))
    }

    /// Returns stable non-secret identity metadata for the fixture.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

pub(super) struct ShutdownRecordingAgent {
    pub(super) shutdowns: Arc<AtomicUsize>,
}

#[async_trait]
impl Agent<TinyAdapter> for ShutdownRecordingAgent {
    /// Waits for observations without producing autonomous game activity.
    async fn respond(
        &mut self,
        _available_actions: Option<Vec<TinyAction>>,
    ) -> Result<Option<AgentResponse<TinyAction>>> {
        Ok(None)
    }

    /// Records that the runtime cleanly shut the agent down.
    async fn shutdown(&mut self) -> Result<()> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

pub(super) struct ShutdownRecordingAgentFactory {
    pub(super) shutdowns: Arc<AtomicUsize>,
}

#[async_trait]
impl AgentFactory<TinyAdapter> for ShutdownRecordingAgentFactory {
    /// Describes the runtime-only shutdown test agent.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    /// Creates one agent that reports its clean shutdown.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(ShutdownRecordingAgent {
            shutdowns: self.shutdowns.clone(),
        }))
    }

    /// Returns stable non-secret identity metadata for the test agent.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

/// Returns the stable identity shared by runtime-only test factories.
pub(super) fn test_agent_identity() -> Result<AgentIdentity> {
    Ok(AgentIdentity {
        name: "TestAgent".to_string(),
        version: "1".to_string(),
    })
}

#[async_trait]
impl AgentFactory<TinyAdapter> for SecretAgentFactory {
    /// Declares one required factory-purpose credential for readiness tests.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "secret.agent".to_string(),
            name: "Secret agent".to_string(),
            description: "Agent with a required credential.".to_string(),
            config_fields: vec![AgentConfigField {
                key: "api_key".to_string(),
                label: "API key".to_string(),
                help: "Required test credential.".to_string(),
                value: AgentConfigValue::SecretReference {
                    purpose: SecretPurpose::Factory,
                },
                required: true,
                default_value: Value::Null,
            }],
        }
    }

    /// Creates the existing no-op agent; readiness is tested before construction.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(NoopAgent))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

/// Returns minimal metadata for runtime-only test agents.
pub(super) fn test_agent_definition() -> AgentDefinition {
    AgentDefinition {
        id: "test.agent".to_string(),
        name: "Test agent".to_string(),
        description: "Agent used by runtime tests.".to_string(),
        config_fields: Vec::new(),
    }
}

#[async_trait]
impl AgentFactory<TinyAdapter> for NoopAgentFactory {
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        Ok(Box::new(NoopAgent))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

#[async_trait]
impl AgentFactory<TinyAdapter> for GatedAgentFactory {
    /// Describes the agent used to hold construction open during admission tests.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    /// Waits for the test to prove the human has already entered the waiting room.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(Box::new(NoopAgent))
    }

    /// Returns stable non-secret identity metadata for the gated agent.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}

#[async_trait]
impl AgentFactory<TinyAdapter> for FailingAgentFactory {
    /// Describes the agent used to exercise asynchronous construction failure.
    fn definition(&self) -> AgentDefinition {
        test_agent_definition()
    }

    /// Fails construction after the human has already received a waiting session.
    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<TinyAdapter> + Send>> {
        anyhow::bail!("intentional agent construction failure")
    }

    /// Returns stable non-secret identity metadata before construction fails.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        test_agent_identity()
    }
}
