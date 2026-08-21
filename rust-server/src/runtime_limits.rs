//! Process-wide safety limits shared by the server and bundled operational tools.

use std::sync::OnceLock;

use serde::Deserialize;

/// All non-experiment safety ceilings embedded in a Parlando binary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLimits {
    /// Coarse protection for unauthenticated direct-participant creation.
    pub participant_creation: ParticipantCreationLimit,
}

/// Sliding-window ceiling for direct-participant creation attempts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ParticipantCreationLimit {
    /// Maximum accepted creation attempts inside one window.
    pub max_attempts: usize,
    /// Sliding-window duration in seconds.
    pub window_seconds: i64,
}

/// Returns the validated limits embedded from `config/runtime-limits.json` at build time.
///
/// The same immutable value is used by production request enforcement and tools such as
/// `runtime-stress`, preventing preflight calculations from drifting away from the server.
pub fn bundled_runtime_limits() -> &'static RuntimeLimits {
    static LIMITS: OnceLock<RuntimeLimits> = OnceLock::new();
    LIMITS.get_or_init(|| {
        let limits: RuntimeLimits =
            serde_json::from_str(include_str!("../config/runtime-limits.json"))
                .expect("bundled runtime limits must be valid JSON");
        assert!(
            limits.participant_creation.max_attempts > 0,
            "participant creation max_attempts must be positive"
        );
        assert!(
            limits.participant_creation.window_seconds > 0,
            "participant creation window_seconds must be positive"
        );
        limits
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ensures the shipped asset remains parseable and internally valid.
    #[test]
    fn bundled_limits_are_valid() {
        let limits = bundled_runtime_limits();
        assert_eq!(limits.participant_creation.max_attempts, 300);
        assert_eq!(limits.participant_creation.window_seconds, 60);
    }
}
