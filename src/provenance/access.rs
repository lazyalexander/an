//! Capability lookup is repeated in the operation transaction, including on replay.
use super::domain::{ArtifactId, BlockRef, VersionId};
use crate::{
    permission,
    store::{find, get, hash, put, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "right", rename_all = "snake_case", deny_unknown_fields)]
pub enum Right {
    Read {
        artifact: ArtifactId,
        version: VersionId,
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

impl Actor {
    pub fn authenticate(db: &Connection, task: &str) -> Result<Self> {
        if scoped() {
            let token = std::env::var("AN_PROVENANCE_TOKEN").context("Invalid delegation")?;
            let digest = hash(token.as_bytes());
            let grant = find(db, "provenance_key", &digest)?.context("Inactive delegation")?;
            let grant_id = grant.as_str().context("Invalid delegation")?;
            let record = get(db, "provenance_grant", grant_id)?;
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
            return Ok(Self {
                id: format!("agent:{grant_id}"),
                task: task.into(),
                grant: Some(grant_id.into()),
                rights: serde_json::from_value(record["rights"].clone())?,
            });
        }
        permission::controller(db, task)?;
        Ok(Self {
            id: format!("controller:{task}"),
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
            Right::Register { artifact }
            | Right::Revise { artifact }
            | Right::Decide { artifact, .. } => self.owns(db, artifact),
        }
    }

    pub fn require(&self, db: &Connection, right: Right) -> Result<()> {
        ensure!(self.allows(db, &right)?, "Material outside authority");
        Ok(())
    }
}

pub fn grant(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let target = args["target"].as_str().context("Expected target task")?;
    let workspace = get(db, "task", &actor.task)?["workspace"].clone();
    ensure!(
        actor.task == target
            || (workspace.is_string() && get(db, "task", target)?["workspace"] == workspace),
        "Target outside workspace"
    );
    let epoch = get(db, "turn", target)?;
    ensure!(epoch.is_string(), "Target turn inactive");
    let rights: Vec<Right> = serde_json::from_value(args["rights"].clone())?;
    ensure!(!rights.is_empty(), "Empty grant");
    for right in &rights {
        actor.require(db, right.clone())?;
        if let Right::Read { artifact, version } | Right::Decide { artifact, version } = right {
            ensure!(
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pv_version WHERE artifact=? AND version=?)",
                    rusqlite::params![artifact.0, version.0],
                    |r| r.get::<_, bool>(0)
                )?,
                "Unknown version"
            );
        }
    }
    let evidence: Vec<_> = rights
        .iter()
        .filter_map(|right| match right {
            Right::Evidence { snapshot } => Some(snapshot),
            _ => None,
        })
        .collect();
    if !evidence.is_empty() {
        if let Some(p) = permission::gate(
            db,
            &actor.task,
            "log",
            json!(["delegate", target, evidence]),
            json!([epoch]),
            args["request_id"].as_str(),
            None,
        )? {
            return Ok(p);
        }
    }
    let id = uid();
    let secret = uid();
    let record = json!({
        "id":id, "issuer":actor.id, "task":actor.task, "target":target, "epoch":epoch,
        "target_scope":permission::delegation_scope(db,target)?,
        "source_scope":permission::delegation_scope(db,&actor.task)?,
        "rights":rights, "active":true
    });
    put(db, "provenance_grant", &id, &record)?;
    put(db, "provenance_key", &hash(secret.as_bytes()), &json!(id))?;
    Ok(json!({"grant":id,"token":secret}))
}

pub fn revoke(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let id = args["grant"].as_str().context("Expected grant")?;
    let mut record = get(db, "provenance_grant", id)?;
    ensure!(record["issuer"] == actor.id, "Outside authority");
    record["active"] = json!(false);
    put(db, "provenance_grant", id, &record)?;
    Ok(json!({"grant":id,"active":false}))
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
