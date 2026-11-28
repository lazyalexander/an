//! Map the requested lifecycle and Apple Container's observation to a registry status.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstanceIntent {
    Load,
    Stop,
    Unload,
}

impl InstanceIntent {
    pub(crate) fn recorded(instance: &Value) -> Option<Self> {
        Self::deserialize(instance.get("intent")?).ok()
    }

    pub(crate) fn is_shutdown(self) -> bool {
        matches!(self, Self::Stop | Self::Unload)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GuestState {
    Absent,
    Running,
    Stopped,
    Created,
    Exited,
    Unknown,
    IdentityMismatch,
    #[serde(untagged)]
    Other(String),
}

impl GuestState {
    pub(super) fn reported(state: Option<&str>) -> Self {
        match state {
            Some("running") => Self::Running,
            Some("stopped") => Self::Stopped,
            Some("created") => Self::Created,
            Some("exited") => Self::Exited,
            // Absence and ownership can only be established from the container list.
            None | Some("unknown" | "absent" | "identity_mismatch") => Self::Unknown,
            Some(other) => Self::Other(other.to_owned()),
        }
    }

    pub(super) fn can_delete(&self) -> bool {
        matches!(self, Self::Stopped | Self::Created | Self::Exited)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstanceStatus {
    Running,
    Stopped,
    Unloaded,
    CleanupPending,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Termination {
    Confirmed,
    Unconfirmed,
}

pub(crate) struct Settlement {
    pub guest: GuestState,
    pub status: InstanceStatus,
    pub termination: Termination,
}

impl Settlement {
    pub(super) fn from_observation(intent: Option<InstanceIntent>, guest: GuestState) -> Self {
        use GuestState::{Absent, Created, Exited, Running, Stopped};
        use InstanceIntent::{Stop, Unload};
        use Termination::{Confirmed, Unconfirmed};
        let (status, termination) = match (intent, &guest) {
            (Some(Stop | Unload), Absent) => (InstanceStatus::Unloaded, Confirmed),
            (Some(Stop), Stopped | Created | Exited) => (InstanceStatus::Stopped, Confirmed),
            (Some(Stop | Unload), _) => (InstanceStatus::CleanupPending, Unconfirmed),
            (_, Running) => (InstanceStatus::Running, Unconfirmed),
            (_, Absent | Stopped | Created | Exited) => (InstanceStatus::Stopped, Unconfirmed),
            _ => (InstanceStatus::Unknown, Unconfirmed),
        };
        Self {
            guest,
            status,
            termination,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termination_requires_a_matching_intent_and_confirmed_observation() {
        for guest in [
            GuestState::Unknown,
            GuestState::IdentityMismatch,
            GuestState::reported(Some("absent")),
        ] {
            let settled = Settlement::from_observation(Some(InstanceIntent::Unload), guest);
            assert_eq!(settled.status, InstanceStatus::CleanupPending);
            assert_eq!(settled.termination, Termination::Unconfirmed);
        }
        let idle = Settlement::from_observation(Some(InstanceIntent::Load), GuestState::Absent);
        assert_eq!(idle.status, InstanceStatus::Stopped);
        assert_eq!(idle.termination, Termination::Unconfirmed);
        let stopped = Settlement::from_observation(Some(InstanceIntent::Stop), GuestState::Exited);
        assert_eq!(stopped.termination, Termination::Confirmed);
        let gone = Settlement::from_observation(Some(InstanceIntent::Unload), GuestState::Absent);
        assert_eq!(gone.status, InstanceStatus::Unloaded);
        assert_eq!(gone.termination, Termination::Confirmed);
    }
}
