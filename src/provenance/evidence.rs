//! A frozen log names the exact captured bytes and the run that produced them.
use super::access::{Actor, Right};
use crate::{
    permission,
    store::{canonical, find, get, hash, put, revision, root},
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

pub fn freeze(tx: &Transaction, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let run = args["attempt"].as_str().context("Expected run attempt")?;
    let record = get(tx, "run", run)?;
    ensure!(
        record["task"] == actor.task
            && matches!(
                record["status"].as_str(),
                Some("completed" | "awaiting_publication" | "delivered" | "failed" | "cancelled")
            ),
        "Expected a settled task run"
    );
    let start = args["start"].as_u64().context("Expected start offset")?;
    let length = args["length"]
        .as_u64()
        .filter(|n| *n <= 65536)
        .context("Expected length up to 65536")?;
    if let Some(p) = permission::gate(
        tx,
        &actor.task,
        "log",
        json!([run, start, length, "freeze"]),
        json!([run]),
        args["request_id"].as_str(),
        None,
    )? {
        return Ok(p);
    }
    let mut file = File::open(root()?.join("runs").join(run).join("log"))
        .context("Captured log unavailable")?;
    let size = file.metadata()?.len();
    ensure!(start <= size, "Range outside captured log");
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(length).read_to_end(&mut bytes)?;
    let end = start + bytes.len() as u64;
    let rev = revision(tx, &json!({"log":STANDARD.encode(bytes)}))?;
    let id = hash(canonical(&json!([actor.task, run, start, end, rev])).as_bytes());
    let frozen = json!({"snapshot":id,"task":actor.task,"run":run,"revision":rev,"start":start,"end":end,
        "captured_size":size,"partial":start>0 || end<size,"short_read":end-start<length,"capture_truncated":record["log_truncated"]});
    put(tx, "frozen_evidence", &id, &frozen)?;
    Ok(frozen)
}

pub fn read(tx: &Transaction, actor: &Actor, args: &Value) -> Result<Value> {
    let id = args["snapshot"].as_str().context("Expected snapshot ID")?;
    actor.require(
        tx,
        Right::Evidence {
            snapshot: id.into(),
        },
    )?;
    let record = get(tx, "frozen_evidence", id)?;
    if actor.grant.is_none() {
        if let Some(p) = permission::gate(
            tx,
            &actor.task,
            "log",
            json!([id, "read"]),
            json!([record["revision"]]),
            args["request_id"].as_str(),
            None,
        )? {
            return Ok(p);
        }
    }
    Ok(
        json!({"snapshot":record,"base64":get(tx,"revision",record["revision"].as_str().context("Missing snapshot revision")?)?["log"]}),
    )
}

/// A new turn may reauthorize a specific retained log through a published
/// document's citations. The request reveals no other revisions or destinations.
pub fn reopen(tx: &Transaction, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let id = args["snapshot"].as_str().context("Expected snapshot")?;
    let delivery = get(
        tx,
        "graph_delivery",
        args["delivery"].as_str().context("Expected delivery")?,
    )?;
    let workspace = get(tx, "task", &actor.task)?["workspace"].clone();
    ensure!(
        workspace.is_string() && delivery["workspace"] == workspace,
        "Evidence unavailable"
    );
    let reachable:bool=tx.query_row("WITH RECURSIVE lineage(a,v,b) AS (
        SELECT b.artifact,b.version,b.block FROM pv_published p JOIN pv_block b USING(artifact,version)
        WHERE p.delivery=?1
        UNION SELECT e.source,e.source_version,e.source_block FROM lineage l JOIN pv_edge e
        ON (l.a,l.v,l.b)=(e.artifact,e.version,e.block))
        SELECT EXISTS(SELECT 1 FROM lineage l JOIN pv_version v ON (l.a,l.v)=(v.artifact,v.version)
        JOIN json_each(v.evidence) s ON s.value=?2)",rusqlite::params![args["delivery"].as_str(),id],|r|r.get(0))?;
    ensure!(reachable, "Evidence unavailable");
    let stamp = permission::stamp(tx, &actor.task)?;
    if let Some(p) = permission::gate(
        tx,
        &actor.task,
        "log",
        json!([id, "reopen"]),
        json!([args["delivery"]]),
        args["request_id"].as_str(),
        None,
    )? {
        return Ok(p);
    }
    put(
        tx,
        "evidence_access",
        &canonical(&json!([actor.task, id])),
        &stamp,
    )?;
    Ok(json!({"snapshot":id,"authorized":true}))
}

pub fn reopened(db: &rusqlite::Connection, task: &str, id: &str) -> Result<bool> {
    Ok(find(db, "evidence_access", &canonical(&json!([task, id])))?
        == Some(permission::stamp(db, task)?))
}
