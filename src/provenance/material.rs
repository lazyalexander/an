//! Resolve immutable files through task inputs and actual graph attempts.
use super::{access::Actor, domain::Material};
use crate::{
    graph, permission,
    store::{canonical, find, get, hash, put},
};
use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

fn resolve(db: &Connection, task: &str, material: &Material) -> Result<(Value, String)> {
    match material {
        Material::Input { name } => {
            let task = get(db, "task", task)?;
            let revision = task["inputs"][name]
                .as_str()
                .context("Unknown task input")?;
            Ok((
                json!({"revision":revision,"input":name}),
                format!("task-input:{name}"),
            ))
        }
        Material::Output {
            node,
            attempt,
            port,
        } => {
            let graph = get(db, "graph", task)?;
            // Previous attempts can also be cited, using their recorded output.
            let current = &graph["activity"][node];
            let state = std::iter::once(current)
                .chain(
                    graph["events"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .rev()
                        .filter(|e| e["node"] == *node)
                        .map(|e| &e["data"]),
                )
                .find(|state| {
                    state["id"] == *attempt
                        && state["status"] == "done"
                        && state["outputs"][port]["revision"].is_string()
                })
                .context("Output outside completed attempt")?;
            let binding = &state["outputs"][port];
            Ok((
                json!({"revision":binding["revision"],"node":node,"attempt":attempt,"port":port,
                "bindings":state["bindings"],"execution":state["evidence"]}),
                graph::binding_origin(binding),
            ))
        }
    }
}

fn key(task: &str, stamp: &Value, material: &Material) -> Result<String> {
    Ok(hash(canonical(&json!([task, stamp, material])).as_bytes()))
}

pub fn admit(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let material: Material = serde_json::from_value(args["material"].clone())?;
    let stamp = permission::stamp(db, &actor.task)?;
    let id = key(&actor.task, &stamp, &material)?;
    if find(db, "provenance_material", &id)?.is_some() {
        return Ok(json!({"admitted":id}));
    }
    let (record, origin) = resolve(db, &actor.task, &material)?;
    if let Some(pending) = permission::gate(
        db,
        &actor.task,
        "preview",
        json!([origin, "provenance", id]),
        json!([record["revision"]]),
        args["request_id"].as_str(),
        Some(json!(["preview", origin, "provenance"])),
    )? {
        return Ok(pending);
    }
    put(db, "provenance_material", &id, &record)?;
    Ok(json!({"admitted":id}))
}

pub fn admitted(db: &Connection, actor: &Actor, material: &Material) -> Result<Value> {
    actor.controller()?;
    let id = key(&actor.task, &permission::stamp(db, &actor.task)?, material)?;
    find(db, "provenance_material", &id)?.context("Material requires admission in this turn")
}
