use anyhow::Result;
use parlando_server::{GameAdapter, PlayerRole};

use super::state_engine::{
    apply_action, available_actions, derive_systems, initial_state, validate_action,
    FilteredKnowledge, SpaceAction, SpaceEvent, SpaceGameState, SpaceObservation, SpaceSummary,
};

#[derive(Clone, Debug, Default)]
pub struct SpaceGameAdapter;

impl SpaceGameAdapter {
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
