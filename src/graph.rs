pub(crate) mod candidate;
mod host;
pub mod model;
mod scheduler;
mod sources;
mod validation;

pub use host::host;
use host::package;
use scheduler::advance;
pub(crate) use scheduler::paused;
pub use scheduler::wake;
pub(crate) use validation::response_valid;
use validation::validate;

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
        permission::current(&db, task)?;
        graph["model_calls"] = json!(model::history(&db, task)?);
        graph["turn_scopes"] = json!(crate::intent::scope_summary(&db, task)?);
        return Ok(graph);
    }
    if op == "graph_say" {
        let receipt = say(&tx, task, &mut graph, args)?;
        tx.commit()?;
        return Ok(receipt);
    }
    let candidate = op == "graph_candidate";
    let nonce = field(args, "idempotency_key")?;
    let key = if candidate {
        canonical(&json!([task, op, nonce]))
    } else {
        canonical(&json!([task, nonce]))
    };
    if let Some(prior) = find(&tx, "graph_submission", &key)? {
        ensure!(
            prior["args"] == *args,
            "Idempotency key reused with different content"
        );
        tx.commit()?;
        advance(&mut db, task, &mut graph, &lock)?;
        return Ok(prior["receipt"].clone());
    }
    let instruction_cursor = args.get("instruction_cursor");
    if let Some(cursor) = instruction_cursor {
        let authority = hash(canonical(&permission::stamp(&tx, task)?).as_bytes());
        if candidate {
            let latest = graph["events"]
                .as_array()
                .context("Missing graph events")?
                .iter()
                .rev()
                .find(|e| {
                    e["actor"] == "user"
                        && e["kind"] == "instruction"
                        && e["data"]["authority"] == authority
                });
            ensure!(
                latest.is_some_and(|e| e["cursor"] == *cursor),
                "Expected the latest user instruction"
            );
        }
        ensure!(
            cursor.as_u64().is_some_and(|n| n > 0)
                && graph["events"]
                    .as_array()
                    .context("Missing graph events")?
                    .iter()
                    .any(|e| e["cursor"] == *cursor
                        && e["actor"] == "user"
                        && e["kind"] == "instruction"
                        && e["data"]["authority"] == authority),
            "Expected an instruction from the current task and authority"
        );
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
    if candidate {
        ensure!(
            instruction_cursor.is_some() && retry.is_empty() && supersede.is_empty(),
            "A candidate adds nodes for a user instruction"
        );
    }
    for id in retry {
        let model_unknown =
            graph["nodes"][&id]["kind"] == "model" && graph["activity"][&id]["status"] == "unknown";
        let state = &mut graph["activity"][&id];
        ensure!(
            state["status"] == "failed" || state["status"] == "cancelled" || model_unknown,
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
    append_nodes(&tx, &record, &mut graph, additions, None)?;
    let card = if candidate {
        candidate::review(
            &tx,
            task,
            &record,
            &graph,
            additions,
            &args["instruction_cursor"],
        )?
    } else {
        Value::Null
    };
    if candidate {
        let stamp = permission::stamp(&tx, task)?;
        for node in additions {
            let state = &mut graph["activity"][field(node, "id")?];
            put(
                &tx,
                "candidate_review",
                field(state, "id")?,
                &json!({"card":card,"stamp":stamp}),
            )?;
        }
    }
    let rev = graph["revision"].as_u64().unwrap();
    let events = graph["events"].as_array_mut().unwrap();
    let cursor = events.len() + 1;
    let mut submission = json!({"cursor":cursor,"time_ms":now_ms(),"actor":"agent","kind":"submitted","revision":rev,"retry":args["retry"],"supersede":args["supersede"],"nodes":additions.iter().map(|n|&n["id"]).collect::<Vec<_>>()});
    if let Some(cursor) = instruction_cursor {
        submission["instruction_cursor"] = cursor.clone();
    }
    events.push(submission);
    let mut receipt = json!({"revision":rev,"cursor":cursor,"status":"accepted"});
    if candidate {
        receipt["review_card"] = card;
    }
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

/// An agent that returned after proposing a scope needs a continuation only
/// while the corresponding instruction still lacks a submitted graph.
pub(crate) fn intent_decided(db: &Connection, task: &str, card: &Value) -> Result<()> {
    let Some(cursor) = card["instruction_cursor"].as_u64().filter(|n| *n > 0) else {
        return Ok(());
    };
    let mut graph = get(db, "graph", task)?;
    let events = graph["events"].as_array().context("Invalid graph events")?;
    if events.iter().any(|e| {
        (e["kind"] == "submitted" && e["actor"] == "agent" && e["instruction_cursor"] == cursor)
            || (e["kind"] == "instruction"
                && e["actor"] == "user"
                && e["cursor"].as_u64().is_some_and(|n| n > cursor))
    }) {
        return Ok(());
    }
    event(
        &mut graph,
        "user",
        "intent_decided",
        task,
        json!({"intent":card["id"],"instruction_cursor":cursor,"decisions":card["decisions"]}),
    );
    put(db, "graph", task, &graph)
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

fn append_nodes(
    db: &Connection,
    record: &Value,
    graph: &mut Value,
    additions: &[Value],
    driver: Option<&str>,
) -> Result<()> {
    let nodes = graph["nodes"].as_object_mut().unwrap();
    let mut reserved = BTreeMap::new();
    for surface in nodes
        .values()
        .chain(additions.iter())
        .filter(|n| n["continuation"].is_string())
    {
        let saver = field(&surface["save"], "node")?;
        let owner = field(surface, "continuation")?;
        ensure!(
            reserved
                .insert(saver.to_owned(), owner.to_owned())
                .is_none(),
            "Duplicate saver reservation"
        );
    }
    for surface in additions.iter().filter(|n| n["continuation"].is_string()) {
        let saver = field(&surface["save"], "node")?;
        ensure!(
            !nodes.contains_key(saver) && !additions.iter().any(|n| n["id"] == saver),
            "Continuation requires an unclaimed saver"
        );
    }
    for node in additions {
        let id = field(node, "id")?;
        if let Some(owner) = reserved.get(id) {
            ensure!(
                Some(owner.as_str()) == driver,
                "Saver reserved for continuation"
            );
        }
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
            "model" => {
                ensure!(
                    field(node, "model")?
                        == record["model_grant"]["alias"]
                            .as_str()
                            .context("No task model grant")?,
                    "Model outside task grant"
                );
                if model::is_image(&record["model_grant"]["model"]) {
                    ensure!(
                        node["inputs"].as_object().is_some_and(|m| {
                            m.contains_key("request")
                                && m.keys().all(|k| k == "request" || k == "image")
                        }) && node["outputs"] == json!(["result.png"]),
                        "Expected request (and optional image) binding and result.png"
                    );
                } else {
                    ensure!(
                        node["inputs"]
                            .as_object()
                            .is_some_and(|m| m.len() == 1 && m.contains_key("request"))
                            && node["outputs"] == json!(["result.json"]),
                        "Expected request binding and result.json"
                    );
                }
                ensure!(
                    node.as_object().unwrap().keys().all(|k| matches!(
                        k.as_str(),
                        "id" | "kind" | "title" | "after" | "model" | "inputs" | "outputs"
                    )),
                    "Unexpected model node field"
                );
            }
            "run" | "expand" => {
                if node["kind"] == "expand" {
                    ensure!(
                        !record["editable"]
                            .as_array()
                            .is_some_and(|images| images.contains(&node["spool"])),
                        "Driver requires a read-only spool"
                    );
                    ensure!(
                        node["outputs"] == json!(["subgraph.json"]),
                        "Expected subgraph.json"
                    );
                }
                ensure!(
                    record["work"].get(field(node, "spool")?).is_some(),
                    "Spool outside task"
                );
                let spool = field(node, "spool")?;
                let digest = spool.rsplit_once('@').context("Invalid image reference")?.1;
                let manifest = get(db, "admission", digest)?["manifest"].clone();
                if let Some(panels) = manifest.get("panels") {
                    crate::panels::validate(panels)?;
                }
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
                    if node.get("save").is_some() {
                        node["outputs"] == json!(["response", "document"])
                    } else {
                        node["outputs"] == json!(["response"])
                    },
                    "Invalid surface outputs"
                );
            }
            "delivery" => {
                ensure!(
                    node["inputs"].as_object().is_some_and(|m| !m.is_empty())
                        && node["outputs"] == json!([]),
                    "Expected delivery bindings"
                );
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
        let mut stored = node.clone();
        if matches!(node["kind"].as_str(), Some("surface" | "delivery")) {
            ensure!(
                node.as_object().unwrap().keys().all(|k| matches!(
                    k.as_str(),
                    "id" | "kind"
                        | "title"
                        | "after"
                        | "inputs"
                        | "outputs"
                        | "save"
                        | "continuation"
                        | "panel"
                        | "spool"
                )),
                "New panels require an admitted declaration"
            );
            if node["kind"] != "delivery"
                || node.get("panel").is_some()
                || node.get("spool").is_some()
            {
                let spool = field(node, "spool")?;
                ensure!(
                    record["work"].get(spool).is_some(),
                    "Panel spool outside task"
                );
                let admission = get(
                    db,
                    "admission",
                    spool.rsplit_once('@').context("Invalid spool")?.1,
                )?;
                let manifest = &admission["manifest"];
                ensure!(
                    manifest["schema"] == "an.spool/v2",
                    "Expected v2 panel declaration"
                );
                crate::panels::validate(&manifest["panels"])?;
                let panel = manifest["panels"]
                    .get(field(node, "panel")?)
                    .context("Unknown panel")?;
                ensure!(panel["kind"] == node["kind"], "Panel kind mismatch");
                crate::panels::check_bindings(panel, &node["inputs"])?;
                if stored.get("title").is_none() {
                    stored["title"] = panel["title"].clone();
                }
                if node["kind"] == "surface" {
                    stored["response"] = crate::panels::schema(panel)?;
                }
                stored["package"] = json!({"declaration":panel,"code_revision":admission["code_revision"],"manifest_digest":admission["manifest_digest"]});
            }
        }
        nodes.insert(id.to_owned(), stored);
    }
    validate(nodes)?;
    for node in additions {
        graph["activity"][field(node, "id")?] = json!({"status":"queued","attempt":1,"id":uid()});
    }
    graph["revision"] = json!(graph["revision"].as_u64().unwrap() + 1);
    Ok(())
}

/// Record a message from the agent to the person.
///
/// Messages are text only and never carry decisions; those stay typed
/// surfaces. Each message names the graph revision it refers to and, when it
/// answers an instruction, that instruction's event cursor. A repeated
/// idempotency key returns the original receipt instead of a second message.
fn say(tx: &Connection, task: &str, graph: &mut Value, args: &Value) -> Result<Value> {
    ensure!(get(tx, "task", task)?["closed"] != true, "Task closed");
    let text = field(args, "text")?;
    ensure!(!text.trim().is_empty(), "Empty message");
    let max = crate::config::load()?.limit("agent_message_max_bytes");
    ensure!(text.len() as u64 <= max, "Message exceeds {max} bytes");
    let purpose = args["purpose"].as_str().unwrap_or("progress");
    ensure!(
        matches!(purpose, "progress" | "question" | "result"),
        "Unknown message purpose {purpose}"
    );
    let key = canonical(&json!([task, "say", field(args, "idempotency_key")?]));
    if let Some(prior) = find(tx, "graph_message", &key)? {
        ensure!(
            prior["args"] == *args,
            "Idempotency key reused with different content"
        );
        return Ok(prior["receipt"].clone());
    }
    let events = graph["events"].as_array().context("Invalid graph events")?;
    let reply_to = match args.get("reply_to") {
        None | Some(Value::Null) => Value::Null,
        Some(cursor) => {
            let cursor = cursor.as_u64().context("Expected an instruction cursor")?;
            ensure!(
                events
                    .iter()
                    .any(|e| e["cursor"] == cursor && e["kind"] == "instruction"),
                "reply_to must name an instruction"
            );
            json!(cursor)
        }
    };
    let revision = graph["revision"].clone();
    let authority = hash(canonical(&permission::stamp(tx, task)?).as_bytes());
    event(
        graph,
        "agent",
        "message",
        task,
        json!({"text":text,"purpose":purpose,"reply_to":reply_to,"revision":revision,"authority":authority}),
    );
    put(tx, "graph", task, graph)?;
    let receipt =
        json!({"cursor":graph["events"].as_array().map_or(0, Vec::len),"revision":revision});
    put(
        tx,
        "graph_message",
        &key,
        &json!({"args":args,"receipt":receipt}),
    )?;
    Ok(receipt)
}
