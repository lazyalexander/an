//! A continuation waits on one immutable decision. Only an accepted version
//! releases the driver. Rejection settles it before it can acquire an instance.
use crate::permission;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub fn declare(db: &Connection, task: &str, node: &Value) -> Result<()> {
    permission::controller(db, task)?;
    let guard = &node["await_decision"];
    ensure!(
        node["kind"] == "expand" && guard.as_object().is_some_and(|m| m.len() == 2),
        "Expected decision-gated continuation"
    );
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pv_version v JOIN pv_artifact a ON a.id=v.artifact
        WHERE v.task=? AND v.artifact=? AND v.version=? AND a.kind='decision')",
        params![task, guard["artifact"].as_str(), guard["version"].as_u64()],
        |r| r.get(0),
    )?;
    ensure!(exists, "Decision outside task");
    Ok(())
}

pub fn accepted(db: &Connection, guard: &Value) -> Result<Option<String>> {
    let result:Option<(String,String)>=db.query_row("SELECT receipt,json_extract(body,'$.choice') FROM pv_choice WHERE artifact=? AND version=?",
        params![guard["artifact"].as_str(),guard["version"].as_u64()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    result
        .map(|(id, status)| {
            (status == "accept")
                .then_some(id)
                .context("Required decision rejected")
        })
        .transpose()
}
