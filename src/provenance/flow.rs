//! Record the host observation at each transfer boundary.
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde_json::Value;

pub(crate) enum Phase {
    Authorized,
    HandedOff,
    Published,
}

pub fn record(
    db: &Connection,
    task: &str,
    phase: Phase,
    attempt: &str,
    target: &str,
    bindings: &Value,
) -> Result<()> {
    let status = match phase {
        Phase::Authorized => "authorized",
        Phase::HandedOff => "handed_off",
        Phase::Published => "published",
    };
    for binding in bindings
        .as_object()
        .context("Expected bound inputs")?
        .values()
    {
        let revision = binding["revision"].as_str().context("Invalid revision")?;
        let producer = binding["run"].as_str().or(binding["attempt"].as_str());
        let delivery = matches!(phase, Phase::Published).then_some(attempt);
        db.execute(
            "INSERT INTO pv_flow(task,revision,phase,attempt,target,source_attempt,source_input,delivery,time_ms)
             VALUES(?,?,?,?,?,?,?,?,unixepoch('subsec')*1000)",
            params![task,revision,status,attempt,target,producer,binding["source"].as_str(),delivery],
        )?;
    }
    Ok(())
}

pub fn produced(db: &Connection, task: &str, attempt: &str, revision: &str) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO pv_product VALUES(?,?,?)",
        params![task, attempt, revision],
    )?;
    Ok(())
}
