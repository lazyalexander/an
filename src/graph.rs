mod admission;
use admission::append_nodes;
mod attempt;
use attempt::AttemptStatus;
mod confirmation;
mod control;
mod deliveries;
mod host;
pub mod model;
mod reference;
pub(crate) use reference::ArtifactRef;
mod scheduler;
mod sources;
mod surfaces;
mod view;
pub(crate) use sources::lineage;
mod validation;

pub use host::host;
use scheduler::advance;
pub(crate) use scheduler::paused;
pub use scheduler::wake;
use surfaces::package;
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

pub(crate) fn empty() -> Value {
    json!({"revision":0,"nodes":{},"activity":{},"events":[]})
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
    let instruction_cursor = args.get("instruction_cursor");
    if let Some(cursor) = instruction_cursor {
        let authority = hash(canonical(&permission::stamp(&tx, task)?).as_bytes());
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
    for id in retry {
        let is_model = graph["nodes"][&id]["kind"] == "model";
        let state = &mut graph["activity"][&id];
        ensure!(
            AttemptStatus::of(state).can_retry(is_model),
            "Expected settled failed attempt"
        );
        *state = attempt::next(state)?;
    }
    for id in supersede {
        let state = &mut graph["activity"][&id];
        ensure!(
            AttemptStatus::of(state).can_supersede(),
            "Expected unconsumed node"
        );
        state
            .as_object_mut()
            .context("Invalid activity")?
            .remove("waiting_reason");
        state["status"] = json!(AttemptStatus::Superseded);
    }
    append_nodes(&tx, task, &record, &mut graph, additions, None)?;
    let rev = graph["revision"].as_u64().unwrap();
    let events = graph["events"].as_array_mut().unwrap();
    let cursor = events.len() + 1;
    let mut submission = json!({"cursor":cursor,"time_ms":now_ms(),"actor":"agent","kind":"submitted","revision":rev,"retry":args["retry"],"supersede":args["supersede"],"nodes":additions.iter().map(|n|&n["id"]).collect::<Vec<_>>()});
    if let Some(cursor) = instruction_cursor {
        submission["instruction_cursor"] = cursor.clone();
    }
    if let Some(cause) = args.get("decision_receipt") {
        crate::provenance::require_accepted(
            &tx,
            task,
            cause.as_str().context("Expected decision receipt")?,
        )?;
        submission["decision_receipt"] = cause.clone();
    }
    events.push(submission);
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

pub(crate) fn event(graph: &mut Value, actor: &str, kind: &str, id: &str, data: Value) {
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
