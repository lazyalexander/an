use super::inputs::{bridge_artifact, input_bindings};
use super::worker::reap;
use super::{save, string};
use crate::engine::capacity;
use crate::store::uid;
use crate::store::{get, put, revision, root};
use crate::{artifacts, permission};
use anyhow::{ensure, Context, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::fs::{self, File};

pub(super) fn submit(
    db: &mut Connection,
    task: &str,
    t: &Value,
    i: &mut Value,
    args: &Value,
    manifest: &Value,
    launch: impl FnOnce(&str, &File) -> std::io::Result<std::process::Child>,
) -> Result<Value> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let spool = string(i, "spool")?;
    if args["_graph_run"].is_string() {
        permission::current(&tx, task)?;
    }
    ensure!(
        i["status"] == "running" && i["intent"] == "load" && i["active"].is_null(),
        "Instance is not ready"
    );
    let editable = t["editable"]
        .as_array()
        .is_some_and(|v| v.contains(&i["spool"]));
    if editable {
        let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE kind='run' AND json_extract(body,'$.task')=? AND json_extract(body,'$.spool')=? AND json_extract(body,'$.instance') IS NULL AND json_extract(body,'$.status') IN ('running','unknown','cancel_pending'))",[task,spool],|r|r.get(0))?;
        ensure!(!busy, "Unresolved legacy run owns editable spool");
    }
    let argv: Vec<String> = serde_json::from_value(args["argv"].clone())?;
    ensure!(
        argv.first()
            .is_some_and(|s| !s.is_empty() && !s.starts_with('-')),
        "Expected a guest executable"
    );
    let outputs: Vec<String> =
        serde_json::from_value(args.get("outputs").cloned().unwrap_or(json!([])))?;
    for p in &outputs {
        artifacts::safe(p)?;
    }
    let named = args.get("inputs").filter(|v| !v.is_null());
    let bindings = input_bindings(args, manifest)?;
    let mut tickets = Vec::new();
    let mut decisions = Vec::new();
    let mut files = json!({});
    let mut provenance_inputs = json!({});
    let mut input_stamp = Value::Null;
    if !bindings.is_empty() || args["_graph_run"].is_string() {
        input_stamp = permission::stamp(&tx, task)?;
    }
    // Roll back approval consumption if a later port still needs a decision.
    tx.execute_batch("SAVEPOINT input_decisions")?;
    for (port, source) in bindings {
        let (rev, flow) = if let Some(binding) = args["_graph_bindings"].get(source) {
            let origin = &binding["spool"];
            let flow = if origin.is_null() {
                json!(["input", "response", spool])
            } else {
                json!(["handoff", origin, spool])
            };
            (binding["revision"].clone(), Some(flow))
        } else if let Some(rev) = t["inputs"].get(source) {
            (rev.clone(), Some(json!(["input", source, spool])))
        } else if source.starts_with("handoff-") {
            let ticket = get(&tx, "handoff", source)?;
            ensure!(
                ticket["task"] == task
                    && ticket["recipient"] == spool
                    && ticket["used"].is_null()
                    && ticket["stamp"] == input_stamp
                    && !tickets.iter().any(|(key, _)| *key == source),
                "Invalid or revoked handoff"
            );
            let rev = ticket["revision"].clone();
            tickets.push((source, ticket));
            (rev, None)
        } else if source.starts_with("artifact-") {
            let item = bridge_artifact(&tx, task, t, source)?;
            let origin = string(&item, "tool")?;
            (
                item["revision"].clone(),
                Some(json!(["bridge_input", origin, spool])),
            )
        } else {
            let up = get(&tx, "run", source)?;
            if up["task"] != task {
                ensure!(
                    t["workspace"].is_string()
                        && up["outputs"].as_array().is_some_and(|a| !a.is_empty())
                        && get(&tx, "task", string(&up, "task")?)?["workspace"] == t["workspace"],
                    "Delivery outside workspace"
                );
            }
            ensure!(up["status"] == "delivered", "Input has not been delivered");
            (
                up["output"].clone(),
                Some(json!(["handoff", up["spool"], spool])),
            )
        };
        if let Some(flow) = flow {
            let operation = if args["_graph_bindings"]
                .get(source)
                .is_some_and(|b| !b["spool"].is_null())
            {
                "handoff"
            } else {
                "input"
            };
            let scope = match port {
                Some(name) => json!([source, rev, spool, name]),
                None => json!([source, rev, spool]),
            };
            let details = match port {
                Some(name) => json!([i["id"], argv, outputs, name, named]),
                None => json!([i["id"], argv, outputs]),
            };
            let request = match port {
                Some(name) => args["input_requests"][name].as_str(),
                None => args["request_id"].as_str(),
            };
            if let Some(id) = request {
                decisions.push((id, get(&tx, "request", id)?));
            }
            let pending = if let Some(reference) =
                port.and_then(|name| args["_graph_sources"].get(name))
            {
                let input = json!({"source":source,"revision":rev});
                let binding = args["_graph_bindings"].get(source).unwrap_or(&input);
                let route = crate::intent::graph_route(&tx, operation, reference, spool, binding)?;
                permission::graph_gate(&tx, task, operation, scope, details, request, flow, route)?
            } else {
                permission::gate(&tx, task, operation, scope, details, request, Some(flow))?
            };
            if let Some(mut pending) = pending {
                let id = string(&pending, "request_id")?.to_owned();
                let row = get(&tx, "request", &id)?;
                tx.execute_batch("ROLLBACK TO input_decisions")?;
                put(&tx, "request", &id, &row)?;
                if let Some(port) = port {
                    pending["port"] = json!(port);
                }
                tx.commit()?;
                return Ok(pending);
            }
        }
        let producer = args["_graph_bindings"]
            .get(source)
            .and_then(|binding| binding.get("run").or(binding.get("attempt")));
        provenance_inputs[port.unwrap_or("input")] =
            json!({"revision":rev,"run":producer,"source":t["inputs"].get(source).map(|_|source)});
        let input = get(&tx, "revision", rev.as_str().context("Invalid revision")?)?;
        for (name, entry) in input.as_object().context("Invalid files")? {
            artifacts::safe(name)?;
            let key = if let Some(port) = port {
                format!("{port}/{name}")
            } else {
                name.clone()
            };
            files[key] = entry.clone();
        }
    }
    let inp = revision(&tx, &files)?;
    let id = args["_graph_run"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(uid);
    let dir = root()?.join("runs").join(&id);
    fs::create_dir_all(&dir)?;
    // An interrupted reservation can leave a directory before its transaction commits.
    let lease = File::options()
        .create(true)
        .append(true)
        .open(dir.join("lease"))?;
    lease.lock()?;
    let mut r = json!({"inputs":named,"id":id,"instance":i["id"],"task":task,"spool":spool,"work":t["work"][spool],"argv":argv,"input":inp,"source":args["input"],"outputs":outputs,"editable":editable,"status":"running","input_stamp":input_stamp});
    for (key, record) in &tickets {
        let mut used = record.clone();
        used["used"] = json!(id);
        put(&tx, "handoff", key, &used)?;
    }
    if args["_graph_run"].is_string() {
        r["graph_bindings"] = args["_graph_bindings"].clone();
        let digest = spool.rsplit_once('@').context("Invalid admitted image")?.1;
        let image = get(&tx, "admission", digest)?;
        r["image_code"] = image["code_revision"].clone();
        r["code_omissions"] = image["code_omissions"].clone();
    }
    crate::provenance::flow::record(
        &tx,
        task,
        crate::provenance::flow::Phase::Authorized,
        &id,
        spool,
        &provenance_inputs,
    )?;
    r["bound_inputs"] = provenance_inputs;
    capacity::enqueue(&tx, &id)?;
    r["capacity_managed"] = json!(true);
    i["active"] = json!(id);
    save(&tx, "run", &r)?;
    save(&tx, "instance", i)?;
    tx.commit()?;
    match launch(&id, &lease) {
        Ok(child) => {
            // The caller holds the task lock until the spawned worker has an owner.
            r["worker_pid"] = json!(child.id());
            reap(child);
            save(db, "run", &r)?;
        }
        Err(e) => {
            let tx = db.transaction()?;
            capacity::release(&tx, &id)?;
            for (key, record) in tickets {
                put(&tx, "handoff", key, &record)?;
            }
            for (key, record) in decisions {
                put(&tx, "request", key, &record)?;
            }
            r["status"] = json!("failed");
            r["error"] = json!(format!("Worker launch failed: {e}"));
            i["active"] = Value::Null;
            save(&tx, "run", &r)?;
            save(&tx, "instance", i)?;
            tx.commit()?;
        }
    }
    Ok(json!(id))
}
