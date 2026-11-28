mod expansion;
mod presentation;
mod run;
use super::*;
use expansion::accept_expansion;

fn bindings(db: &Connection, task: &str, graph: &Value, node: &Value) -> Result<Option<Value>> {
    let mut result = json!({});
    for dep in node["after"].as_array().into_iter().flatten() {
        if graph["activity"][dep.as_str().unwrap()]["status"] != "done" {
            return Ok(None);
        }
    }
    for (name, r) in node["inputs"].as_object().into_iter().flatten() {
        result[name] = match ArtifactRef::try_from(r)? {
            ArtifactRef::Input { input } => {
                json!({"source":input,"revision":get(db,"task",task)?["inputs"][input]})
            }
            ArtifactRef::Output {
                node: upstream,
                port,
            } => {
                let producer = &graph["nodes"][upstream];
                let state = &graph["activity"][upstream];
                let provisional = port == "response"
                    && (producer["save"]["node"] == node["id"]
                        || producer["continuation"] == node["id"]);
                match AttemptStatus::of(state) {
                    AttemptStatus::Done => {}
                    AttemptStatus::Saving if provisional => {}
                    _ => return Ok(None),
                }
                state["outputs"]
                    .get(port)
                    .context("Missing upstream version")?
                    .clone()
            }
        };
    }
    Ok(Some(result))
}

pub(crate) fn paused(graph: &Value) -> bool {
    graph["control"] == "paused"
}

/// Wake the scheduler after a durable change, including worker settlement.
/// A later state read can repeat this reconciliation after a host crash.
pub fn wake(task: &str) -> Result<()> {
    let mut db = connect()?;
    if find(&db, "graph", task)?.is_none() {
        return Ok(());
    }
    let lock = instances::lock(&db, task)?;
    if get(&db, "task", task)?["closed"] == true {
        // A cancelled model send can settle after finish has released its turn.
        super::model::reconcile_sends(&db, task)?;
        return crate::store::gc::after_finish(&mut db, task);
    }
    if permission::current(&db, task).is_err() {
        return super::model::reconcile_sends(&db, task);
    }
    let mut graph = get(&db, "graph", task)?;
    advance(&mut db, task, &mut graph, &lock)
}

