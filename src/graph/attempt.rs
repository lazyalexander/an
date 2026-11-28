//! Scheduling and retry policy for one graph node attempt.
use crate::store::uid;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AttemptStatus {
    Queued,
    PermissionRequired,
    Running,
    Waiting,
    Saving,
    CancelPending,
    CleanupPending,
    Done,
    Failed,
    Cancelled,
    Superseded,
    Unknown,
    AuthorizationExpired,
    #[serde(other)]
    Unrecognized,
}

pub(super) fn queued(number: u64) -> Value {
    json!({"id":uid(), "attempt":number, "status":AttemptStatus::Queued})
}

pub(super) fn next(activity: &Value) -> Result<Value> {
    let number = activity["attempt"]
        .as_u64()
        .and_then(|n| n.checked_add(1))
        .context("Invalid graph attempt number")?;
    Ok(queued(number))
}

impl AttemptStatus {
    pub(super) fn of(activity: &Value) -> Self {
        Self::deserialize(&activity["status"]).unwrap_or(Self::Unrecognized)
    }

    pub(super) fn can_advance(self, paused: bool) -> bool {
        match self {
            Self::Queued | Self::PermissionRequired => !paused,
            Self::Running | Self::Unknown => true,
            _ => false,
        }
    }

    pub(super) fn can_retry(self, model: bool) -> bool {
        matches!(self, Self::Failed | Self::Cancelled) || (model && self == Self::Unknown)
    }

    pub(super) fn can_supersede(self) -> bool {
        matches!(self, Self::Waiting | Self::Queued)
    }

    pub(super) fn can_stop(self) -> bool {
        matches!(
            self,
            Self::Queued
                | Self::PermissionRequired
                | Self::Running
                | Self::Waiting
                | Self::Saving
                | Self::CancelPending
                | Self::CleanupPending
                | Self::Unknown
        )
    }

    pub(super) fn allows_task_close(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Superseded | Self::Cancelled
        )
    }

    pub(super) fn needs_driver_cleanup(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::AuthorizationExpired
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_and_pending_attempts_have_distinct_recovery_policies() {
        for status in [
            AttemptStatus::CancelPending,
            AttemptStatus::CleanupPending,
            AttemptStatus::of(&json!({"status":"future_status"})),
        ] {
            assert!(!status.can_retry(true));
            assert!(!status.can_advance(false));
            assert!(!status.allows_task_close());
        }
        assert!(AttemptStatus::Unknown.can_retry(true));
        assert!(!AttemptStatus::Unknown.can_retry(false));
        assert!(AttemptStatus::Running.can_advance(true));
        assert!(!AttemptStatus::Queued.can_advance(true));
    }
}
