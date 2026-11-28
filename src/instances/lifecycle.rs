use super::{items, path, save, string};
use crate::engine::{
    apple, capacity, observe, storage, InstanceIntent, InstanceStatus, Termination,
};
use crate::store::uid;
use crate::store::{find, get, root};
use crate::{artifacts, runtime};
use anyhow::{bail, ensure, Context, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::time::{SystemTime, UNIX_EPOCH};

/// Called in the same transaction as task closure. A later finish can retry cleanup.
pub(crate) fn request_task_unload(db: &Connection, task: &str) -> Result<()> {
    for mut instance in items(db, "instance", task)? {
        if instance["status"] != "unloaded" {
            ensure!(
                instance["active"].is_null(),
                "Settle the active instance before closing"
            );
            instance["intent"] = json!("unload");
            save(db, "instance", &instance)?;
        }
    }
    Ok(())
}

pub(crate) fn finish_unloads(db: &mut Connection, task: &str, lock: &File) -> Result<Value> {
    let mut pending = Vec::new();
    for mut instance in items(db, "instance", task)? {
        if instance["status"] != "unloaded" && instance["intent"] == "unload" {
            reconcile(db, &mut instance, lock)?;
            if instance["status"] != "unloaded" {
                pending.push(instance["id"].clone());
            }
        }
    }
    if pending.is_empty() {
        crate::store::gc::after_finish(db, task)?;
    }
    Ok(json!({"closed":true,"cleanup_pending":pending}))
}

pub(super) fn reconcile(db: &mut Connection, i: &mut Value, f: &File) -> Result<Termination> {
    if i["status"] == "unloaded" {
        return Ok(
            if InstanceIntent::recorded(i).is_some_and(InstanceIntent::is_shutdown) {
                Termination::Confirmed
            } else {
                Termination::Unconfirmed
            },
        );
    }
    let actual = observe(i, f);
    i["checked_at"] = json!(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64());
    if let Some(id) = i["active"].as_str() {
        let lease = File::options()
            .create(true)
            .append(true)
            .open(root()?.join("runs").join(id).join("lease"))?;
        let mut run = get(db, "run", id)?;
        if lease.try_lock().is_ok() && run["status"] == "running" {
            run["status"] = json!("unknown");
            save(db, "run", &run)?;
        }
    }
    let settled = crate::engine::settle(i, f, actual)?;
    i["status"] = json!(settled.status);
    if settled.status == InstanceStatus::Unloaded {
        if let Err(e) = storage::remove(&path(i)?, f) {
            i["status"] = json!("cleanup_pending");
            i["intent"] = json!("unload");
            i["error"] = json!(e.to_string());
        }
    }
    i["observed"] = json!(settled.guest);
    save_observation(db, i, settled.termination)?;
    Ok(settled.termination)
}

pub(super) fn save_observation(
    db: &mut Connection,
    i: &mut Value,
    termination: Termination,
) -> Result<()> {
    // Reserve the writer before reading the run. A deferred read cannot upgrade
    // while a competing queue worker holds SQLite's write reservation.
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if termination == Termination::Confirmed {
        if let Some(id) = i["unsettled_capacity"].as_str() {
            capacity::release(&tx, id)?;
            i["unsettled_capacity"] = Value::Null;
        }
        if let Some(id) = i["active"].as_str() {
            let mut r = get(&tx, "run", id)?;
            if r["capacity_managed"] == true {
                capacity::release(&tx, id)?;
            }
            r["status"] = json!("cancelled");
            save(&tx, "run", &r)?;
            i["active"] = Value::Null;
        }
    }
    save(&tx, "instance", i)?;
    tx.commit()?;
    Ok(())
}
pub(super) fn load(
    db: &mut Connection,
    task: &str,
    t: &Value,
    image: &str,
    f: &File,
    parallel: bool,
    owner: Option<&str>,
) -> Result<Value> {
    let dedicated = owner.is_some();
    let work = t["work"][image].as_str().context("Spool outside task")?;
    ensure!(
        !dedicated
            || !t["editable"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(image))),
        "Driver requires a read-only spool"
    );
    let platform = runtime::require(db, image)?["platform"].clone();
    let graph = if parallel {
        find(db, "graph", task)?
    } else {
        None
    };
    for mut i in items(db, "instance", task)? {
        if i["spool"] != image || i["status"] == "unloaded" || dedicated || i["dedicated"] == true {
            continue;
        }
        reconcile(db, &mut i, f)?;
        if i["status"] == "running" {
            if parallel
                && (!i["active"].is_null()
                    || graph
                        .as_ref()
                        .and_then(|g| g["activity"].as_object())
                        .is_some_and(|nodes| {
                            nodes.values().any(|s| {
                                s["status"] == "permission_required" && s["instance"] == i["id"]
                            })
                        }))
                && !t["editable"]
                    .as_array()
                    .is_some_and(|a| a.contains(&json!(image)))
            {
                continue;
            }
            ensure!(
                i["platform"] == platform,
                "Unload instance to reload admitted platform"
            );
            return Ok(i);
        }
        if i["status"] == "stopped" && i["active"].is_null() {
            i["intent"] = json!("unload");
            save(db, "instance", &i)?;
            reconcile(db, &mut i, f)?;
        }
        ensure!(
            i["status"] == "unloaded",
            "Resolve previous instance before loading"
        );
    }
    let id = owner
        .map(|attempt| format!("driver-{attempt}"))
        .unwrap_or_else(uid);
    if let Some(mut prior) = find(db, "instance", &id)? {
        ensure!(
            prior["task"] == task && prior["spool"] == image && prior["owner"].as_str() == owner,
            "Invalid driver reservation"
        );
        reconcile(db, &mut prior, f)?;
        ensure!(
            prior["status"] == "running",
            "Reserved driver did not start"
        );
        return Ok(prior);
    }
    let mut i = json!({"id":id,"name":format!("an-{id}"),"task":task,"spool":image,"platform":platform,"status":"loading","dedicated":dedicated,"owner":owner,"intent":"load","active":null});
    let config = crate::config::load()?;
    let disk_mib = config.limit("instance_disk_mib");
    i["disk_mib"] = json!(disk_mib);
    capacity::reserve_instance(db, &i, config.limit("max_loaded_instances"))?;
    let setup = (|| -> Result<()> {
        let p = storage::create(&path(&i)?, disk_mib, f)?;
        artifacts::stage(&p.join("work"), &get(db, "revision", work)?, false)?;
        fs::create_dir(p.join("input"))?;
        fs::create_dir(p.join("output"))?;
        apple::start(
            image,
            string(&i, "name")?,
            &format!("an.instance={id}"),
            &p,
            t["editable"]
                .as_array()
                .is_some_and(|v| v.contains(&json!(image))),
            &platform,
            f,
        )?;
        Ok(())
    })();
    if let Err(e) = setup {
        i["intent"] = json!("unload");
        i["error"] = json!(e.to_string());
        save(db, "instance", &i)?;
        reconcile(db, &mut i, f)?;
        bail!("Failed to load {id}: {e}")
    }
    reconcile(db, &mut i, f)?;
    ensure!(i["status"] == "running", "Instance failed to start: {i}");
    Ok(i)
}
