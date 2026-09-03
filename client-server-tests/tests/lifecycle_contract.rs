use anyhow::Result;
use parlando::test_support::ServeOptions;
use parlando_client_server_contract_tests::{
    contract_config, run_node_driver, spawn_inactive_server, spawn_server,
};
use serde_json::json;

/// Runs one production JavaScript participant scenario against an active live server.
async fn run_active_scenario(
    config: parlando::test_support::ExperimentConfig,
    scenario: &str,
) -> Result<()> {
    let server = spawn_server(config, ServeOptions::default()).await?;
    let result = run_node_driver(
        &server,
        "lifecycle-driver.mjs",
        json!({"scenario": scenario}),
    )
    .await?;
    assert_eq!(result, json!({"scenario": scenario, "status": "passed"}));
    Ok(())
}

/// Confirms the JavaScript welcome client and admission call agree when intake is closed.
#[tokio::test]
async fn inactive_welcome_refuses_participant_registration() -> Result<()> {
    let server = spawn_inactive_server(contract_config(), ServeOptions::default()).await?;
    let result = run_node_driver(
        &server,
        "lifecycle-driver.mjs",
        json!({"scenario": "inactive-welcome"}),
    )
    .await?;
    assert_eq!(
        result,
        json!({"scenario": "inactive-welcome", "status": "passed"})
    );
    Ok(())
}

/// Exercises configuration discovery, consent admission, waiting, and idempotent explicit leave.
#[tokio::test]
async fn welcome_waiting_and_waiting_room_leave_follow_one_authoritative_lifecycle() -> Result<()> {
    run_active_scenario(contract_config(), "welcome-waiting-leave").await
}

/// Exercises the complete two-client path, including rejection, chat, transitions, and completion.
#[tokio::test]
async fn paired_clients_share_rejections_messages_transitions_and_completion() -> Result<()> {
    run_active_scenario(contract_config(), "paired-game").await
}

/// Confirms an explicit JavaScript-client departure yields asymmetric participant outcomes.
#[tokio::test]
async fn active_leave_ends_the_caller_and_partner_with_distinct_outcomes() -> Result<()> {
    run_active_scenario(contract_config(), "active-leave").await
}

/// Verifies a transient disconnect can obtain a new ticket and resume the same room.
#[tokio::test]
async fn reconnect_before_grace_preserves_session_then_expiry_terminates_both_roles() -> Result<()>
{
    let mut config = contract_config();
    config.session.reconnect_grace_seconds = 1;
    run_active_scenario(config, "reconnect").await
}

/// Verifies the server, rather than transport heartbeats, owns waiting and idle deadlines.
#[tokio::test]
async fn waiting_and_idle_deadlines_reconcile_to_distinct_terminal_outcomes() -> Result<()> {
    let mut waiting_config = contract_config();
    waiting_config.session.waiting_session_timeout_seconds = 1;
    run_active_scenario(waiting_config, "waiting-timeout").await?;

    let mut idle_config = contract_config();
    idle_config.session.session_idle_timeout_seconds = 1;
    idle_config.session.session_max_lifetime_seconds = 10;
    run_active_scenario(idle_config, "idle-timeout").await
}

/// Confirms JavaScript-client activity resets idle time but cannot extend absolute lifetime.
#[tokio::test]
async fn meaningful_activity_cannot_extend_the_absolute_lifetime() -> Result<()> {
    let mut config = contract_config();
    config.session.session_idle_timeout_seconds = 1;
    config.session.session_max_lifetime_seconds = 1;
    run_active_scenario(config, "lifetime-timeout").await
}
