use super::{field, surfaces::package};
use crate::store::get;
use anyhow::{ensure, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

pub(super) fn open_or_resume(
    tx: rusqlite::Transaction<'_>,
    task: &str,
    t: &Value,
    action: &str,
    args: &Value,
) -> Result<Value> {
    let delivery = get(&tx, "graph_delivery", field(args, "delivery")?)?;
    ensure!(
        delivery["task"] == task
            || (t["workspace"].is_string() && t["workspace"] == delivery["workspace"]),
        "Delivery outside workspace"
    );
    if action == "resume" {
        let result = crate::workspace::resume_graph(&tx, task, t, &delivery, &[])?;
        tx.commit()?;
        return Ok(result);
    }
    package(
        &tx,
        &delivery["bindings"],
        &delivery["package"],
        &Value::Null,
    )
}

pub(super) fn list(db: &Connection, task: &str, t: &Value) -> Result<Value> {
    let mut stmt=db.prepare("SELECT body FROM objects WHERE kind='graph_delivery' AND (json_extract(body,'$.task')=? OR json_extract(body,'$.workspace')=?) ORDER BY rowid DESC")?;
    let rows = stmt.query_map(rusqlite::params![task, t["workspace"].as_str()], |r| {
        r.get::<_, String>(0)
    })?;
    let mut records = vec![];
    for row in rows {
        records.push(serde_json::from_str::<Value>(&row?)?);
    }
    Ok(json!(records))
}
