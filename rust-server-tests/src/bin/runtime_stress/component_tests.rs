//! Production component integration tests for the stress fixture.

use super::*;
use parlando::{
    component::{ComponentValue, GameFrame, LoadedGame},
    ExperimentRunner,
};

/// Loads the independently built fixture component specified by the test command.
fn component_factory() -> LoadedGame {
    let path = std::env::var_os("PARLANDO_TEST_COMPONENT")
        .expect("set PARLANDO_TEST_COMPONENT to the locally built fixture component");
    LoadedGame::load(Path::new(&path)).expect("load fixture component")
}

/// Encodes a typed test input with the same wire codec as local policies.
fn payload<T: Serialize>(value: &T) -> ComponentValue {
    ComponentValue::from_typed(value).unwrap()
}

/// Constructs a fixture action without materializing a JSON tree.
fn mark(sequence: u64, finish: bool) -> ComponentValue {
    payload(&StressAction::Mark { sequence, finish })
}

/// Materializes an externally visible value only for JSON parity assertions.
fn json_view(value: ComponentValue) -> Value {
    serde_json::to_value(value).unwrap()
}

/// Decodes the stress fixture's typed completion.
fn total_actions(value: Option<ComponentValue>) -> Option<u64> {
    value.map(|outcome| {
        outcome
            .decode::<parlando_stress_fixture::StressCompletion>()
            .unwrap()
            .total_actions
    })
}

/// Constructs a failure probe from the test-only guest's tagged action shape.
fn failure(kind: &str) -> ComponentValue {
    #[derive(Serialize)]
    struct Failure<'a> {
        #[serde(rename = "type")]
        kind: &'a str,
    }

    payload(&Failure { kind })
}