pub(super) fn advance(
    db: &mut Connection,
    task: &str,
    graph: &mut Value,
    lock: &File,
) -> Result<()> {
    super::model::reconcile_sends(db, task)?;
    loop {
        let nodes = graph["nodes"].as_object().context("Invalid graph")?.clone();
        let mut finished = false;
        for (id, node) in &nodes {
            let mut state = graph["activity"][id].clone();
            if state["status"] == "saving" {
                let saver = field(&node["save"], "node")?;
                let run = &graph["activity"][saver];
                let continuation_superseded = node["continuation"]
                    .as_str()
                    .is_some_and(|id| graph["activity"][id]["status"] == "superseded");
                if run["status"] == "superseded" || continuation_superseded {
                    state["status"] = json!(AttemptStatus::Superseded);
                    graph["activity"][id] = state;
                    event(graph, "host", "superseded", id, json!({"save_node":saver}));
                    put(db, "graph", task, graph)?;
                } else if run["status"] == "done" {
                    let saved = run["outputs"]
                        .get(field(&node["save"], "port")?)
                        .context("Missing saved version")?
                        .clone();
                    state["outputs"]["document"] = saved.clone();
                    state["status"] = json!(AttemptStatus::Done);
                    graph["activity"][id] = state.clone();
                    event(
                        graph,
                        "host",
                        "saved",
                        id,
                        json!({"request":state["id"],"run":saved["run"],"revision":saved["revision"]}),
                    );
                    put(db, "graph", task, graph)?;
                    finished = true;
                }
                continue;
            }
            if state["status"] == "waiting"
                && state["authority"]
                    != json!(hash(canonical(&permission::stamp(db, task)?).as_bytes()))
            {
                state = attempt::next(&state)?;
            }
            if state["status"] == "cancel_pending" {
                let run = instances::dispatch_locked(
                    db,
                    task,
                    "status",
                    &json!({"run":state["id"]}),
                    lock,
                )?;
                if run["status"] != "cancelled" {
                    continue;
                }
                state["status"] = json!(AttemptStatus::Cancelled);
            } else if state["status"] == "cleanup_pending" {
                state["status"] = state["after_cleanup"].take();
            } else {
                // Paused tasks still observe work that may already be running.
                if !AttemptStatus::of(&state).can_advance(paused(graph)) {
                    continue;
                }
                let Some(bound) = bindings(db, task, graph, node)? else {
                    continue;
                };
                permission::current(db, task)?;
                state
                    .as_object_mut()
                    .context("Invalid activity")?
                    .remove("waiting_reason");
                if let Err(error) = step(db, task, graph, node, &bound, &mut state, lock) {
                    if permission::current(db, task).is_err() {
                        return Ok(());
                    }
                    if let Some(limit) =
                        error.downcast_ref::<crate::engine::capacity::InstanceLimit>()
                    {
                        state["status"] = json!(AttemptStatus::Queued);
                        state["waiting_reason"] =
                            json!({"reason":"instance_capacity","limit":limit.0});
                    } else {
                        state["status"] = json!(AttemptStatus::Failed);
                        state["error"] = json!(error.to_string());
                    }
                }
            }
            if node["kind"] == "expand" && AttemptStatus::of(&state).needs_driver_cleanup() {
                ensure!(
                    state["status"] != "done" || state["instance"].is_string(),
                    "Missing driver instance"
                );
                if let Some(instance) = state["instance"].as_str() {
                    if find(db, "instance", instance)?.is_some() {
                        let settled = instances::dispatch_locked(
                            db,
                            task,
                            "unload",
                            &json!({"instance":instance}),
                            lock,
                        )?;
                        if settled["status"] != "unloaded" {
                            state["after_cleanup"] = state["status"].take();
                            state["status"] = json!(AttemptStatus::CleanupPending);
                        }
                    } else {
                        ensure!(state["status"] != "done", "Missing driver instance");
                    }
                }
                if state["status"] == "done" {
                    if let Err(error) = accept_expansion(db, task, graph, id, &mut state) {
                        if permission::current(db, task).is_err() {
                            return Ok(());
                        }
                        state["status"] = json!(AttemptStatus::Failed);
                        state["error"] = json!(error.to_string());
                    }
                }
            }
            finished |= state["status"] == "done";
            if graph["activity"][id] != state {
                let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if state["status"] == "authorization_expired" {
                    graph["activity"][id] = state.clone();
                    event(graph, "host", "authorization_expired", id, state.clone());
                    put(&tx, "graph", task, graph)?;
                    state = attempt::next(&state)?;
                    finished = true;
                }
                graph["activity"][id] = state.clone();
                event(graph, "host", field(&state, "status")?, id, state.clone());
                put(&tx, "graph", task, graph)?;
                tx.commit()?;
            }
        }
        if !finished {
            break;
        }
    }
    Ok(())
}

fn step(
    db: &mut Connection,
    task: &str,
    graph: &mut Value,
    node: &Value,
    bound: &Value,
    state: &mut Value,
    lock: &File,
) -> Result<()> {
    if let Some(guard) = node.get("await_decision") {
        let Some(receipt) = crate::provenance::continuation::accepted(db, guard)? else {
            state["bindings"] = bound.clone();
            state["waiting_reason"] = json!({"reason":"decision"});
            return Ok(());
        };
        state["decision_receipt"] = json!(receipt);
    }
    if let Some(saved) = find(db, "graph_ready", field(state, "id")?)? {
        *state = saved;
        return Ok(());
    }
    let stamp = json!(hash(canonical(&permission::stamp(db, task)?).as_bytes()));
    if node["kind"] == "expand"
        && state["authority"] != stamp
        && find(db, "run", field(state, "id")?)?.is_some()
    {
        anyhow::bail!("Driver authority changed before acceptance");
    }
    if node["kind"] == "model" {
        return super::model::schedule(db, task, node, bound, state, &stamp, paused(graph));
    }
    if state["authority"] != stamp {
        state["requests"] = json!({});
        state["authority"] = stamp.clone();
    }
    state["bindings"] = bound.clone();
    if matches!(node["kind"].as_str(), Some("run" | "expand")) {
        return run::schedule(db, task, graph, node, bound, state, lock);
    }
    presentation::schedule(db, task, node, bound, state)
}
