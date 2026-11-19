//! Waiting, activity, and farewell deadlines owned by a live session.

use super::SessionLifecycle;
use chrono::{DateTime, Utc};

/// The session exposes shared references to the timestamps used by projections and expiry.
pub(in crate::app) struct LiveSessionTiming {
    /// Fixed beginning of the unmatched waiting phase.
    pub waiting_started_at: String,
    /// Fixed server-owned deadline for finding a partner.
    pub waiting_deadline_at: String,
    /// Fixed absolute infrastructure lifetime deadline.
    pub lifetime_deadline_at: String,
    /// Last accepted message or action; heartbeats never advance this value.
    pub last_meaningful_activity_at: Option<String>,
    /// Fixed idle deadline derived from the last meaningful activity.
    pub idle_deadline_at: Option<String>,
    /// Fixed end of the optional post-completion human voice relay period.
    pub voice_ends_at: Option<String>,
    pub updated_at: String,
}

impl LiveSessionTiming {
    /// Initializes fixed lifetime and waiting deadlines from the durable session row.
    pub(super) fn waiting(
        started: String,
        waiting_deadline: String,
        lifetime_deadline: String,
    ) -> Self {
        Self {
            updated_at: started.clone(),
            waiting_started_at: started,
            waiting_deadline_at: waiting_deadline,
            lifetime_deadline_at: lifetime_deadline,
            last_meaningful_activity_at: None,
            idle_deadline_at: None,
            voice_ends_at: None,
        }
    }

    /// Selects the authoritative expiry cause, including simultaneous deadlines.
    pub(in crate::app) fn due_deadline<'a>(
        &self,
        lifecycle: &SessionLifecycle,
        mut reconnect_deadlines: impl Iterator<Item = &'a str>,
        now: DateTime<Utc>,
    ) -> Option<&'static str> {
        let elapsed =
            |stamp: &str| DateTime::parse_from_rfc3339(stamp).is_ok_and(|deadline| deadline <= now);
        if lifecycle.is_ended() {
            None
        } else if elapsed(&self.lifetime_deadline_at) {
            Some("maximum_lifetime")
        } else if lifecycle == &SessionLifecycle::Forming && elapsed(&self.waiting_deadline_at) {
            Some("waiting_timeout")
        } else if reconnect_deadlines.any(elapsed) {
            Some("reconnect_timeout")
        } else if lifecycle == &SessionLifecycle::Running
            && self.idle_deadline_at.as_deref().is_some_and(elapsed)
        {
            Some("idle_timeout")
        } else {
            None
        }
    }

    /// Advances both representations of meaningful activity and the associated idle deadline.
    pub(super) fn record_activity(&mut self, at: DateTime<Utc>, idle: DateTime<Utc>) {
        let timestamp = at.to_rfc3339();
        self.updated_at = timestamp.clone();
        self.last_meaningful_activity_at = Some(timestamp);
        self.idle_deadline_at = Some(idle.to_rfc3339());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renewing idle time leaves the original waiting and lifetime limits intact.
    #[test]
    fn activity_only_renews_idle_deadline() {
        let start = DateTime::from_timestamp(1_000, 0).unwrap();
        let waiting = start + chrono::Duration::minutes(10);
        let lifetime = start + chrono::Duration::hours(2);
        let mut clock = LiveSessionTiming::waiting(
            start.to_rfc3339(),
            waiting.to_rfc3339(),
            lifetime.to_rfc3339(),
        );
        assert!(clock.idle_deadline_at.is_none());
        let activity = start + chrono::Duration::minutes(1);
        let idle = activity + chrono::Duration::minutes(3);
        clock.record_activity(activity, idle);
        assert_eq!(clock.updated_at, activity.to_rfc3339());
        assert_eq!(
            clock.last_meaningful_activity_at,
            Some(activity.to_rfc3339())
        );
        assert_eq!(clock.idle_deadline_at, Some(idle.to_rfc3339()));
        assert_eq!(clock.waiting_deadline_at, waiting.to_rfc3339());
        assert_eq!(clock.lifetime_deadline_at, lifetime.to_rfc3339());
    }
    /// At exact deadlines, lifetime outranks waiting and reconnect outranks idle.
    #[test]
    fn coincident_deadlines_use_stable_precedence() {
        let start = DateTime::from_timestamp(1_000, 0).unwrap();
        let cutoff = start + chrono::Duration::seconds(10);
        let clock = LiveSessionTiming::waiting(
            start.to_rfc3339(),
            cutoff.to_rfc3339(),
            cutoff.to_rfc3339(),
        );
        let forming = SessionLifecycle::Forming;
        assert_eq!(
            clock.due_deadline(
                &forming,
                std::iter::empty(),
                cutoff - chrono::Duration::nanoseconds(1)
            ),
            None
        );
        assert_eq!(
            clock.due_deadline(&forming, std::iter::empty(), cutoff),
            Some("maximum_lifetime")
        );

        let mut clock = LiveSessionTiming::waiting(
            start.to_rfc3339(),
            cutoff.to_rfc3339(),
            (cutoff + chrono::Duration::hours(1)).to_rfc3339(),
        );
        clock.record_activity(start, cutoff);
        let disconnected_until = cutoff.to_rfc3339();
        let disconnected = std::iter::once(disconnected_until.as_str());
        assert_eq!(
            clock.due_deadline(&forming, disconnected.clone(), cutoff),
            Some("waiting_timeout")
        );
        assert_eq!(
            clock.due_deadline(&SessionLifecycle::Running, disconnected, cutoff),
            Some("reconnect_timeout")
        );
        assert_eq!(
            clock.due_deadline(&SessionLifecycle::Running, std::iter::empty(), cutoff),
            Some("idle_timeout")
        );
    }
}
