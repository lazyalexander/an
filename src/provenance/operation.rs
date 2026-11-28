use anyhow::{ensure, Result};
use serde::Deserialize;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Operation {
    Inbox,
    SubmitReview,
    ReviewInbox,
    NotifyCorrection,
    Corrections,
    Notifications,
    TakeNotification,
    AckNotification,
    Release,
    RequestReview,
    ReadReview,
    Sources,
    Impact,
    HandoffBlock,
    Grant,
    Revoke,
    Freeze,
    ReadEvidence,
    ReopenEvidence,
    AdmitMaterial,
    History,
    Decide,
    Commit,
}

impl Operation {
    pub(super) fn parse(name: &str) -> Result<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(name))
            .map_err(|_| anyhow::anyhow!("Unknown provenance operation"))
    }

    pub(super) fn allow_mcp(self, delegated: bool) -> Result<()> {
        use Operation::*;
        let allowed = match self {
            HandoffBlock | NotifyCorrection | Grant | Revoke | AdmitMaterial | Freeze
            | ReopenEvidence => false,
            Commit | Decide => delegated,
            Inbox | ReviewInbox | SubmitReview | Corrections | Notifications | TakeNotification
            | AckNotification | Release | RequestReview | ReadReview | Sources | Impact
            | ReadEvidence | History => true,
        };
        ensure!(allowed, "Controller CLI required");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Operation;

    #[test]
    fn existing_mcp_channels_keep_their_operation_sets() {
        for (names, ordinary, scoped) in [
            ("handoff-block notify-correction grant revoke admit-material freeze reopen-evidence", false, false),
            ("commit decide", false, true),
            ("inbox review-inbox submit-review corrections notifications take-notification ack-notification release request-review read-review sources impact read-evidence history", true, true),
        ] {
            for name in names.split_whitespace() {
                let operation = Operation::parse(name).unwrap();
                assert_eq!(operation.allow_mcp(false).is_ok(), ordinary, "{name}");
                assert_eq!(operation.allow_mcp(true).is_ok(), scoped, "{name}");
            }
        }
        assert!(Operation::parse("missing").is_err());
    }
}
