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
        db.execute("INSERT INTO pv_flow(task,revision,phase,attempt,target,source_attempt,source_input,delivery,time_ms)
            VALUES(?,?,?,?,?,?,?,?,unixepoch('subsec')*1000)",params![task,binding["revision"].as_str().context("Invalid revision")?,status,attempt,target,binding["run"].as_str().or(binding["attempt"].as_str()),binding["source"].as_str(),matches!(phase,Phase::Published).then_some(attempt)])?;
    }
    Ok(())
}
