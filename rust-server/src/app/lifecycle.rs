use super::*;

/// Intake lifecycle for the single experiment owned by this server process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExperimentLifecycle {
    /// New participants and room entry are paused.
    Inactive,
    /// New participants may enter the experiment.
    Active,
}

impl ExperimentLifecycle {
    /// Parses the complete lifecycle vocabulary accepted by the administrator API.
    pub(super) fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "inactive" => Ok(Self::Inactive),
            "active" => Ok(Self::Active),
            _ => Err(AppError::bad_request(
                "Experiment status must be inactive or active.",
            )),
        }
    }

    /// Returns the stable storage and protocol representation.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Active => "active",
        }
    }
}

/// Rejects new experiment intake while preserving existing room connections.
pub(super) async fn require_active_experiment<A: GameAdapter>(
    state: &AppState<A>,
) -> Result<tokio::sync::RwLockReadGuard<'_, ExperimentLifecycle>, AppError> {
    let lifecycle = state.experiment_lifecycle.read().await;
    if *lifecycle == ExperimentLifecycle::Active {
        Ok(lifecycle)
    } else {
        Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Experiment intake is inactive",
        ))
    }
}
