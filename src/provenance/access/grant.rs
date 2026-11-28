use super::{Actor, Right};
use crate::{
    permission,
    store::{get, hash, put, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
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
        ensure!(
            !matches!(right, Right::Release { .. }) || target == actor.task,
            "Release requires the delivery task"
        );
        if let Right::Notify {
            artifact,
            version,
            recipient,
        } = right
        {
            ensure!(
                target == actor.task,
                "Notification requires the decision task"
            );
            crate::provenance::notification::recipient(db, target, recipient, artifact, *version)?;
        }
        if matches!(right, Right::ReceiveNotice { .. }) {
            ensure!(
                target == actor.task,
                "Notification requires the decision task"
            );
        }
        actor.require(db, right.clone())?;
        if let Right::Read { artifact, version }
        | Right::ReceiveNotice { artifact, version }
        | Right::Decide { artifact, version } = right
        {
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
    if rights.iter().any(|r| {
        matches!(
            r,
            Right::ReadInbox | Right::ReviewInbox | Right::SubmitReview { .. }
        )
    }) {
        ensure!(
            target == actor.task && rights.len() == 1,
            "Inbox delegation requires a single task-local right"
        );
    }
    if let Some(id) = rights.iter().find_map(|right| match right {
        Right::InspectSnapshot { snapshot } => Some(snapshot),
        _ => None,
    }) {
        ensure!(
            rights.len() == 1,
            "Snapshot reader requires an exclusive read-only grant"
        );
        if let Some(request) = crate::provenance::snapshot::authorize(
            db,
            actor,
            id,
            target,
            args["request_id"].as_str(),
        )? {
            return Ok(request);
        }
    }
    if let [Right::SubmitReview { source }] = rights.as_slice() {
        if let Some(pending) =
            crate::workspace::inbox::authorize(db, actor, source, args["request_id"].as_str())?
        {
            return Ok(pending);
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
