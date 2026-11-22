//! Runs Counterweight Hotel with two seats using the same policy backend.

#[path = "../../../../support/headless.rs"]
mod headless;

/// Launches the production experiment runner with the hotel porter.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    headless::run(
        "counterweight-hotel",
        "Counterweight Hotel",
        env!("CARGO_PKG_VERSION"),
        parlando_counterweight_hotel::PorterFactory::for_game,
    )
    .await
}
