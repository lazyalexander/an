//! The transaction boundary shared by the CLI and the task MCP gateway.
mod access;
mod attribution;
pub(crate) mod continuation;
mod correction;
mod domain;
mod evidence;
pub(crate) mod export;
pub(crate) mod flow;
mod handoff;
mod history;
pub(crate) use handoff::valid_endpoint;
pub(crate) use history::require_accepted;
mod inbox;
mod material;
mod notification;
pub(crate) mod publication;
mod query;
pub(crate) mod release;
pub(crate) mod review;
mod versions;
use crate::{
    instances,
    store::{canonical, connect, hash},
};
pub(crate) use access::scoped;
use access::Actor;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};

pub fn authenticate(db: &Connection, task: &str) -> Result<()> {
    Actor::authenticate(db, task).map(|_| ())
}

pub fn schema(db: &Connection) -> Result<()> {
    Ok(db.execute_batch(include_str!("schema.sql"))?)
}

pub fn execute(task: &str, action: &str, args: &Value) -> Result<Value> {
    let mut db = connect()?;
    let lock = instances::lock(&db, task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actor = Actor::authenticate(&tx, task)?;
    let result = match action {
        "inbox" => inbox::list(&tx, &actor)?,
        "submit-review" => {
            let submission: inbox::Submission = serde_json::from_value(args.clone())?;
            replay(
                &tx,
                &actor,
                &submission.idempotency_key,
                action,
                args,
                || inbox::submit(&tx, &actor, &submission.source),
            )?
        }
        "notify-correction" => correction::notify(&tx, &actor, args)?,
        "corrections" => correction::inspect(&tx, &actor, args)?,
        "notifications" => notification::list(&tx, &actor)?,
        "take-notification" => notification::take(&tx, &actor, args)?,
        "ack-notification" => notification::ack(&tx, &actor, args)?,
        "release" => release::submit(&tx, &actor, args)?,
        "request-review" => review::request(&tx, &actor, args)?,
        "read-review" => review::read(&tx, &actor, args)?,
        "sources" => query::trace(&tx, &actor, args, query::Direction::Sources)?,
        "impact" => query::trace(&tx, &actor, args, query::Direction::Impact)?,
        "handoff-block" => handoff::issue(&tx, &actor, args)?,
        "grant" => access::grant(&tx, &actor, args)?,
        "revoke" => access::revoke(&tx, &actor, args)?,
        "freeze" => evidence::freeze(&tx, &actor, args)?,
        "read-evidence" => evidence::read(&tx, &actor, args)?,
        "reopen-evidence" => evidence::reopen(&tx, &actor, args)?,
        "admit-material" => material::admit(&tx, &actor, args)?,
        "history" => history::view(
            &tx,
            &actor,
            args["cursor"].as_u64().context("Expected cursor")?,
        )?,
        "decide" => {
            let decision: domain::Decision = serde_json::from_value(args.clone())?;
            let key = decision.idempotency_key.clone();
            replay(&tx, &actor, &key, action, args, || {
                history::decide(&tx, &actor, decision)
            })?
        }
        "commit" => {
            let key = args["idempotency_key"]
                .as_str()
                .context("Expected idempotency key")?;
            replay(&tx, &actor, key, action, args, || {
                let batch = versions::parse(args.clone())?;
                let receipt = versions::commit(&tx, &actor, batch)?;
                history::proposed(&tx, task, &receipt)?;
                Ok(receipt)
            })?
        }
        _ => anyhow::bail!("Unknown provenance operation"),
    };
    tx.commit()?;
    drop(lock);
    if action == "decide" {
        if let Err(error) = crate::graph::wake(task) {
            eprintln!("Decision recorded. Scheduler will reconcile on next state read: {error}");
        }
    }
    Ok(result)
}

fn replay(
    tx: &Transaction,
    actor: &Actor,
    key: &str,
    action: &str,
    args: &Value,
    f: impl FnOnce() -> Result<Value>,
) -> Result<Value> {
    ensure!(!key.is_empty(), "Expected idempotency key");
    let digest = hash(canonical(&json!([action, args])).as_bytes());
    let prior: Option<(String, String)> = tx
        .query_row(
            "SELECT digest,receipt FROM pv_request WHERE task=? AND actor=? AND key=?",
            params![actor.task, actor.id, key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((stored, receipt)) = prior {
        ensure!(
            stored == digest,
            "Idempotency key reused with different content"
        );
        return Ok(serde_json::from_str(&receipt)?);
    }
    let receipt = f()?;
    tx.execute(
        "INSERT INTO pv_request VALUES(?,?,?,?,?)",
        params![actor.task, actor.id, key, digest, receipt.to_string()],
    )?;
    Ok(receipt)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod benchmark;
