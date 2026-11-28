use super::{event, field, AttemptStatus};
use crate::store::{canonical, find, get, hash, put};
use crate::{instances, permission};
use anyhow::{ensure, Context, Result};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Control {
    Pause,
    Resume,
    Stop,
}

impl Control {
    fn recorded_transition(self) -> (&'static str, &'static str) {
        match self {
            Self::Pause => ("paused", "paused"),
            Self::Resume => ("running", "resumed"),
            Self::Stop => ("paused", "stopped"),
        }
    }
}

#[derive(Default)]
pub(super) struct StopPlan {
    cancelled_nodes: Vec<String>,
    pub runs: Vec<String>,
    pub drivers: Vec<String>,
}

pub(super) struct AppliedControl {
    pub receipt: Value,
    pub stop: StopPlan,
}

pub(super) fn apply(
    tx: &Connection,
    task: &str,
    record: &Value,
    graph: &mut Value,
    args: &Value,
) -> Result<AppliedControl> {
    let name = field(args, "action")?;
    let control =
        Control::deserialize(&args["action"]).with_context(|| format!("Unknown control {name}"))?;
    permission::current(tx, task)?;
    ensure!(record["closed"] != true, "Task closed");
    let stop = match control {
        Control::Stop => request_stop(tx, task, graph)?,
        Control::Pause | Control::Resume => StopPlan::default(),
    };
    let (mode, past) = control.recorded_transition();
    graph["control"] = json!(mode);
    event(
        graph,
        "user",
        past,
        task,
        json!({"cancelled":stop.cancelled_nodes, "cancelling":stop.runs}),
    );
    put(tx, "graph", task, graph)?;
    let receipt = json!({"control":mode,"cancelled":stop.cancelled_nodes,"cancelling":stop.runs});
    Ok(AppliedControl { receipt, stop })
}

fn request_stop(tx: &Connection, task: &str, graph: &mut Value) -> Result<StopPlan> {
    let mut plan = StopPlan::default();
    // Record cancellation under the task lock before any fallible runtime
    // calls. A worker waiting to start sees the stop intent after restart.
    let mut query = tx.prepare("SELECT id, body FROM objects WHERE kind='run' AND json_extract(body,'$.task')=? AND json_extract(body,'$.status') IN ('running','unknown','cancel_pending') ORDER BY id")?;
    let active = query
        .query_map([task], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, body) in active {
        let mut run: Value = serde_json::from_str(&body)?;
        if let Some(instance) = run["instance"].as_str() {
            let mut i = get(tx, "instance", instance)?;
            if !instances::can_cancel_run(&i, &id) {
                continue;
            }
            if i["intent"] == "load" {
                i["intent"] = json!("stop");
                put(tx, "instance", instance, &i)?;
            }
        }
        if run["status"] != "unknown" || run["instance"].is_string() {
            run["status"] = json!("cancel_pending");
            put(tx, "run", &id, &run)?;
        }
        plan.runs.push(id);
    }
    let nodes = graph["nodes"].clone();
    for (id, state) in graph["activity"]
        .as_object_mut()
        .context("Invalid graph activity")?
    {
        let attempt = state["id"].as_str().unwrap_or_default().to_owned();
        if nodes[id]["kind"] == "model" {
            super::model::release_unsent(tx, &attempt, "Task stopped before send")?;
        }
        if let Some(instance) = state["instance"].as_str() {
            if instance == format!("driver-{attempt}") {
                if let Some(mut i) = find(tx, "instance", instance)? {
                    if i["status"] != "unloaded" {
                        i["intent"] = json!("unload");
                        put(tx, "instance", instance, &i)?;
                        plan.drivers.push(instance.to_owned());
                    }
                }
            }
        }
        if AttemptStatus::of(state).can_stop() {
            if plan.runs.contains(&attempt) {
                state["status"] = json!(AttemptStatus::CancelPending);
                continue;
            }
            if state["instance"]
                .as_str()
                .is_some_and(|i| plan.drivers.iter().any(|d| d == i))
            {
                state["status"] = json!(AttemptStatus::CleanupPending);
                state["after_cleanup"] = json!(AttemptStatus::Cancelled);
                continue;
            }
            state
                .as_object_mut()
                .context("Invalid activity")?
                .remove("waiting_reason");
            state["status"] = json!(AttemptStatus::Cancelled);
            plan.cancelled_nodes.push(id.clone());
        }
    }
    Ok(plan)
}

/// Relay a person's instruction to the agent, unchanged.
///
/// An never interprets the text. It records who sent it (the person, under
/// the current turn), when, and which graph revision the person was looking
/// at, so the agent can tell when an instruction refers to an older state.
/// The agent reads it as a `user`/`instruction` event from graph state. An
/// instruction never answers a waiting surface.
pub(super) fn instruct(
    tx: &Connection,
    task: &str,
    t: &Value,
    graph: &mut Value,
    args: &Value,
) -> Result<Value> {
    permission::current(tx, task)?;
    ensure!(t["closed"] != true, "Task closed");
    let text = args["text"].as_str().context("Expected instruction text")?;
    ensure!(!text.trim().is_empty(), "Empty instruction");
    let max = crate::config::load()?.limit("instruction_max_bytes");
    ensure!(text.len() as u64 <= max, "Instruction exceeds {max} bytes");
    let seen = args["revision"]
        .as_u64()
        .context("Expected the graph revision the person saw")?;
    let current = graph["revision"].as_u64().unwrap_or(0);
    ensure!(seen <= current, "Unknown graph revision");
    let stale = seen != current;
    event(
        graph,
        "user",
        "instruction",
        task,
        json!({"text":text,"seen_revision":seen,"revision":current,"stale":stale,"authority":hash(canonical(&permission::stamp(tx,task)?).as_bytes())}),
    );
    put(tx, "graph", task, graph)?;
    let cursor = graph["events"].as_array().map_or(0, Vec::len);
    Ok(
        json!({"cursor":cursor,"revision":current,"stale":stale,"authority":hash(canonical(&permission::stamp(tx,task)?).as_bytes())}),
    )
}

pub(super) fn finish(tx: &Connection, task: &str, mut t: Value, graph: &mut Value) -> Result<()> {
    if t["closed"] == true {
        return instances::request_task_unload(tx, task);
    }
    permission::current(tx, task)?;
    for state in graph["activity"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, state)| state)
    {
        ensure!(
            AttemptStatus::of(state).allows_task_close(),
            "Unresolved graph node"
        );
    }
    let unresolved:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE kind='run' AND json_extract(body,'$.task')=? AND json_extract(body,'$.status') IN ('running','unknown','cancel_pending'))",[task],|r|r.get(0))?;
    ensure!(!unresolved, "Unresolved run");
    instances::request_task_unload(tx, task)?;
    t["closed"] = json!(true);
    put(tx, "task", task, &t)?;
    put(tx, "turn", task, &Value::Null)?;
    event(graph, "host", "closed", task, json!({}));
    put(tx, "graph", task, graph)?;
    Ok(())
}
