//! The transaction boundary shared by the CLI and the task MCP gateway.
mod access;
mod decisions;
pub(crate) use decisions::continuation;
use decisions::{attribution, correction, history, notification};
mod domain;
mod evidence;
pub(crate) mod export;
pub(crate) mod flow;
mod handoff;
pub(crate) mod retention;
use crate::workspace::inbox;
pub(crate) use domain::BlockRef;
pub(crate) use handoff::valid_endpoint;
pub(crate) use history::require_accepted;
mod material;
mod operation;
use operation::Operation;
pub(crate) mod publication;
mod query;
pub(crate) mod release;
pub(crate) mod review;
mod snapshot;
mod versions;
use crate::{
    instances,
    store::{canonical, connect, hash},
};
pub(crate) use access::scoped;
pub(crate) use access::{Actor, Right};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};

pub(crate) fn block_registration(
    db: &Connection,
    task: &str,
    source: &BlockRef,
) -> Result<Option<(u64, Option<String>)>> {
    Ok(db
        .query_row(
            "SELECT registered_cursor,json_extract(material,'$.attempt')
        FROM pv_block JOIN pv_version USING(artifact,version)
        WHERE artifact=? AND version=? AND block=? AND task=?",
            params![source.artifact.0, source.version.0, source.block, task],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

pub fn authenticate(db: &Connection, task: &str) -> Result<()> {
    Actor::authenticate(db, task).map(|_| ())
}

pub fn schema(db: &Connection) -> Result<()> {
    Ok(db.execute_batch(include_str!("schema.sql"))?)
}

pub fn execute(task: &str, action: &str, args: &Value) -> Result<Value> {
    dispatch(task, action, args, false)
}

pub(crate) fn execute_mcp(task: &str, action: &str, args: &Value) -> Result<Value> {
    dispatch(task, action, args, true)
}

fn dispatch(task: &str, action: &str, args: &Value, from_mcp: bool) -> Result<Value> {
    // Reject CLI-only operations before opening an MCP application transaction.
    let operation = Operation::parse(action);
    if let (true, Ok(op)) = (from_mcp, &operation) {
        op.allow_mcp(scoped())?;
    }
    let mut db = connect()?;
    let lock = instances::lock(&db, task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let actor = Actor::authenticate(&tx, task)?;
    let op = operation?;
    let result = match op {
        Operation::FreezeExecution => snapshot::create(&tx, &actor, args)?,
        Operation::ReadExecution => snapshot::read(&tx, &actor, args)?,
        Operation::Inbox => inbox::list(&tx, &actor)?,
        Operation::ReviewInbox => {
            let review: inbox::Review = serde_json::from_value(args.clone())?;
            actor.require(&tx, Right::ReviewInbox)?;
            ensure!(inbox::is_primary(&tx, task)?, "Primary inbox outside task");
            replay(&tx, &actor, &review.idempotency_key, action, args, || {
                inbox::review(&tx, &actor, &review)
            })?
        }
        Operation::SubmitReview => {
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
        Operation::NotifyCorrection => correction::notify(&tx, &actor, args)?,
        Operation::Corrections => correction::inspect(&tx, &actor, args)?,
        Operation::Notifications => notification::list(&tx, &actor)?,
        Operation::TakeNotification => notification::take(&tx, &actor, args)?,
        Operation::AckNotification => notification::ack(&tx, &actor, args)?,
        Operation::Release => release::submit(&tx, &actor, args)?,
        Operation::RequestReview => review::request(&tx, &actor, args)?,
        Operation::ReadReview => review::read(&tx, &actor, args)?,
        Operation::Sources => query::trace(&tx, &actor, args, query::Direction::Sources)?,
        Operation::Impact => query::trace(&tx, &actor, args, query::Direction::Impact)?,
        Operation::HandoffBlock => handoff::issue(&tx, &actor, args)?,
        Operation::Grant => access::grant(&tx, &actor, args)?,
        Operation::Revoke => access::revoke(&tx, &actor, args)?,
        Operation::Freeze => evidence::freeze(&tx, &actor, args)?,
        Operation::ReadEvidence => evidence::read(&tx, &actor, args)?,
        Operation::ReopenEvidence => evidence::reopen(&tx, &actor, args)?,
        Operation::AdmitMaterial => material::admit(&tx, &actor, args)?,
        Operation::History => history::view(
            &tx,
            &actor,
            args["cursor"].as_u64().context("Expected cursor")?,
        )?,
        Operation::Decide => {
            let decision: domain::Decision = serde_json::from_value(args.clone())?;
            let key = decision.idempotency_key.clone();
            replay(&tx, &actor, &key, action, args, || {
                history::decide(&tx, &actor, decision)
            })?
        }
        Operation::Commit => {
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
    };
    tx.commit()?;
    drop(lock);
    if op == Operation::Decide {
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
