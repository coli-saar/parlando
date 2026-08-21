use super::*;

/// Participant-availability lifecycle for one experiment hosted by a game process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExperimentLifecycle {
    /// New participants and session entry are paused.
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
pub(super) async fn require_open_experiment<A: Game>(
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

#[cfg(test)]
mod tests {
    use super::ExperimentLifecycle;

    /// Verifies every lifecycle has one stable parse and serialization representation.
    #[test]
    fn lifecycle_wire_values_round_trip() {
        for (wire, lifecycle) in [
            ("inactive", ExperimentLifecycle::Inactive),
            ("testing", ExperimentLifecycle::Testing),
            ("active", ExperimentLifecycle::Active),
            ("completed", ExperimentLifecycle::Completed),
            ("archived", ExperimentLifecycle::Archived),
        ] {
            assert_eq!(ExperimentLifecycle::parse(wire).unwrap(), lifecycle);
            assert_eq!(lifecycle.as_str(), wire);
        }
        for invalid in ["", "ACTIVE", " active", "running", "deleted"] {
            assert!(ExperimentLifecycle::parse(invalid).is_err(), "{invalid:?}");
        }
    }

    /// Exhaustively verifies the deliberate lifecycle-transition graph.
    #[test]
    fn lifecycle_transition_table_is_exhaustive() {
        use ExperimentLifecycle::{Active, Archived, Completed, Inactive, Testing};

        let states = [Inactive, Testing, Active, Completed, Archived];
        let allowed = [
            (Inactive, Testing),
            (Inactive, Active),
            (Inactive, Archived),
            (Testing, Inactive),
            (Testing, Active),
            (Active, Inactive),
            (Active, Completed),
            (Completed, Archived),
            (Archived, Inactive),
        ];
        for current in states {
            for next in states {
                let expected = current == next || allowed.contains(&(current, next));
                assert_eq!(
                    current.can_transition_to(next),
                    expected,
                    "unexpected transition result for {} -> {}",
                    current.as_str(),
                    next.as_str()
                );
            }
        }
    }

    /// Confirms intake and immutable data-purpose classification agree with lifecycle policy.
    #[test]
    fn lifecycle_intake_and_data_purpose_are_consistent() {
        use ExperimentLifecycle::{Active, Archived, Completed, Inactive, Testing};

        assert!(!Inactive.allows_intake());
        assert!(Testing.allows_intake());
        assert!(Active.allows_intake());
        assert!(!Completed.allows_intake());
        assert!(!Archived.allows_intake());
        assert_eq!(Testing.data_purpose(), "testing");
        for lifecycle in [Inactive, Active, Completed, Archived] {
            assert_eq!(lifecycle.data_purpose(), "research");
        }
    }
}
