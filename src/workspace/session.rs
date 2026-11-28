//! Move a primary binding without recreating the task or its inbox.
use super::binding::{guard, Selector};
use crate::{
    instances, permission,
    store::{find, get, put, uid},
};
use anyhow::{ensure, Context, Result};
use clap::Args;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};

#[derive(Args)]
pub(super) struct Target {
    workspace: String,
    #[arg(long)]
    task: String,
    #[command(flatten)]
    session: Selector,
}

pub(super) fn transfer(
    db: &mut Connection,
    target: &Target,
    digest: Option<&str>,
) -> Result<Value> {
    let workspace = &target.workspace;
    let task = &target.task;
    let previous = get(db, "workspace_primary", workspace)?;
    let from = previous.as_str().context("Invalid primary binding")?;
    let to = target
        .session
        .identity()?
        .context("Expected destination session")?;
    ensure!(from != to, "Destination is already primary");
    // MCP takes its binding lock before its task lock. Acquire both exclusive
    // binding guards before opening a write transaction or waiting on the task.
    let _from = guard(from, true)?;
    let _to = guard(&to, true)?;
    let _task = instances::lock(db, task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    permission::controller(&tx, task)?;
    ensure!(
        get(&tx, "workspace_primary", workspace)? == previous,
        "Primary binding changed"
    );
    let binding = get(&tx, "binding", from)?;
    let record = get(&tx, "task", task)?;
    ensure!(
        binding["task"] == task.as_str()
            && binding["workspace"] == workspace.as_str()
            && record["workspace"] == workspace.as_str()
            && record["closed"] != true,
        "Expected open primary task"
    );
    if let Some(existing) = find(&tx, "binding", &to)? {
        ensure!(
            existing["workspace"] == workspace.as_str() && existing["task"].is_null(),
            "Destination already has a workspace or task"
        );
    }
    let prior_agent = permission::controller_identity(&tx, task)?;
    let agent = if let Some(digest) = digest {
        permission::transfer_controller(&tx, task, digest)?;
        let identity = format!("controller:{}", uid());
        put(&tx, "task_controller_identity", task, &json!(identity))?;
        identity
    } else {
        prior_agent.clone()
    };
    put(&tx, "workspace_primary_agent", workspace, &json!(agent))?;
    put(&tx, "binding", &to, &binding)?;
    tx.execute("DELETE FROM objects WHERE kind='binding' AND id=?", [from])?;
    put(&tx, "workspace_primary", workspace, &json!(to))?;
    let id = uid();
    let receipt = json!({"id":id,"workspace":workspace,"task":task,
        "action":if digest.is_some() {"takeover"} else {"continue"},
        "from_session":from,"to_session":to,"from_agent":prior_agent,"to_agent":agent});
    put(&tx, "workspace_transfer", &id, &receipt)?;
    tx.commit()?;
    Ok(receipt)
}
