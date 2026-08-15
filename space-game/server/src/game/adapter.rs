use anyhow::{bail, Result};
use parlando_server::{
    AgentConfigFieldDescriptor, AgentFactoryDescriptor, GameAdapter, PlayerRole,
};
use serde_json::Value;

use super::state_engine::{
    apply_action, available_actions, derive_systems, initial_state, validate_action,
    FilteredKnowledge, SpaceAction, SpaceEvent, SpaceGameState, SpaceObservation, SpaceSummary,
};

#[derive(Clone, Debug, Default)]
/// Connects the Space Game state engine to the reusable Parlando runtime.
pub struct SpaceGameAdapter;

impl SpaceGameAdapter {
    /// Creates the stateless adapter used by every experiment runtime in this process.
    pub fn new() -> Self {
        Self
    }
}

impl GameAdapter for SpaceGameAdapter {
    type State = SpaceGameState;
    type Action = SpaceAction;
    type Observation = SpaceObservation;
    type Event = SpaceEvent;
    type Summary = SpaceSummary;

    fn initial_state(&self) -> Self::State {
        initial_state()
    }

    /// Advertises the Space Game agents compiled into this server binary.
    fn agent_factories(&self) -> Vec<AgentFactoryDescriptor> {
        vec![
            AgentFactoryDescriptor {
                id: "space_game.back_and_forth".to_string(),
                display_name: "Back and forth".to_string(),
                description: "Built-in deterministic Space Game agent; useful for local experiments and testing.".to_string(),
                config_fields: Vec::new(),
            },
            AgentFactoryDescriptor {
                id: "remote_grpc".to_string(),
                display_name: "Remote gRPC agent".to_string(),
                description: "Connects to an external agent process using the Parlando agent protocol.".to_string(),
                config_fields: vec![
                    AgentConfigFieldDescriptor {
                        key: "endpoint".to_string(),
                        label: "Agent endpoint".to_string(),
                        help: "gRPC endpoint of the external agent process, for example http://127.0.0.1:50051.".to_string(),
                        kind: "url".to_string(),
                        required: true,
                        default_value: Value::String("http://127.0.0.1:50051".to_string()),
                    },
                    AgentConfigFieldDescriptor {
                        key: "agent_name".to_string(),
                        label: "Agent name".to_string(),
                        help: "Name sent to the external process when a session is created.".to_string(),
                        kind: "text".to_string(),
                        required: false,
                        default_value: Value::String("space-game-remote-agent".to_string()),
                    },
                    AgentConfigFieldDescriptor {
                        key: "agent_version".to_string(),
                        label: "Agent version".to_string(),
                        help: "Optional version recorded with the agent participant.".to_string(),
                        kind: "text".to_string(),
                        required: false,
                        default_value: Value::Null,
                    },
                    AgentConfigFieldDescriptor {
                        key: "protocol_version".to_string(),
                        label: "Protocol version".to_string(),
                        help: "Agent protocol expected by the external process.".to_string(),
                        kind: "text".to_string(),
                        required: false,
                        default_value: Value::String("parlando-agent-v2".to_string()),
                    },
                ],
            },
        ]
    }

    /// Rejects game-owned options because the current Space Game has no such parameters.
    fn validate_config(&self, config: &Value) -> Result<()> {
        if !config.as_object().is_some_and(|options| options.is_empty()) {
            bail!("Space Game currently has no game-specific options; use an empty YAML mapping")
        }
        Ok(())
    }

    fn validate_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Result<()> {
        validate_action(state, action, player.as_str())
    }

    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> Result<Self::State> {
        apply_action(state, action)
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        SpaceObservation {
            players: state.players.clone(),
            fuses: state.fuses.clone(),
            breakers: state.breakers.clone(),
            valves: state.valves.clone(),
            override_held: state.override_held,
            battery: state.battery.clone(),
            relay: state.relay.clone(),
            pressure_drained: state.pressure_drained,
            oxygen_fan_tripped: state.oxygen_fan_tripped,
            visual_effects: state.visual_effects.clone(),
            beacon_launched: state.beacon_launched,
            role: player.as_str().to_string(),
            systems: derive_systems(state),
            knowledge: FilteredKnowledge {
                a: if player == PlayerRole::A {
                    state.knowledge.a.clone()
                } else {
                    vec![]
                },
                b: if player == PlayerRole::B {
                    state.knowledge.b.clone()
                } else {
                    vec![]
                },
            },
            private_knowledge: if player == PlayerRole::A {
                state.knowledge.a.clone()
            } else {
                state.knowledge.b.clone()
            },
            log: vec![],
        }
    }

