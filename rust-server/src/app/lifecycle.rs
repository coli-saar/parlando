use super::*;

/// Participant-availability lifecycle for one experiment hosted by a game process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExperimentLifecycle {
    /// New participants and room entry are paused.
    Inactive,
    /// Participant intake is open, but every new session is test data.
    Testing,
    /// New participants may enter the experiment.
    Active,
    /// Research intake has ended successfully and its results remain exportable.
    Completed,
    /// The experiment is soft-deleted from ordinary use and export.
    Archived,
}

impl ExperimentLifecycle {
    /// Parses the complete lifecycle vocabulary accepted by the administrator API.
    pub(super) fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "inactive" => Ok(Self::Inactive),
            "testing" => Ok(Self::Testing),
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "archived" => Ok(Self::Archived),
            _ => Err(AppError::bad_request(
                "Experiment status must be inactive, testing, active, completed, or archived.",
            )),
        }
    }

    /// Returns the stable storage and protocol representation.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Testing => "testing",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Archived => "archived",
        }
    }

    /// Reports whether participants may create or enter sessions in this lifecycle.
    pub(super) fn allows_intake(self) -> bool {
        matches!(self, Self::Testing | Self::Active)
    }

    /// Returns the immutable data-use purpose assigned to intake created now.
    pub(super) fn data_purpose(self) -> &'static str {
        if self == Self::Testing {
            "testing"
        } else {
            "research"
        }
    }

    /// Validates deliberate administrator transitions and rejects destructive shortcuts.
    pub(super) fn can_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (
                    Self::Inactive,
                    Self::Testing | Self::Active | Self::Archived
                ) | (Self::Testing, Self::Inactive | Self::Active)
                    | (Self::Active, Self::Inactive | Self::Completed)
                    | (Self::Completed, Self::Archived)
                    | (Self::Archived, Self::Inactive)
            )
    }
}

/// Rejects new intake unless the experiment is collecting test or research sessions.
pub(super) async fn require_open_experiment<A: GameAdapter>(
    state: &AppState<A>,
) -> Result<tokio::sync::RwLockReadGuard<'_, ExperimentLifecycle>, AppError> {
    let lifecycle = state.experiment_lifecycle.read().await;
    if lifecycle.allows_intake() {
        Ok(lifecycle)
    } else {
        Err(AppError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Experiment intake is closed",
        ))
    }
}