/// Verifies real component calls, role views, rule rejection, and checkpoint recovery.
#[test]
#[ignore = "requires a locally built fixture component with test-traps"]
fn component_recovers_the_caller_checkpoint_after_an_abandoned_step_and_trap() {
    let factory = component_factory();
    let game = factory.session();
    let initial = game
        .init(&payload(&BTreeMap::<String, ()>::new()), 7)
        .unwrap();
    let mark_a = mark(4, false);
    let mark_b = mark(1, true);

    let abandoned = game
        .step(&initial, &mark_a, PlayerRole::A)
        .unwrap()
        .unwrap();
    assert_eq!(
        json_view(abandoned.observation(PlayerRole::A))["counts"],
        json!([1, 0])
    );

    assert_eq!(
        json_view(initial.observation(PlayerRole::A))["counts"],
        json!([0, 0])
    );
    assert_eq!(
        json_view(initial.observation(PlayerRole::B))["counts"],
        json!([0, 0])
    );
    assert!(initial.completion().is_none());

    let recovered = game
        .step(&initial, &mark_a, PlayerRole::A)
        .unwrap()
        .unwrap();
    assert_eq!(json_view(recovered.observation(PlayerRole::A))["role"], "A");
    assert_eq!(json_view(recovered.observation(PlayerRole::B))["role"], "B");
    assert_eq!(
        json_view(recovered.observation(PlayerRole::A))["own_history"],
        json!([4])
    );
    assert_eq!(
        json_view(recovered.observation(PlayerRole::B))["own_history"],
        json!([])
    );
    assert_eq!(
        game.step(&recovered, &mark_a, PlayerRole::A)
            .unwrap()
            .err()
            .expect("duplicate sequence must be rejected")
            .to_string(),
        "stale_or_duplicate_sequence"
    );
    assert!(game
        .step(&recovered, &failure("trap"), PlayerRole::B)
        .is_err());

    assert!(game
        .step(&recovered, &failure("empty_view"), PlayerRole::B)
        .is_err());

    // The host accepts in-bounds codec output without walking it. A broken
    // value from this test-only guest fails when it is actually serialized.
    let malformed = game
        .step(&recovered, &failure("malformed_view"), PlayerRole::B)
        .unwrap()
        .unwrap();
    assert!(serde_json::to_value(malformed.observation(PlayerRole::B)).is_err());

    // Metadata is also lazy. An invalid value from a nonconforming guest
    // is omitted when read through the infallible Game interface.
    let action = failure("malformed_metadata");
    let malformed = game
        .step(&recovered, &action, PlayerRole::B)
        .unwrap()
        .unwrap();
    assert_eq!(malformed.transition_metadata(), None);

    // Fuel exhaustion must invalidate the partially mutated instance just like
    // a guest trap. The following terminal move proves reconstruction succeeds.
    let exhausted = game
        .step(&recovered, &failure("exhaust_fuel"), PlayerRole::B)
        .err()
        .expect("a nonterminating guest must exhaust its fuel");
    assert!(format!("{exhausted:#}").contains("fuel"));

    let terminal = game
        .step(&recovered, &mark_b, PlayerRole::B)
        .unwrap()
        .unwrap();
    assert_eq!(total_actions(terminal.completion()), Some(2));
    assert_eq!(
        json_view(terminal.observation(PlayerRole::B))["last_sequence"],
        json!([4, 1])
    );

    let new_game = factory.session();
    let persisted = serde_json::to_value(&recovered).unwrap();
    assert_eq!(persisted["recovery"]["turns"].as_array().unwrap().len(), 1);
    let original_config = persisted["recovery"]["origin"]["Seed"]["config"]
        .as_str()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(original_config).unwrap(),
        json!({})
    );
    let original_action = persisted["recovery"]["turns"][0]["action"]
        .as_str()
        .unwrap();
    assert_eq!(
        original_action,
        r#"{"finish":false,"sequence":4,"type":"mark"}"#
    );
    let checkpoint: GameFrame = serde_json::from_value(persisted).unwrap();
    let rebuilt = new_game
        .step(&checkpoint, &mark_b, PlayerRole::B)
        .unwrap()
        .unwrap();
    assert_eq!(
        total_actions(rebuilt.completion()),
        total_actions(terminal.completion())
    );
    assert_eq!(
        json_view(rebuilt.observation(PlayerRole::A)),
        json_view(terminal.observation(PlayerRole::A))
    );
}

/// Creates an agent which makes one move from each seat and ends play from B.
struct SingleMarkFactory;

/// Issues one action carrying the termination flag for its seat.
struct SingleMark {
    role: PlayerRole,
    sequence: u64,
}

#[async_trait]
impl Agent for SingleMark {
    /// Reads the admitted seat's sequence through its typed observation.
    async fn start(&mut self, initial: ComponentValue) -> Result<()> {
        self.sequence = observed_sequence(&initial, self.role)?;
        anyhow::ensure!(self.sequence == 0);
        Ok(())
    }

    /// Checks the accepted sequence exposed to this seat on every transition.
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        _: ComponentValue,
        observation: ComponentValue,
    ) -> Result<()> {
        let observed = observed_sequence(&observation, self.role)?;
        anyhow::ensure!(observed == self.sequence + u64::from(actor == self.role));
        self.sequence = observed;
        Ok(())
    }

    /// Supplies the fixture's single scheduled action.
    async fn respond(&mut self) -> Result<Option<AgentResponse>> {
        let action = agent_action(StressAction::Mark {
            sequence: self.sequence + 1,
            finish: self.role == PlayerRole::B,
        })?;
        Ok(Some(AgentResponse::action(action)))
    }
}

#[async_trait]
impl AgentFactory for SingleMarkFactory {
    /// Registers the deterministic policy for the parity experiment.
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "single-mark".into(),
            name: "Single mark".into(),
            description: "Marks once from each seat".into(),
            config_fields: vec![],
        }
    }

    /// Builds the scripted policy for its assigned seat.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        Ok(Box::new(SingleMark {
            role: context.role,
            sequence: 0,
        }))
    }

    /// Attributes results to this deterministic policy.
    fn identity(&self, _: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "single-mark".into(),
            version: "1".into(),
        })
    }
}

