use super::lifecycle::reconcile;
use super::{can_cancel_run, owned, save, string};
use crate::engine::{capacity, InstanceIntent, Termination};
use crate::store::{get, hash, put, root, uid};
use crate::{artifacts, permission};
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{ErrorKind, Read, Seek, SeekFrom};

pub(super) fn publish(
    db: &mut Connection,
    task: &str,
    t: &Value,
    op: &str,
    args: &Value,
) -> Result<Value> {
    let id = string(args, "run")?;
    let mut r = owned(db, task, id)?;
    ensure!(
        r["status"] == "awaiting_publication",
        "Expected private candidate"
    );
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let files = get(&tx, "revision", string(&r, "candidate")?)?;
    let files = files.as_object().context("Invalid revision")?;
    let mut manifest = vec![];
    let mut names = files.keys().collect::<Vec<_>>();
    names.sort();
    for name in &names {
        let data = artifacts::bytes(&files[*name])?;
        manifest.push(json!([name, data.len(), hash(&data)]));
    }
    let dest = if op == "handoff" {
        let s = string(args, "recipient_spool")?;
        ensure!(t["work"].get(s).is_some(), "Recipient outside task");
        s
    } else {
        t["workspace"].as_str().unwrap_or(task)
    };
    let result = if let Some(p) = permission::gate(
        &tx,
        task,
        op,
        json!([r["spool"], names, dest]),
        json!([id, manifest]),
        args["request_id"].as_str(),
        Some(json!([op, r["spool"], dest])),
    )? {
        p
    } else if op == "handoff" {
        let key = format!("handoff-{}", uid());
        put(
            &tx,
            "handoff",
            &key,
            &json!({"task":task,"recipient":dest,"source":id,"revision":r["candidate"],"stamp":permission::stamp(&tx,task)?}),
        )?;
        json!({"handoff":key})
    } else {
        r["output"] = r.as_object_mut().unwrap().remove("candidate").unwrap();
        r["status"] = json!("delivered");
        save(&tx, "run", &r)?;
        json!({"run":id,"status":"delivered"})
    };
    tx.commit()?;
    Ok(result)
}

pub(super) fn log(db: &mut Connection, task: &str, args: &Value) -> Result<Value> {
    let id = string(args, "run")?;
    let r = owned(db, task, id)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(p) = permission::gate(
        &tx,
        task,
        "log",
        json!([id, "agent"]),
        json!({}),
        args["request_id"].as_str(),
        None,
    )? {
        tx.commit()?;
        return Ok(p);
    }
    let mut bytes = vec![];
    match File::open(root()?.join("runs").join(id).join("log")) {
        Ok(mut f) => {
            f.seek(SeekFrom::Start(f.metadata()?.len().saturating_sub(65536)))?;
            f.take(65536).read_to_end(&mut bytes)?;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut result = String::from_utf8_lossy(&bytes).into_owned();
    if let Some(err) = r["error"].as_str() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&format!("An: {err}"));
    }
    tx.commit()?;
    Ok(json!(result))
}

#[derive(PartialEq, Eq)]
pub(super) enum RunAction {
    Inspect,
    Cancel,
}

pub(super) fn status(
    db: &mut Connection,
    task: &str,
    action: RunAction,
    lock: &File,
    args: &Value,
) -> Result<Value> {
    let id = string(args, "run")?;
    let mut r = owned(db, task, id)?;
    if r.get("instance").is_none() {
        if action == RunAction::Cancel && r["status"] == "running" {
            r["status"] = json!("cancel_pending");
        }
        if matches!(r["status"].as_str(), Some("running" | "cancel_pending")) {
            if let Some(pid) = r["worker_pid"].as_i64() {
                if unsafe { libc::kill(pid as i32, 0) } < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                {
                    r["status"] = json!("unknown");
                }
            }
        }
        put(db, "run", id, &r)?;
        return Ok(r);
    }
    let mut i = get(db, "instance", string(&r, "instance")?)?;
    if action == RunAction::Cancel
        && matches!(r["status"].as_str(), Some("running" | "unknown"))
        && can_cancel_run(&i, id)
    {
        let tx = db.transaction()?;
        if InstanceIntent::recorded(&i) == Some(InstanceIntent::Load) {
            i["intent"] = json!(InstanceIntent::Stop);
            save(&tx, "instance", &i)?;
        }
        r["status"] = json!("cancel_pending");
        save(&tx, "run", &r)?;
        tx.commit()?;
    }
    let termination = reconcile(db, &mut i, lock)?;
    let mut r = owned(db, task, id)?;
    if r["status"] == "cancel_pending"
        && i["active"].is_null()
        && termination == Termination::Confirmed
    {
        r["status"] = json!("cancelled");
        put(db, "run", id, &r)?;
    }
    let mut status = json!({"id":id,"status":r["status"],"spool":r["spool"]});
    if r["capacity_managed"] == true && r["status"] == "running" {
        if let Some(position) = capacity::position(db, id)? {
            status["waiting"] = json!({"reason":"execution_capacity","position":position});
        }
    }
    Ok(status)
}
