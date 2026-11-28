use crate::engine::InstanceIntent;
use crate::runtime;
use crate::store::{connect, find, get, put, root};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::path::PathBuf;

mod inputs;
mod lifecycle;
mod runs;
mod submission;
mod worker;

pub(crate) use lifecycle::{finish_unloads, request_task_unload};
use lifecycle::{load, reconcile};
use submission::submit;
use worker::spawn_worker;
pub use worker::worker;

fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().with_context(|| format!("Expected {k}"))
}
pub(crate) fn lock(db: &Connection, task: &str) -> Result<File> {
    get(db, "task", task)?;
    ensure!(
        !task.contains('/') && task != "." && task != "..",
        "Invalid task ID"
    );
    let dir = root()?.join("locks");
    fs::create_dir_all(&dir)?;
    let f = File::options()
        .create(true)
        .append(true)
        .open(dir.join(task))?;
    f.lock()?;
    Ok(f)
}
fn path(i: &Value) -> Result<PathBuf> {
    Ok(root()?.join("instances").join(string(i, "id")?))
}
fn save(db: &Connection, kind: &str, r: &Value) -> Result<()> {
    put(db, kind, string(r, "id")?, r)
}
fn items(db: &Connection, kind: &str, task: &str) -> Result<Vec<Value>> {
    let mut q = db.prepare(
        "SELECT body FROM objects WHERE kind=? AND json_extract(body,'$.task')=? ORDER BY rowid",
    )?;
    let mut rows = q.query([kind, task])?;
    let mut out = vec![];
    while let Some(row) = rows.next()? {
        out.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
    }
    Ok(out)
}
fn owned(db: &Connection, task: &str, id: &str) -> Result<Value> {
    let r = get(db, "run", id)?;
    ensure!(r["task"] == task, "Run outside task");
    Ok(r)
}

/// A worker can clear active after it records an uncertain guest exit. Its instance
/// then keeps the stop intent through reconciliation, even after the guest is gone.
pub(crate) fn can_cancel_run(instance: &Value, run: &str) -> bool {
    instance["active"] == run
        || (instance["active"].is_null()
            && InstanceIntent::recorded(instance).is_some_and(InstanceIntent::is_shutdown))
}

fn task_state(db: &mut Connection, task: &str, t: &Value, lock: &File) -> Result<Value> {
    let mut result = t.clone();
    let mut instances = items(db, "instance", task)?;
    for i in &mut instances {
        reconcile(db, i, lock)?;
    }
    let mut spools = json!({});
    for s in t["work"].as_object().context("Invalid task")?.keys() {
        spools[s] = runtime::summary(db, s)?;
    }
    let mut runs = json!({});
    let mut stmt = db.prepare("SELECT id, json_extract(body, '$.status') FROM objects WHERE kind='run' AND json_extract(body, '$.task')=?")?;
    let mut rows = stmt.query([task])?;
    while let Some(row) = rows.next()? {
        runs[row.get::<_, String>(0)?] = json!(row.get::<_, String>(1)?);
    }
    result["task"] = json!(task);
    result["spools"] = spools;
    result["runs"] = runs;
    result["instances"] = json!(instances);
    Ok(result)
}

pub fn dispatch(task: &str, op: &str, args: &Value) -> Result<Value> {
    ensure!(
        !args
            .as_object()
            .is_some_and(|a| a.keys().any(|k| k.starts_with('_'))),
        "Reserved instance argument"
    );
    let mut db = connect()?;
    let lock = lock(&db, task)?;
    dispatch_locked(&mut db, task, op, args, &lock)
}
pub(crate) fn dispatch_locked(
    db: &mut Connection,
    task: &str,
    op: &str,
    args: &Value,
    lock: &File,
) -> Result<Value> {
    let t = get(db, "task", task)?;
    if t["closed"] == true {
        ensure!(
            matches!(op, "state" | "status" | "unload" | "skill" | "log"),
            "Task is closed"
        );
    }
    if matches!(op, "load" | "run") {
        ensure!(
            !find(db, "graph", task)?
                .as_ref()
                .is_some_and(crate::graph::paused),
            "Task paused"
        );
    }
    match op {
        "load" => {
            return load(
                db,
                task,
                &t,
                string(args, "spool")?,
                lock,
                args["_parallel"] == true,
                args["_driver"].as_str(),
            )
        }
        "skill" => {
            let s = string(args, "spool")?;
            ensure!(t["work"].get(s).is_some(), "Spool outside task");
            return runtime::require(db, s);
        }
        "state" => return task_state(db, task, &t, lock),
        "publish" | "handoff" => return runs::publish(db, task, &t, op, args),
        "cancel" => return runs::status(db, task, runs::RunAction::Cancel, lock, args),
        "status" => return runs::status(db, task, runs::RunAction::Inspect, lock, args),
        "log" => return runs::log(db, task, args),
        "run" | "unload" => {}
        _ => bail!("Unknown operation {op}"),
    }
    let mut i = if let Some(id) = args["instance"].as_str() {
        let i = get(db, "instance", id)?;
        ensure!(
            i["task"] == task && (args["spool"].is_null() || i["spool"] == args["spool"]),
            "Instance outside requested task or spool"
        );
        i
    } else {
        items(db, "instance", task)?
            .into_iter()
            .rev()
            .find(|i| {
                i["spool"] == args["spool"]
                    && i["status"] != "unloaded"
                    && (op != "run" || i["dedicated"] != true)
            })
            .context("Load the spool first")?
    };
    if op == "run" && i["dedicated"] == true {
        if i["owner"].is_null() && args["_graph_run"].is_string() {
            // Upgrade previously reserved instances only from durable graph ownership.
            let graph = get(db, "graph", task)?;
            ensure!(
                graph["activity"]
                    .as_object()
                    .is_some_and(
                        |activity| activity
                            .iter()
                            .any(|(node, a)| graph["nodes"][node]["kind"] == "expand"
                                && a["id"] == args["_graph_run"]
                                && a["instance"] == i["id"])
                    ),
                "Missing driver reservation"
            );
            i["owner"] = args["_graph_run"].clone();
            save(db, "instance", &i)?;
        }
        ensure!(
            i["owner"].is_string() && i["owner"] == args["_graph_run"],
            "Instance reserved for driver"
        );
    }
    if op == "unload" && i["status"] != "unloaded" {
        i["intent"] = json!("unload");
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(id) = i["active"].as_str() {
            let mut r = owned(&tx, task, id)?;
            r["status"] = json!("cancel_pending");
            save(&tx, "run", &r)?;
        }
        save(&tx, "instance", &i)?;
        tx.commit()?;
    }
    reconcile(db, &mut i, lock)?;
    if op == "unload" {
        return Ok(i);
    }
    ensure!(
        i["disk_mib"].is_number(),
        "Unload and reload this instance to apply its disk budget"
    );
    let admission = runtime::require(db, string(&i, "spool")?)?;
    ensure!(
        i["platform"] == admission["platform"],
        "Reload instance on admitted platform"
    );
    submit(
        db,
        task,
        &t,
        &mut i,
        args,
        &admission["manifest"],
        spawn_worker,
    )
}

#[cfg(test)]
mod tests;
