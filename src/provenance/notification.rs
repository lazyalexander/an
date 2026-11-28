//! A task-local recipient polls for work. Dispatch is durable before payload return.
use super::{
    access::{require_active_grant, Actor, Right},
    domain::{ArtifactId, Decision, VersionId},
};
use crate::store::{get, uid};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(super) fn recipient(
    db: &Connection,
    task: &str,
    id: &str,
    artifact: &ArtifactId,
    version: VersionId,
) -> Result<()> {
    let grant = get(db, "provenance_grant", id)?;
    require_active_grant(db, task, &grant)?;
    let rights: Vec<Right> = serde_json::from_value(grant["rights"].clone())?;
    ensure!(
        rights.contains(&Right::ReceiveNotice {
            artifact: artifact.clone(),
            version
        }),
        "Recipient outside notification scope"
    );
    Ok(())
}

pub fn enqueue(
    db: &Connection,
    actor: &Actor,
    decision: &Decision,
    to: &str,
    receipt: &str,
) -> Result<Value> {
    actor.require(
        db,
        Right::Notify {
            artifact: decision.artifact.clone(),
            version: decision.version,
            recipient: to.into(),
        },
    )?;
    recipient(db, &actor.task, to, &decision.artifact, decision.version)?;
    let id = uid();
    db.execute(
        "INSERT INTO pv_notice(id,task,decision,sender_grant,recipient) VALUES(?,?,?,?,?)",
        params![id, actor.task, receipt, actor.grant, to],
    )?;
    Ok(json!({"id":id,"recipient":to}))
}

pub fn list(db: &Connection, actor: &Actor) -> Result<Value> {
    let mut notices = db.prepare(
        "SELECT id,decision,recipient FROM pv_notice
         WHERE task=? AND (?2 IS NULL OR recipient=?2 OR sender_grant=?2) ORDER BY rowid",
    )?;
    let mut rows = notices.query(params![actor.task, actor.grant])?;
    let mut items = vec![];
    let mut attempts = db.prepare(
        "SELECT id,status,started,settled FROM pv_notice_attempt WHERE notice=? ORDER BY seq",
    )?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let history = attempts
            .query_map([&id], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "status": r.get::<_, String>(1)?,
                    "started": r.get::<_, i64>(2)?,
                    "settled": r.get::<_, Option<i64>>(3)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        items.push(json!({
            "id": id,
            "decision": row.get::<_, String>(1)?,
            "recipient": row.get::<_, String>(2)?,
            "attempts": history,
        }));
    }
    Ok(json!({"notifications":items}))
}

fn receiver(actor: &Actor) -> Result<&str> {
    actor
        .grant
        .as_deref()
        .context("Recipient delegation required")
}

pub fn take(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    let id = args["id"].as_str().context("Expected notification ID")?;
    let retry = args
        .get("retry_after")
        .map(|v| v.as_str().context("Expected prior attempt ID"))
        .transpose()?;
    let (decision, sender): (String, Option<String>) = db
        .query_row(
            "SELECT decision,sender_grant FROM pv_notice WHERE id=? AND task=? AND recipient=?",
            params![id, actor.task, receiver(actor)?],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .context("Notification unavailable")?;
    if let Some(sender) = sender {
        require_active_grant(db, &actor.task, &get(db, "provenance_grant", &sender)?)?;
    }
    let prior: Option<String> = db
        .query_row(
            "SELECT id FROM pv_notice_attempt WHERE notice=? ORDER BY seq DESC LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(
        prior.as_deref() == retry,
        "Explicit retry must name the latest attempt"
    );
    let received: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pv_notice_attempt WHERE notice=? AND status='received')",
        [id],
        |r| r.get(0),
    )?;
    ensure!(!received, "Notification already received");
    let attempt = uid();
    db.execute(
        "INSERT INTO pv_notice_attempt(notice,id,status,started) VALUES(?,?,'unknown',unixepoch())",
        params![id, attempt],
    )?;
    // Only the granted version and outcome cross into the recipient's view.
    let (artifact, version, choice): (String, u32, String) = db.query_row(
        "SELECT artifact,version,json_extract(body,'$.choice') FROM pv_choice WHERE receipt=?",
        [&decision],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(
        json!({"id":id,"attempt":attempt,"decision":decision,"artifact":artifact,"version":version,"choice":choice}),
    )
}

pub fn ack(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    let id = args["attempt"].as_str().context("Expected attempt ID")?;
    let status = args["status"]
        .as_str()
        .context("Expected recipient outcome")?;
    ensure!(
        matches!(status, "failed" | "received"),
        "Invalid recipient outcome"
    );
    let previous: String = db
        .query_row(
            "SELECT a.status FROM pv_notice_attempt a JOIN pv_notice n ON n.id=a.notice
         WHERE a.id=? AND n.task=? AND n.recipient=?",
            params![id, actor.task, receiver(actor)?],
            |r| r.get(0),
        )
        .optional()?
        .context("Attempt unavailable")?;
    ensure!(
        previous == "unknown" || previous == status,
        "Conflicting recipient receipt"
    );
    if previous == "unknown" {
        db.execute(
            "UPDATE pv_notice_attempt SET status=?,settled=unixepoch() WHERE id=?",
            params![status, id],
        )?;
    }
    Ok(json!({"attempt":id,"status":status}))
}
