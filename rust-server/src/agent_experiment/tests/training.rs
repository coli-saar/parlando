use super::*;

/// Checkpoint-specific ordinary factory exposed by the mock learner.
struct TinyCheckpointFactory {
    checkpoint: CheckpointId,
    events: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentFactory<TinyGame> for TinyCheckpointFactory {
    /// Exposes the same uniform factory ID used by learner YAML.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "tiny.rl".to_string(),
            name: "Tiny RL agent".to_string(),
            description: "Checkpoint-specific tiny learner policy.".to_string(),
            config_fields: vec![crate::game::AgentConfigField {
                key: "mode".to_string(),
                label: "Mode".to_string(),
                help: "Normalized learner setting used by the runner contract test.".to_string(),
                value: crate::game::AgentConfigValue::String {
                    format: crate::game::StringFormat::Plain,
                },
                required: true,
                default_value: json!("default"),
            }],
        }
    }

    /// Creates the same deterministic action policy for each mock checkpoint.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<TinyGame> + Send>> {
        self.events.lock().unwrap().push(format!(
            "create-rl:{}:{}",
            self.checkpoint.as_str(),
            context.role.as_str()
        ));
        Ok(Box::new(TinyAgent {
            role: context.role,
            acted: false,
            events: self.events.clone(),
        }))
    }

    /// Includes the checkpoint in stable policy provenance.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "TinyRLAgent".to_string(),
            version: self.checkpoint.as_str().to_string(),
        })
    }
}

/// Mock learner which records idempotent updates and creates numbered checkpoints.
struct TinyLearner {
    events: Arc<Mutex<Vec<String>>>,
    updates: Arc<Mutex<Vec<(String, String, usize)>>>,
}

#[async_trait]
impl RLAgent<TinyGame> for TinyLearner {
    /// Uses one stable registry ID with no YAML learner discriminator.
    fn factory_id(&self) -> &str {
        "tiny.rl"
    }

    /// Treats the YAML string as the learner-owned initial checkpoint reference.
    fn resolve_checkpoint(&self, reference: &Value) -> Result<CheckpointId> {
        CheckpointId::new(
            reference
                .as_str()
                .context("tiny checkpoint reference must be a string")?,
        )
    }

    /// Produces an ordinary factory bound to one resolved checkpoint.
    fn factory(&self, checkpoint: &CheckpointId) -> Result<Arc<dyn AgentFactory<TinyGame>>> {
        Ok(Arc::new(TinyCheckpointFactory {
            checkpoint: checkpoint.clone(),
            events: self.events.clone(),
        }))
    }

    /// Records the update and returns its deterministic numbered checkpoint.
    async fn train(
        &mut self,
        context: &RLTrainingContext,
        base: &CheckpointId,
        batch: TrainingBatch,
    ) -> Result<CheckpointId> {
        anyhow::ensure!(
            context.settings["mode"] == "configured",
            "runner did not deliver normalized learner settings"
        );
        let mut updates = self.updates.lock().unwrap();
        updates.push((
            batch.update_id,
            base.as_str().to_string(),
            batch.steps.len(),
        ));
        CheckpointId::new(format!("checkpoint-{}", updates.len()))
    }
}

/// Dense test reward equal to the accepted counter increment for both roles.
struct TinyReward;

impl RewardFunction<TinyGame> for TinyReward {
    /// Returns the YAML registry key.
    fn id(&self) -> &'static str {
        "count_delta"
    }

    /// Returns stable test reward provenance.
    fn version(&self) -> &'static str {
        "1"
    }

    /// Accepts no reward parameters.
    fn validate_parameters(&self, parameters: &Value) -> Result<()> {
        require_empty_parameters(parameters)
    }

    /// Rewards the public counter change symmetrically.
    fn rewards(
        &self,
        before: &TinyState,
        after: &TinyState,
        _actor: PlayerRole,
        _action: &TinyAction,
        _completion: Option<&TinyCompletion>,
        _parameters: &Value,
    ) -> RoleRewards {
        let reward = f64::from(after.count.saturating_sub(before.count));
        RoleRewards {
            player_a: reward,
            player_b: reward,
        }
    }
}