    fn available_actions(
        &self,
        state: &Self::State,
        player: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        Some(available_actions(state, player.as_str()))
    }

    fn events_for_action(
        &self,
        before: &Self::State,
        after: &Self::State,
        action: &Self::Action,
        player: PlayerRole,
    ) -> Vec<Self::Event> {
        let actor = action.player();
        let mut events = vec![];
        if matches!(actor, Some("A" | "B")) {
            events.push(SpaceEvent {
                event_type: "action".to_string(),
                actor: actor.map(str::to_string),
                text: event_text(action, actor == Some(player.as_str())),
            });
        }
        let before_items = if player == PlayerRole::A {
            &before.knowledge.a
        } else {
            &before.knowledge.b
        };
        let after_items = if player == PlayerRole::A {
            &after.knowledge.a
        } else {
            &after.knowledge.b
        };
        for item in after_items {
            if !before_items.contains(item) {
                events.push(SpaceEvent {
                    event_type: "knowledge".to_string(),
                    actor: actor.map(str::to_string),
                    text: item.clone(),
                });
            }
        }
        events
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        state.beacon_launched
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        SpaceSummary {
            beacon_launched: state.beacon_launched,
            systems: derive_systems(state),
        }
    }
}

/// Formats one accepted game action from the observing participant's perspective.
fn event_text(action: &SpaceAction, is_actor: bool) -> String {
    let subject = if is_actor {
        "You".to_string()
    } else {
        format!("Player {}", action.player().unwrap_or("?"))
    };
    match action {
        SpaceAction::MoveStep { direction, .. } => format!("{subject} move {direction}."),
        SpaceAction::ToggleFuse { color, .. } => format!("{subject} toggle the {color} fuse."),
        SpaceAction::ToggleBreaker { breaker, .. } => {
            format!("{subject} toggle the {} breaker.", breaker.to_uppercase())
        }
        SpaceAction::SetValve { valve, open, .. } => format!(
            "{subject} {} valve {valve}.",
            if *open { "open" } else { "close" }
        ),
        SpaceAction::HoldOverride { held, .. } => format!(
            "{subject} {} the bypass.",
            if *held { "hold" } else { "release" }
        ),
        SpaceAction::ChargeBattery { .. } => format!("{subject} try the battery charger."),
        SpaceAction::MoveBattery { .. } => format!("{subject} move the battery sled."),
        SpaceAction::CycleRelay { .. } => format!("{subject} rotate the signal relay."),
        SpaceAction::LaunchBeacon { .. } => format!("{subject} launch the beacon."),
        SpaceAction::RunDiagnostic { .. } => format!("{subject} run diagnostics."),
        _ => format!("{subject} act."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms the dashboard cannot save game-owned keys that Space Game would ignore.
    #[test]
    fn game_configuration_rejects_unknown_options() {
        let adapter = SpaceGameAdapter::new();
        adapter.validate_config(&serde_json::json!({})).unwrap();
        let error = adapter
            .validate_config(&serde_json::json!({"difficulty": 2}))
            .unwrap_err();
        assert!(error.to_string().contains("no game-specific options"));
    }

    /// Confirms the dashboard receives every agent selector resolved by Space Game.
    #[test]
    fn compiled_agent_catalogue_matches_factory_selectors() {
        let factories = SpaceGameAdapter::new().agent_factories();
        assert_eq!(
            factories
                .iter()
                .map(|factory| factory.id.as_str())
                .collect::<Vec<_>>(),
            vec!["space_game.back_and_forth", "remote_grpc"]
        );
        let remote = factories
            .iter()
            .find(|factory| factory.id == "remote_grpc")
            .unwrap();
        assert!(remote
            .config_fields
            .iter()
            .any(|field| field.key == "endpoint" && field.required));
    }
}
