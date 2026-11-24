mod host;
mod scheduler;
mod sources;
mod validation;

pub use host::host;
use host::package;
use scheduler::advance;
pub use scheduler::wake;
use validation::{response_valid, validate, validate_schema};

use crate::{artifacts, instances, permission, store::*};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::Connection;
use rusqlite::TransactionBehavior;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("Expected {key}"))
}

pub fn dispatch(task: &str, op: &str, args: &Value) -> Result<Value> {
    let mut db = connect()?;
    let lock = instances::lock(&db, task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    permission::current(&tx, task)?;
    let mut graph = find(&tx, "graph", task)?
        .unwrap_or(json!({"revision":0,"nodes":{},"activity":{},"events":[]}));
    if op == "graph_state" {
        tx.commit()?;
        advance(&mut db, task, &mut graph, &lock)?;
        let after = args["after"].as_u64().unwrap_or(0);
        graph["cursor"] = json!(graph["events"].as_array().unwrap().len());
        graph["events"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e["cursor"].as_u64().unwrap() > after);
        return Ok(graph);
    }
    let key = canonical(&json!([task, field(args, "idempotency_key")?]));
    if let Some(prior) = find(&tx, "graph_submission", &key)? {
        ensure!(
            prior["args"] == *args,
            "Idempotency key reused with different content"
        );
        tx.commit()?;
        advance(&mut db, task, &mut graph, &lock)?;
        return Ok(prior["receipt"].clone());
    }
    if args["base_revision"] != graph["revision"] {
        return Ok(json!({"status":"conflict","revision":graph["revision"]}));
    }
    let record = get(&tx, "task", task)?;
    let additions = args["nodes"].as_array().context("Expected nodes")?;
    let retry: Vec<String> =
        serde_json::from_value(args.get("retry").cloned().unwrap_or(json!([])))?;
    let supersede: Vec<String> =
        serde_json::from_value(args.get("supersede").cloned().unwrap_or(json!([])))?;
    ensure!(
        !additions.is_empty() || !retry.is_empty() || !supersede.is_empty(),
        "Empty submission"
    );
    for id in retry {
        let state = &mut graph["activity"][&id];
        ensure!(
            state["status"] == "failed" || state["status"] == "cancelled",
            "Expected settled failed attempt"
        );
        *state =
            json!({"id":uid(),"attempt":state["attempt"].as_u64().unwrap()+1,"status":"queued"});
    }
    for id in supersede {
        let state = &mut graph["activity"][&id];
        ensure!(
            state["status"] == "waiting" || state["status"] == "queued",
            "Expected unconsumed node"
        );
        state["status"] = json!("superseded");
    }
    let nodes = graph["nodes"].as_object_mut().unwrap();
    for node in additions {
        let id = field(node, "id")?;
        ensure!(
            node.get("inputs").is_none_or(Value::is_object),
            "Expected input map"
        );
        for (name, r) in node
            .get("inputs")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            artifacts::safe(name)?;
            if let Some(input) = r.get("input") {
                ensure!(
                    record["inputs"]
                        .get(input.as_str().context("Expected input name")?)
                        .is_some(),
                    "Unknown task input"
                );
            }
        }
        ensure!(!nodes.contains_key(id), "Existing node {id}");
        match field(node, "kind")? {
            "run" => {
                ensure!(
                    record["work"].get(field(node, "spool")?).is_some(),
                    "Spool outside task"
                );
                let spool = field(node, "spool")?;
                let digest = spool.rsplit_once('@').context("Invalid image reference")?.1;
                let manifest = get(&tx, "admission", digest)?["manifest"].clone();
                let ports = manifest["inputs"]
                    .as_array()
                    .context("Invalid manifest inputs")?;
                for port in ports {
                    ensure!(
                        port["required"] != true
                            || node["inputs"].get(field(port, "name")?).is_some(),
                        "Missing required input"
                    );
                }
                for name in node["inputs"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, _)| k)
                {
                    ensure!(
                        ports.iter().any(|port| port["name"] == name.as_str()),
                        "Undeclared input {name}"
                    );
                }
                let argv: Vec<String> = serde_json::from_value(node["argv"].clone())?;
                ensure!(
                    argv.first()
                        .is_some_and(|s| !s.is_empty() && !s.starts_with('-')),
                    "Invalid argv"
                );
            }
            "surface" => {
                ensure!(
                    node["inputs"].get(field(node, "package")?).is_some(),
                    "Missing panel package binding"
                );
                ensure!(
                    if node.get("save").is_some() {
                        node["outputs"] == json!(["response", "document"])
                    } else {
                        node["outputs"] == json!(["response"])
                    },
                    "Invalid surface outputs"
                );
                validate_schema(&node["response"])?;
            }
            "delivery" => {
                ensure!(
                    node["inputs"].as_object().is_some_and(|m| !m.is_empty())
                        && node["outputs"] == json!([]),
                    "Expected delivery bindings"
                );
                if let Some(package) = node.get("package") {
                    ensure!(
                        node["inputs"]
                            .get(package.as_str().context("Invalid package")?)
                            .is_some(),
                        "Missing package"
                    );
                }
            }
            _ => anyhow::bail!("Unknown node kind"),
        }
        let mut names = BTreeSet::new();
        for p in node["outputs"].as_array().context("Expected output list")? {
            ensure!(
                names.insert(artifacts::safe(p.as_str().context("Invalid output name")?)?),
                "Duplicate output"
            );
        }
        nodes.insert(id.to_owned(), node.clone());
    }
    validate(nodes)?;
    let rev = graph["revision"].as_u64().unwrap() + 1;
    graph["revision"] = json!(rev);
    let events = graph["events"].as_array_mut().unwrap();
    let cursor = events.len() + 1;
    events.push(json!({"cursor":cursor,"time_ms":now_ms(),"actor":"agent","kind":"submitted","revision":rev,"retry":args["retry"],"supersede":args["supersede"],"nodes":additions.iter().map(|n|&n["id"]).collect::<Vec<_>>()}));
    for node in additions {
        graph["activity"][field(node, "id")?] = json!({"status":"queued","attempt":1,"id":uid()});
    }
    let receipt = json!({"revision":rev,"cursor":cursor,"status":"accepted"});
    put(&tx, "graph", task, &graph)?;
    put(
        &tx,
        "graph_submission",
        &key,
        &json!({"args":args,"receipt":receipt}),
    )?;
    tx.commit()?;
    advance(&mut db, task, &mut graph, &lock)?;
    Ok(receipt)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn event(graph: &mut Value, actor: &str, kind: &str, id: &str, data: Value) {
    let events = graph["events"].as_array_mut().unwrap();
    events.push(json!({"cursor":events.len()+1,"time_ms":now_ms(),"actor":actor,"kind":kind,"node":id,"data":data}));
}

pub(crate) fn binding_origin(item: &Value) -> String {
    if let Some(spool) = item["spool"].as_str() {
        spool.to_owned()
    } else if let Some(name) = item["source"].as_str() {
        format!("task-input:{name}")
    } else {
        "response".to_owned()
    }
}