/// Runs a two-action experiment and reads its structured parity evidence.
async fn run_headless(factory: LoadedGame) -> Result<Value> {
    let directory = tempfile::tempdir()?;
    let output = directory.path().join("output");
    let yaml = directory.path().join("fixture.yaml");
    std::fs::write(
        &yaml,
        format!(
            r#"schema: parlando-agent-experiment/v1
game:
  id: runtime-stress
agents:
  policy:
    factory: single-mark
sessions:
  scenarios:
    - name: parity
      config: {{}}
      seeds: [7]
  seats:
    player_a: policy
    player_b: policy
schedule:
  kind: alternate_every_response
  first: player_a
limits:
  callback_timeout_seconds: 2
  shutdown_timeout_seconds: 1
  session_timeout_seconds: 5
  decisions: 4
  accepted_actions: 2
  messages: 2
  rejected_actions: 2
execution:
  concurrency: 1
output:
  directory: '{}'
  trace: full
"#,
            output.display()
        ),
    )?;
    let metadata = GameMetadata {
        id: "runtime-stress".into(),
        name: "Runtime Stress Fixture".into(),
        version: semver::Version::new(1, 0, 0),
        build_manifest: json!({}),
    };
    let summary = ExperimentRunner::new(metadata, factory)?
        .agent(SingleMarkFactory)?
        .run_yaml(yaml)
        .await?;
    assert_eq!(summary.failed, 0);
    let artifact = std::fs::read_dir(output.join("results"))?
        .next()
        .context("missing parity result")??
        .path();
    Ok(serde_json::from_slice(&std::fs::read(artifact)?)?)
}

/// Verifies headless play delivers the accepted moves and the guest's terminal outcome.
#[tokio::test]
#[ignore = "requires a locally built fixture component"]
async fn component_completes_headless_run() -> Result<()> {
    let result = run_headless(component_factory()).await?;
    assert_eq!(result["status"], "completed");
    assert_eq!(result["accepted_actions"], 2);
    assert_eq!(result["completion"], json!({"total_actions": 2}));
    Ok(())
}

