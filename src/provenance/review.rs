//! A workspace owner can release one retained decision version for a task's review.
use super::{access::Actor, domain::VersionRef};
use crate::{
    permission,
    store::{find, get, hash, put, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Transaction};
use serde_json::{json, Value};

#[derive(Clone, clap::ValueEnum)]
pub enum Disposition {
    Show,
    Approve,
    Revoke,
    Deny,
}

fn review_scope(tx: &Transaction, task: &str) -> Result<String> {
    let scope = permission::delegation_scope(tx, task)?;
    Ok(hash(crate::store::canonical(&scope).as_bytes()))
}

pub(super) fn request(tx: &Transaction, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let target: VersionRef = serde_json::from_value(args["target"].clone())?;
    let delivery = args["delivery"].as_str().context("Expected delivery")?;
    let workspace = get(tx, "task", &actor.task)?["workspace"].clone();
    ensure!(
        workspace.is_string() && get(tx, "graph_delivery", delivery)?["workspace"] == workspace,
        "Delivery unavailable"
    );
    let scope = review_scope(tx, &actor.task)?;
    let id =
        hash(crate::store::canonical(&json!([actor.task, scope, delivery, target])).as_bytes());
    if let Some(existing) = find(tx, "decision_review", &id)? {
        return Ok(existing);
    }
    let row = json!({"id":id,"workspace":workspace,"task":actor.task,"scope":scope,"delivery":delivery,"target":target,"status":"pending"});
    put(tx, "decision_review", &id, &row)?;
    Ok(row)
}

pub(crate) fn settle(
    tx: &Transaction,
    workspace: &str,
    action: Disposition,
    id: &str,
    key: &str,
) -> Result<Value> {
    let owner = hash(key.as_bytes());
    ensure!(
        find(tx, "review_owner", workspace)? == Some(json!(owner)),
        "Workspace review owner required"
    );
    let mut row = get(tx, "decision_review", id)?;
    ensure!(row["workspace"] == workspace, "Review unavailable");
    let status = match action {
        Disposition::Show => return Ok(row),
        Disposition::Approve => "approved",
        Disposition::Revoke => "revoked",
        Disposition::Deny => "denied",
    };
    if row["status"] == status {
        return Ok(row);
    }
    if status == "revoked" {
        ensure!(row["status"] == "approved", "Expected approved review");
    } else {
        ensure!(row["status"] == "pending", "Review already settled");
    }
    if status == "approved" {
        let reachable: bool = tx.query_row(
            include_str!("review_target.sql"),
            params![
                row["delivery"].as_str(),
                row["target"]["artifact"].as_str(),
                row["target"]["version"].as_u64(),
                workspace
            ],
            |r| r.get(0),
        )?;
        ensure!(reachable, "Decision unavailable for this delivery");
    }
    // Keep the approval receipt when a later revocation is recorded.
    let at: i64 = tx.query_row("SELECT unixepoch()", [], |r| r.get(0))?;
    row[status] = json!({"receipt":uid(),"owner":owner,"at":at});
    row["status"] = json!(status);
    put(tx, "decision_review", id, &row)?;
    Ok(row)
}

pub(super) fn read(tx: &Transaction, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let row = get(
        tx,
        "decision_review",
        args["id"].as_str().context("Expected review ID")?,
    )?;
    ensure!(
        row["task"] == actor.task
            && row["scope"] == review_scope(tx, &actor.task)?
            && row["status"] == "approved",
        "Review unavailable"
    );
    let mut stmt = tx.prepare("SELECT block,json_extract(body,'$.text') FROM pv_block WHERE artifact=? AND version=? ORDER BY rowid")?;
    let blocks = stmt
        .query_map(
            params![
                row["target"]["artifact"].as_str(),
                row["target"]["version"].as_u64()
            ],
            |r| Ok(json!({"block":r.get::<_,String>(0)?,"text":r.get::<_,String>(1)?})),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"target":row["target"],"approval":row["approved"],"blocks":blocks}))
}
