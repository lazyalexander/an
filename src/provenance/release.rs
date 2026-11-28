//! Bind a delegated instruction to one immutable delivery attempt.
use super::{
    access::{require_active_grant, Actor, Right},
    domain::{BlockRef, VersionRef},
};
use crate::store::{find, get, put, uid};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

fn selection(node: &Value) -> Result<&Value> {
    let versions = node["versions"]
        .as_object()
        .context("Expected version selection")?;
    ensure!(
        versions.len() == 1 && node["inputs"].as_object().is_some_and(|m| m.len() == 1),
        "Reviewed publication requires one version and binding"
    );
    Ok(versions.values().next().unwrap())
}

pub(super) fn validate(db: &Connection, task: &str, node: &Value) -> Result<()> {
    let actor = Actor::authenticate(db, task)?;
    actor.controller()?;
    let report: BlockRef = serde_json::from_value(node["publication_review"].clone())?;
    let doc: VersionRef = serde_json::from_value(selection(node)?.clone())?;
    actor.require(
        db,
        Right::Read {
            artifact: report.artifact.clone(),
            version: report.version,
        },
    )?;
    ensure!(
        get(db, "task", task)?["workspace"].is_string(),
        "Expected workspace delivery"
    );
    ensure!(
        node.get("spool").is_none() && node.get("panel").is_none(),
        "Reviewed publication requires a plain document delivery"
    );
    let single_file: bool = db.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM pv_version v JOIN objects r
              ON r.kind='revision' AND r.id=json_extract(v.material,'$.revision')
            JOIN json_each(r.body) f
            WHERE v.artifact=? AND v.version=? AND v.task=?
            GROUP BY v.artifact,v.version
            HAVING count(*)=1 AND min(f.key)=json_extract(v.material,'$.file')
        )",
        params![doc.artifact.0, doc.version.0, task],
        |r| r.get(0),
    )?;
    ensure!(
        single_file,
        "Reviewed publication requires a single-file revision"
    );
    let covered: bool = db.query_row(
        include_str!("reviewed_document.sql"),
        params![
            doc.artifact.0,
            doc.version.0,
            report.artifact.0,
            report.version.0,
            report.block
        ],
        |r| r.get(0),
    )?;
    ensure!(
        covered,
        "Report must verify every declared block of this document version"
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Instruction {
    node: String,
    attempt: String,
    workspace: String,
    document: VersionRef,
}

pub(super) fn submit(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    let instruction: Instruction = serde_json::from_value(args.clone())?;
    let grant = actor
        .grant
        .as_ref()
        .context("Expected delegated publication")?;
    actor.require(
        db,
        Right::Release {
            node: instruction.node.clone(),
            attempt: instruction.attempt.clone(),
        },
    )?;
    let graph = get(db, "graph", &actor.task)?;
    let node = &graph["nodes"][&instruction.node];
    let state = &graph["activity"][&instruction.node];
    ensure!(
        state["id"] == instruction.attempt
            && get(db, "task", &actor.task)?["workspace"] == instruction.workspace
            && *selection(node)? == serde_json::to_value(&instruction.document)?,
        "Publication instruction outside approved scope"
    );
    if let Some(record) = find(db, "publication_release", &instruction.attempt)? {
        ensure!(record["grant"] == *grant, "Delivery already released");
        return Ok(record);
    }
    ensure!(
        state["status"] == "queued",
        "Delivery attempt is no longer queued"
    );
    let record = json!({"id":uid(),"task":actor.task,"node":instruction.node,"attempt":instruction.attempt,
        "workspace":instruction.workspace,"document":instruction.document,"report":node["publication_review"],"grant":grant,"actor":actor.id});
    put(db, "publication_release", &instruction.attempt, &record)?;
    Ok(record)
}

pub(crate) fn authorized(db: &Connection, task: &str, attempt: &str) -> Result<Option<Value>> {
    let Some(record) = find(db, "publication_release", attempt)? else {
        return Ok(None);
    };
    if record.get("confirmation").is_some() {
        ensure!(record["confirmed"] == true, "Human declined publication");
        ensure!(
            record["task"] == task
                && record["authority"]
                    == crate::store::hash(
                        crate::store::canonical(&crate::permission::stamp(db, task)?).as_bytes()
                    ),
            "Human publication approval expired"
        );
        return Ok(Some(record));
    }
    let grant = get(
        db,
        "provenance_grant",
        record["grant"]
            .as_str()
            .context("Missing publication grant")?,
    )?;
    require_active_grant(db, task, &grant)?;
    Ok(Some(record))
}
