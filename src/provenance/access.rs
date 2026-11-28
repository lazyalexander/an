//! Capability lookup is repeated in the operation transaction, including on replay.
mod grant;
use super::domain::{ArtifactId, BlockRef, VersionId};
use crate::{
    permission,
    store::{find, get, hash},
};
use anyhow::{ensure, Context, Result};
pub(super) use grant::{grant, revoke};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "right", rename_all = "snake_case", deny_unknown_fields)]
pub enum Right {
    Read {
        artifact: ArtifactId,
        version: VersionId,
    },
    Release {
        node: String,
        attempt: String,
    },
    ReceiveNotice {
        artifact: ArtifactId,
        version: VersionId,
    },
    Notify {
        artifact: ArtifactId,
        version: VersionId,
        recipient: String,
    },
    ReadInbox,
    ReviewInbox,
    SubmitReview {
        source: BlockRef,
    },
    Register {
        artifact: ArtifactId,
    },
    Revise {
        artifact: ArtifactId,
    },
    Decide {
        artifact: ArtifactId,
        version: VersionId,
    },
    RejectWithReport {
        artifact: ArtifactId,
        version: VersionId,
        report: BlockRef,
    },
    Evidence {
        snapshot: String,
    },
}

pub struct Actor {
    pub id: String,
    pub task: String,
    pub grant: Option<String>,
    rights: Vec<Right>,
}

pub fn scoped() -> bool {
    std::env::var_os("AN_PROVENANCE_TOKEN").is_some()
}

pub(super) fn require_active_grant(db: &Connection, task: &str, record: &Value) -> Result<()> {
    ensure!(
        record["active"] == true
            && record["target"] == task
            && record["epoch"].is_string()
            && permission::delegation_scope(db, task)? == record["target_scope"]
            && permission::delegation_scope(
                db,
                record["task"].as_str().context("Invalid issuer")?
            )? == record["source_scope"],
        "Inactive delegation"
    );
    Ok(())
}

impl Actor {
    pub fn authenticate(db: &Connection, task: &str) -> Result<Self> {
        if scoped() {
            let token = std::env::var("AN_PROVENANCE_TOKEN").context("Invalid delegation")?;
            let digest = hash(token.as_bytes());
            let grant = find(db, "provenance_key", &digest)?.context("Inactive delegation")?;
            let grant_id = grant.as_str().context("Invalid delegation")?;
            let record = get(db, "provenance_grant", grant_id)?;
            require_active_grant(db, task, &record)?;
            return Ok(Self {
                id: format!("agent:{grant_id}"),
                task: task.into(),
                grant: Some(grant_id.into()),
                rights: serde_json::from_value(record["rights"].clone())?,
            });
        }
        permission::controller(db, task)?;
        Ok(Self {
            id: permission::controller_identity(db, task)?,
            task: task.into(),
            grant: None,
            rights: vec![],
        })
    }

    pub fn may_observe_position(&self) -> bool {
        self.grant.is_none()
            || self
                .rights
                .iter()
                .any(|right| matches!(right, Right::Decide { .. } | Right::RejectWithReport { .. }))
    }

    pub fn controller(&self) -> Result<()> {
        ensure!(self.grant.is_none(), "Controller required");
        Ok(())
    }

    fn owns(&self, db: &Connection, artifact: &ArtifactId) -> Result<bool> {
        let owner: Option<String> = db
            .query_row(
                "SELECT owner FROM pv_artifact WHERE id=?",
                [&artifact.0],
                |r| r.get(0),
            )
            .optional()?;
        Ok(owner.as_deref() == Some(&self.task) && self.grant.is_none())
    }

    pub fn allows(&self, db: &Connection, right: &Right) -> Result<bool> {
        if self.grant.is_some() {
            return Ok(self.rights.contains(right));
        }
        match right {
            Right::Release { node, attempt } => {
                let graph = get(db, "graph", &self.task)?;
                Ok(graph["nodes"][node].get("publication_review").is_some()
                    && graph["activity"][node]["id"] == attempt.as_str())
            }
            Right::ReadInbox | Right::ReviewInbox => super::inbox::is_primary(db, &self.task),
            Right::SubmitReview { source } => self.owns(db, &source.artifact),
            Right::Evidence { snapshot } => Ok(find(db, "frozen_evidence", snapshot)?
                .is_some_and(|s| s["task"] == self.task)
                || super::evidence::reopened(db, &self.task, snapshot)?),
            Right::Read { artifact, version } => {
                if self.owns(db, artifact)? {
                    return Ok(true);
                }
                Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM pv_published WHERE artifact=? AND version=? AND workspace=(
                    SELECT json_extract(body,'$.workspace') FROM objects WHERE kind='task' AND id=?))",rusqlite::params![artifact.0,version.0,self.task],|r|r.get(0))?)
            }
            Right::RejectWithReport {
                artifact,
                version,
                report,
            } => {
                if !self.owns(db, artifact)?
                    || !self.allows(
                        db,
                        &Right::Read {
                            artifact: report.artifact.clone(),
                            version: report.version,
                        },
                    )?
                {
                    return Ok(false);
                }
                Ok(db.query_row(
                    include_str!("verified_decision.sql"),
                    rusqlite::params![
                        report.artifact.0,
                        report.version.0,
                        report.block,
                        artifact.0,
                        version.0
                    ],
                    |row| row.get(0),
                )?)
            }
            Right::ReceiveNotice { artifact, .. }
            | Right::Notify { artifact, .. }
            | Right::Register { artifact }
            | Right::Revise { artifact }
            | Right::Decide { artifact, .. } => self.owns(db, artifact),
        }
    }

    pub fn require(&self, db: &Connection, right: Right) -> Result<()> {
        ensure!(self.allows(db, &right)?, "Material outside authority");
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn test_controller(task: &str) -> Actor {
    Actor {
        id: format!("controller:{task}"),
        task: task.into(),
        grant: None,
        rights: vec![],
    }
}