/// Confirms that a serialized frame with a guest checkpoint resumes in a fresh instance.
#[test]
#[ignore = "requires a locally built fixture component"]
fn component_restores_a_periodic_guest_checkpoint() {
    let factory = component_factory();
    let game = factory.session();
    let mut selected = game
        .init(&payload(&BTreeMap::<String, ()>::new()), 7)
        .unwrap();
    for sequence in 1..=32 {
        selected = game
            .step(&selected, &mark(sequence, false), PlayerRole::A)
            .unwrap()
            .unwrap();
    }

    let persisted = serde_json::to_value(&selected).unwrap();
    // The previous guest represented the opaque checkpoint with these exact
    // JSON field names and ordering. Verify it directly before replaying it.
    let previous_history = (1..=32)
        .map(|sequence| format!(r#"["A",{sequence}]"#))
        .collect::<Vec<_>>()
        .join(",");
    let previous_padding = "p".repeat(2048);
    let previous_checkpoint = format!(
        r#"{{"counts":[32,0],"last_sequence":[32,0],"complete":false,"history":[{previous_history}],"padding":"{previous_padding}"}}"#
    );
    assert_eq!(
        serde_json::from_value::<Vec<u8>>(persisted["recovery"]["origin"]["Checkpoint"].clone())
            .unwrap(),
        previous_checkpoint.into_bytes()
    );
    assert_eq!(persisted["recovery"]["turns"], json!([]));
    let restored: GameFrame = serde_json::from_value(persisted).unwrap();
    let replacement = factory.session();
    for role in [PlayerRole::A, PlayerRole::B] {
        assert_eq!(
            json_view(restored.observation(role)),
            json_view(selected.observation(role))
        );
    }
    let terminal = replacement
        .step(&restored, &mark(1, true), PlayerRole::B)
        .unwrap()
        .unwrap();
    let a = json_view(terminal.observation(PlayerRole::A));
    let b = json_view(terminal.observation(PlayerRole::B));
    assert_eq!(a["role"], "A");
    assert_eq!(b["role"], "B");
    // The terminal action evicts A's first move from the 32-action history.
    assert_eq!(a["own_history"], json!((2..=32).collect::<Vec<_>>()));
    assert_eq!(b["own_history"], json!([1]));
    assert_eq!(total_actions(terminal.completion()), Some(33));
}

/// Exercises an independently written guest and interleaves two sessions against one factory.
#[test]
#[ignore = "requires the locally built WAT component"]
fn component_canonical_abi_supports_an_independent_guest() {
    let path = std::env::var_os("PARLANDO_TEST_WAT_COMPONENT")
        .expect("set PARLANDO_TEST_WAT_COMPONENT to the locally built WAT component");
    let factory = LoadedGame::load(Path::new(&path)).unwrap();
    let config = payload(&());
    let first = factory.session();
    let second = factory.session();
    let initial_first = first.init(&config, 7).unwrap();
    let initial_second = second.init(&config, 7).unwrap();
    let view_a = initial_first.observation(PlayerRole::A);
    let view_b = initial_first.observation(PlayerRole::B);
    use parlando::component::{ProjectedValue, Projection, Selection};
    let a = Projection::resolve(
        factory.schema().view_a.clone(),
        &Selection::Fields(&[("A", Selection::U64(()))]),
    )
    .unwrap();
    let b = Projection::resolve(
        factory.schema().view_b.clone(),
        &Selection::Fields(&[("B", Selection::U64(()))]),
    )
    .unwrap();
    // Each role's compiled read stays bound to its declared field after later steps.
    let read = |projection: &Projection<()>, view: &ComponentValue| {
        let mut seen = Vec::new();
        projection
            .read(view, |(), field| {
                if let ProjectedValue::U64(n) = field {
                    seen.push(n);
                }
                Ok(())
            })
            .unwrap();
        seen
    };
    assert!(a.read(&view_b, |_, _| Ok(())).is_err());
    assert_eq!(serde_json::to_value(&view_a).unwrap(), json!({"A":0}));
    assert_eq!(view_b.decode::<BTreeMap<String, u64>>().unwrap()["B"], 0);
    assert_eq!(
        view_a.as_messagepack().as_ptr(),
        initial_first
            .observation(PlayerRole::A)
            .as_messagepack()
            .as_ptr()
    );
    assert_eq!(
        view_b.as_messagepack().as_ptr(),
        initial_first
            .clone()
            .observation(PlayerRole::B)
            .as_messagepack()
            .as_ptr()
    );

    let action = payload(&BTreeMap::from([("try", true)]));
    let terminal_first = first
        .step(&initial_first, &action, PlayerRole::A)
        .unwrap()
        .unwrap();
    assert_eq!(read(&a, &terminal_first.observation(PlayerRole::A)), [1]);
    assert_eq!(read(&b, &terminal_first.observation(PlayerRole::B)), [1]);
    assert_eq!(read(&a, &view_a), [0]);
    assert_eq!(read(&b, &view_b), [0]);
    assert_eq!(
        json_view(terminal_first.observation(PlayerRole::A)),
        json!({"A":1})
    );
    assert_eq!(
        json_view(terminal_first.observation(PlayerRole::B)),
        json!({"B":1})
    );
    assert!(terminal_first
        .completion()
        .unwrap()
        .decode::<bool>()
        .unwrap());
    assert_eq!(
        json_view(initial_second.observation(PlayerRole::B)),
        json!({"B":0})
    );

    assert_eq!(
        terminal_first.transition_metadata(),
        Some(json!({"sum": 1}))
    );

    let recorded = serde_json::to_value(&terminal_first).unwrap();
    assert_eq!(recorded["metadata"], json!({"sum": 1}));
    let reloaded: GameFrame = serde_json::from_value(recorded).unwrap();
    assert_eq!(reloaded.transition_metadata(), Some(json!({"sum": 1})));

    let terminal_second = second
        .step(&initial_second, &action, PlayerRole::B)
        .unwrap()
        .unwrap();
    assert!(terminal_second
        .completion()
        .unwrap()
        .decode::<bool>()
        .unwrap());

    let recovered = factory.session();
    let result = recovered
        .step(&terminal_first, &action, PlayerRole::A)
        .unwrap();
    assert_eq!(result.err().unwrap().to_string(), "already_stepped");
}

/// Runs distinct guest declarations through the same host and checks named inputs and outputs.
#[test]
#[ignore = "requires both v4 fixture components"]
fn one_host_runs_distinct_game_schemas() {
    let stress = component_factory();
    let path = std::env::var_os("PARLANDO_TEST_WAT_COMPONENT").unwrap();
    let independent = LoadedGame::load(Path::new(&path)).unwrap();
    assert_ne!(stress.schema(), independent.schema());
    for (factory, config, action, field) in [
        (&stress, payload(&json!({})), mark(1, true), "counts"),
        (
            &independent,
            payload(&()),
            payload(&json!({"try": true})),
            "A",
        ),
    ] {
        let game = factory.session();
        let initial = game.init(&config, 7).unwrap();
        let state = game
            .step(&initial, &action, PlayerRole::A)
            .unwrap()
            .unwrap();
        let observation = state.observation(PlayerRole::A);
        let public = serde_json::to_value(&observation).unwrap();
        assert!(public.get(field).is_some());
        assert!(state.completion().is_some());
        let persisted: GameFrame =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(persisted.observation(PlayerRole::A)).unwrap(),
            public
        );
    }
}

/// Invalid declarations fail while loading the package, before a session can be created.
#[test]
#[ignore = "requires the independent v4 fixture component"]
fn invalid_guest_declaration_fails_during_load() {
    let path = std::env::var_os("PARLANDO_TEST_WAT_COMPONENT").unwrap();
    let mut bytes = std::fs::read(path).unwrap();
    let marker = br#"{"config":{"type":"null"}"#;
    let start = bytes
        .windows(marker.len())
        .position(|w| w == marker)
        .unwrap();
    bytes[start + marker.len() - 6] = b'?';
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid-schema.wasm");
    std::fs::write(&path, bytes).unwrap();
    let err = LoadedGame::load(&path).err().unwrap();
    assert!(format!("{err:#}").contains("schema"), "{err:#}");
}

/// Verifies that an SDK encoding trap discards the accepted mutation before replay.
#[test]
#[ignore = "requires the locally built SDK encoding-trap component"]
fn sdk_encoding_trap_recovers_the_last_accepted_state() {
    let path = std::env::var_os("PARLANDO_TEST_SDK_COMPONENT")
        .expect("set PARLANDO_TEST_SDK_COMPONENT to the locally built SDK component");
    let factory = LoadedGame::load(Path::new(&path)).unwrap();
    let game = factory.session();
    let initial = game.init(&payload(&()), 7).unwrap();
    let increment = payload(&"increment");
    let accepted = game
        .step(&initial, &increment, PlayerRole::B)
        .unwrap()
        .unwrap();
    assert_eq!(json_view(accepted.observation(PlayerRole::B)), 8);

    let broken = payload(&"invalid_view");
    assert!(game.step(&accepted, &broken, PlayerRole::A).is_err());

    let recovered = game
        .step(&accepted, &increment, PlayerRole::B)
        .unwrap()
        .unwrap();
    assert_eq!(json_view(recovered.observation(PlayerRole::B)), 9);
}
