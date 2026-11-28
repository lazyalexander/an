//! Explicit links retain each decision and its original notification history.
use crate::provenance::{
    access::{Actor, Right},
    domain::{ArtifactId, Decision, VersionId},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(in crate::provenance) fn link(
    db: &Connection,
    actor: &Actor,
    decision: &Decision,
    previous: &str,
    replacement: &str,
) -> Result<()> {
    let prior: Option<u32> = db
        .query_row(
            "SELECT version FROM pv_choice WHERE receipt=? AND artifact=? AND version<?
         AND json_extract(body,'$.task')=? AND json_extract(body,'$.accepted.cursor')<=?",
            params![
                previous,
                decision.artifact.0,
                decision.version.0,
                actor.task,
                decision.seen_cursor
            ],
            |r| r.get(0),
        )
        .optional()?;
    let version =
        prior.context("Expected earlier decision of this artifact at the observed cursor")?;
    actor.require(
        db,
        Right::Decide {
            artifact: decision.artifact.clone(),
            version: VersionId(version),
        },
    )?;
    db.execute(
        "INSERT INTO pv_correction(previous,replacement) VALUES(?,?)",
        params![previous, replacement],
    )
    .context("Decision already corrected")?;
    Ok(())
}

pub(in crate::provenance) fn inspect(
    db: &Connection,
    actor: &Actor,
    args: &Value,
) -> Result<Value> {
    actor.controller()?;
    let receipt = args["decision"]
        .as_str()
        .context("Expected decision receipt")?;
    let mut stmt = db.prepare(
        "SELECT c.previous,c.replacement FROM pv_correction c
         JOIN pv_choice p ON p.receipt=c.previous
         WHERE json_extract(p.body,'$.task')=? AND (c.previous=?2 OR c.replacement=?2)
         ORDER BY p.version",
    )?;
    let mut links = stmt.query(params![actor.task, receipt])?;
    let mut results = vec![];
    let mut recipients = db.prepare(include_str!("correction_recipients.sql"))?;
    while let Some(row) = links.next()? {
        let previous: String = row.get(0)?;
        let replacement: String = row.get(1)?;
        let statuses = recipients
            .query_map(params![previous, replacement], |r| {
                Ok(json!({
                    "recipient":r.get::<_,String>(0)?,
                    "original":r.get::<_,String>(1)?,
                    "correction":r.get::<_,String>(2)?
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        results.push(json!({"previous":previous,"replacement":replacement,"recipients":statuses}));
    }
    Ok(json!({"corrections":results}))
}

/// The immutable correction may precede the grant for receiving its new version.
pub(in crate::provenance) fn notify(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let decision = args["decision"]
        .as_str()
        .context("Expected correction receipt")?;
    let to = args["recipient"]
        .as_str()
        .context("Expected recipient grant")?;
    let (artifact, version): (String,u32) = db.query_row(
        "SELECT p.artifact,p.version FROM pv_choice p JOIN pv_correction c ON c.replacement=p.receipt
         WHERE p.receipt=? AND json_extract(p.body,'$.task')=?
         AND NOT EXISTS(SELECT 1 FROM pv_correction next WHERE next.previous=p.receipt)",
        params![decision,actor.task],|r| Ok((r.get(0)?,r.get(1)?))
    ).optional()?.context("Expected current correction in this task")?;
    let prior: Option<(String, String)> = db
        .query_row(
            "SELECT id,recipient FROM pv_notice WHERE decision=?",
            [decision],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, recipient)) = prior {
        ensure!(
            recipient == to,
            "Correction already addressed to another recipient"
        );
        return Ok(json!({"id":id,"recipient":to}));
    }
    super::notification::enqueue(
        db,
        actor,
        &ArtifactId(artifact),
        VersionId(version),
        to,
        decision,
    )
}