/// Renders a two-epoch learner run with validation after every checkpoint.
fn training_yaml(output: &Path) -> String {
    format!(
        r#"schema: parlando-agent-experiment/v1
game:
  id: tiny
agents:
  learner:
    factory: tiny.rl
    checkpoint: initial
    settings:
      mode: configured
  baseline:
    factory: tiny.agent
training:
  learner: learner
  scenarios:
    - name: train-one-step
      config: {{ target: 1 }}
      seeds: [7]
  seats:
    player_a: learner
    player_b: baseline
  epochs: 2
  checkpoint_every_epochs: 1
  reward:
    kind: count_delta
validation:
  scenarios:
    - name: held-out
      config: {{ target: 1 }}
      seeds: [11]
  seats:
    player_a: learner
    player_b: baseline
  every_checkpoints: 1
schedule:
  kind: alternate_every_response
  first: player_a
limits:
  callback_timeout_seconds: 1
  shutdown_timeout_seconds: 1
  session_timeout_seconds: 5
  decisions: 4
  accepted_actions: 2
  messages: 2
  rejected_actions: 2
output:
  directory: '{}'
"#,
        output.display()
    )
}

/// Builds a runner containing one ordinary opponent, learner, and reward function.
fn training_runner(
    events: Arc<Mutex<Vec<String>>>,
    updates: Arc<Mutex<Vec<(String, String, usize)>>>,
) -> ExperimentRunner<TinyGame> {
    test_runner(events.clone())
        .rl_agent(TinyLearner { events, updates })
        .unwrap()
        .reward(TinyReward)
        .unwrap()
}

/// Exercises training, opaque checkpoints, validation cadence, trajectories, and resume.
#[tokio::test]
async fn training_updates_and_validates_without_repeating_updates_on_resume() {
    let temporary = TempDir::new().unwrap();
    let output = temporary.path().join("training-output");
    let yaml_path = temporary.path().join("training.yaml");
    tokio::fs::write(&yaml_path, training_yaml(&output))
        .await
        .unwrap();
    let updates = Arc::new(Mutex::new(Vec::new()));

    let first = training_runner(Arc::new(Mutex::new(Vec::new())), updates.clone())
        .run_yaml(&yaml_path)
        .await
        .unwrap();
    assert_eq!(first.planned, 4);
    assert_eq!(first.completed, 4);
    assert_eq!(first.checkpoints, 2);
    assert_eq!(
        updates
            .lock()
            .unwrap()
            .iter()
            .map(|(_, base, steps)| (base.as_str(), *steps))
            .collect::<Vec<_>>(),
        [("initial", 1), ("checkpoint-1", 1)]
    );

    let training_result = std::fs::read_dir(output.join("results"))
        .unwrap()
        .filter_map(std::result::Result::ok)
        .map(|entry| std::fs::read(entry.path()).unwrap())
        .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
        .find(|value| value["phase"] == "training")
        .unwrap();
    assert_eq!(training_result["trajectory"][0]["rewards"]["player_a"], 1.0);
    assert_eq!(training_result["trajectory"][0]["accepted"], true);
    assert_eq!(training_result["trajectory"][0]["observation"]["role"], "A");

    let resumed_updates = Arc::new(Mutex::new(Vec::new()));
    let second = training_runner(Arc::new(Mutex::new(Vec::new())), resumed_updates.clone())
        .run_yaml(&yaml_path)
        .await
        .unwrap();
    assert_eq!(second.run_id, first.run_id);
    assert_eq!(second.skipped, 4);
    assert!(resumed_updates.lock().unwrap().is_empty());
}

/// Checkpoint identities reject empty semantic values and preserve opaque content.
#[test]
fn checkpoint_ids_are_nonempty_and_opaque() {
    assert!(CheckpointId::new("").is_err());
    assert!(CheckpointId::new(" \n\t ").is_err());
    let checkpoint = CheckpointId::new("models/run 7/checkpoint:β").unwrap();
    assert_eq!(checkpoint.as_str(), "models/run 7/checkpoint:β");
}

/// Per-role rewards retain fixed game-role meaning instead of acting-seat order.
#[test]
fn role_rewards_map_to_fixed_roles() {
    let rewards = RoleRewards {
        player_a: 1.25,
        player_b: -0.5,
    };
    assert_eq!(rewards.for_role(PlayerRole::A), 1.25);
    assert_eq!(rewards.for_role(PlayerRole::B), -0.5);
}
