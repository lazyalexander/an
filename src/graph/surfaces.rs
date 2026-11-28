use super::{event, field, response_valid, validation, wake};
use crate::store::{canonical, find, get, hash, put, revision};
use crate::{artifacts, permission};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs::File};

pub(super) fn package(
    db: &Connection,
    bindings: &Value,
    entry: &Value,
    schema: &Value,
) -> Result<Value> {
    let mut data = json!({});
    for (name, binding) in bindings.as_object().context("Expected package bindings")? {
        data[name] = get(db, "revision", field(binding, "revision")?)?;
    }
    if let Some(panel) = entry.get("declaration") {
        let mut result = json!({"files":data,"declaration":panel,"response":schema,"assets":{}});
        if let Some(path) = panel["html"].as_str() {
            let files = get(db, "revision", field(entry, "code_revision")?)?;
            result["html"] = json!(String::from_utf8(artifacts::bytes(&files[path])?)?);
        }
        return Ok(result);
    }
    if entry.is_null() {
        return Ok(json!({"files":data}));
    }
    if !schema.is_null() {
        validation::validate_schema(schema)?;
    }
    let name = entry.as_str().context("Expected package binding")?;
    let files = data
        .as_object_mut()
        .unwrap()
        .remove(name)
        .context("Missing package")?;
    let files = files.as_object().context("Invalid package files")?;
    ensure!(files.len() == 1, "Package must contain one JSON manifest");
    let manifest: Value =
        serde_json::from_slice(&artifacts::bytes(files.values().next().unwrap())?)?;
    ensure!(manifest["version"] == 1, "Unsupported package");
    let html = manifest["files"][field(&manifest, "entry")?]
        .as_str()
        .context("Missing HTML entry")?;
    let declared: BTreeSet<String> = serde_json::from_value(manifest["bindings"].clone())?;
    ensure!(
        declared == data.as_object().unwrap().keys().cloned().collect(),
        "Package data bindings changed"
    );
    ensure!(&manifest["response"] == schema, "Response schema changed");
    Ok(json!({"html":html,"assets":manifest["files"],"files":data}))
}

pub(super) fn handle(
    tx: rusqlite::Transaction<'_>,
    lock: File,
    task: &str,
    mut graph: Value,
    action: &str,
    args: &Value,
) -> Result<Value> {
    let id = field(args, "node")?;
    let node = graph["nodes"].get(id).context("Unknown node")?.clone();
    if node.get("confirm_delivery").is_some() {
        permission::controller(&tx, task)?;
    }
    let mut state = graph["activity"][id].clone();
    ensure!(
        state["id"] == args["request"]
            && state["authority"]
                == json!(hash(canonical(&permission::stamp(&tx, task)?).as_bytes()))
            && node["kind"] == "surface",
        "Stale surface"
    );
    let request = field(&state, "id")?.to_owned();
    if let Some(panel) = node["package"].get("declaration") {
        crate::panels::schema(panel).context("This build cannot handle the stored decision")?;
    }
    if matches!(action, "panel" | "decision") {
        ensure!(state["status"] == "waiting", "Surface closed");
        let draft = find(&tx, "graph_draft", &request)?.unwrap_or(Value::Null);
        if action == "decision" {
            ensure!(
                node["package"].get("declaration").is_some(),
                "Expected a declared decision"
            );
            return Ok(json!({"draft":draft}));
        }
        let mut result = package(&tx, &state["bindings"], &node["package"], &node["response"])?;
        if let Some(actions) = node.get("actions") {
            result["actions"] = actions.clone();
        }
        result["draft"] = draft;
        return Ok(result);
    }
    if action == "viewed" {
        ensure!(state["status"] == "waiting", "Surface closed");
        if state["seen"] != true {
            state["seen"] = json!(true);
            graph["activity"][id] = state.clone();
            event(
                &mut graph,
                "user",
                "viewed",
                id,
                json!({"request":request,"bindings":state["bindings"]}),
            );
            put(&tx, "graph", task, &graph)?;
        }
        tx.commit()?;
        return Ok(json!({"status":"waiting"}));
    }
    if action == "draft" {
        ensure!(state["status"] == "waiting", "Surface closed");
        put(&tx, "graph_draft", &request, &args["value"])?;
    } else if action == "respond" {
        let response = args.get("value").context("Expected response")?;
        if state.get("response") == Some(response) {
            return Ok(json!({"status":state["status"]}));
        }
        ensure!(
            state["status"] == "waiting" && state["seen"] == true,
            "Surface closed or unseen"
        );
        response_valid(&node["response"], response)?;
        if node["package"]
            .get("declaration")
            .is_some_and(|p| p["decision"]["type"] == "region")
        {
            for (position, size) in [("x", "width"), ("y", "height")] {
                let start = response[position]
                    .as_f64()
                    .context("Invalid region origin")?;
                let length = response[size].as_f64().context("Invalid region size")?;
                ensure!(
                    length > 0.0 && start + length <= 1.0,
                    "Region extends outside artifact or has zero area"
                );
            }
        }
        if node.get("confirm_delivery").is_some() {
            state["confirmations"] =
                super::confirmation::record(&tx, task, &graph, &node, &state, response)?;
        }
        state["response"] = response.clone();
        let rev = revision(
            &tx,
            &json!({"response.json":STANDARD.encode(serde_json::to_vec(response)?)}),
        )?;
        state["outputs"]["response"] = json!({"revision":rev});
        state["status"] = json!(if node.get("save").is_some() {
            "saving"
        } else {
            "done"
        });
        graph["activity"][id] = state.clone();
        let mut detail = json!({"request":request,"revision":rev,"bindings":state["bindings"],"response":response});
        if let Some(receipts) = state.get("confirmations") {
            detail["confirmations"] = receipts.clone();
        }
        event(&mut graph, "user", "responded", id, detail);
        put(&tx, "graph", task, &graph)?;
    } else {
        anyhow::bail!("Unknown graph host action")
    }
    tx.commit()?;
    if action == "respond" {
        drop(lock);
        if let Err(error) = wake(task) {
            eprintln!("Response recorded. Scheduler will reconcile on next state read: {error}");
        }
    }
    Ok(json!({"status":state["status"]}))
}
