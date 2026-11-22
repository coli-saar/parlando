//! Runs The Last Screening with two seats using the same policy backend.

#[path = "../../../../support/headless.rs"]
mod headless;

/// Launches the production experiment runner with the screening partner.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    headless::run(
        "last-screening",
        "The Last Screening",
        env!("CARGO_PKG_VERSION"),
        parlando_last_screening::ScreeningPartnerFactory::for_game,
    )
    .await
}
