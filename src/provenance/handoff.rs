//! Project a single immutable text block into an ordinary one-use spool ticket.
use super::{
    access::{Actor, Right},
    domain::{valid_name, BlockRef},
};
use crate::{
    permission,
    store::{get, put, revision, uid},
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Transaction};
use serde_json::{json, Value};

pub(crate) fn valid_endpoint(value: &Value) -> bool {
    serde_json::from_value::<BlockRef>(value.clone())
        .is_ok_and(|b| valid_name(&b.artifact.0) && b.version.0 > 0 && valid_name(&b.block))
}

pub fn issue(tx: &Transaction, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let source: BlockRef = serde_json::from_value(args["source"].clone())?;
    let recipient = args["recipient"]
        .as_str()
        .context("Expected recipient spool")?;
    ensure!(
        get(tx, "task", &actor.task)?["work"]
            .get(recipient)
            .is_some(),
        "Recipient outside task"
    );
    actor.require(
        tx,
        Right::Read {
            artifact: source.artifact.clone(),
            version: source.version,
        },
    )?;
    let text: String = tx
        .query_row(
            "SELECT json_extract(body,'$.text') FROM pv_block
         WHERE artifact=? AND version=? AND block=?",
            params![source.artifact.0, source.version.0, source.block],
            |row| row.get(0),
        )
        .context("Block unavailable")?;
    let route = json!(["block_input", source, recipient]);
    if let Some(pending) = permission::gate(
        tx,
        &actor.task,
        "block_input",
        json!([source, recipient]),
        json!(["block.txt"]),
        args["request_id"].as_str(),
        Some(route),
    )? {
        return Ok(pending);
    }
    let rev = revision(tx, &json!({"block.txt":STANDARD.encode(text.as_bytes())}))?;
    let id = format!("handoff-{}", uid());
    put(
        tx,
        "handoff",
        &id,
        &json!({"task":actor.task,"recipient":recipient,"block":source,
        "revision":rev,"stamp":permission::stamp(tx,&actor.task)?}),
    )?;
    Ok(json!({"handoff":id,"revision":rev}))
}
