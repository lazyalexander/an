use super::*;

pub(super) fn schedule(
    db: &mut Connection,
    task: &str,
    graph: &mut Value,
    node: &Value,
    bound: &Value,
    state: &mut Value,
    lock: &File,
) -> Result<()> {
    let attempt = field(state, "id")?.to_owned();
    if find(db, "run", &attempt)?.is_some() {
        instances::dispatch_locked(db, task, "status", &json!({"run":attempt}), lock)?;
        let run = get(db, "run", &attempt)?;
        state["evidence"] = json!({"run":attempt,"spool":run["spool"],"argv":run["argv"],"work":run["work"],"code":run["image_code"],"code_omissions":run["code_omissions"],"input":run["input"],"outputs":run["outputs"],"exit_code":run["exit_code"],"started_at":run["started_at"],"exited_at":run["exited_at"],"log":attempt});
        match field(&run, "status")? {
            "running" | "cancel_pending" => state["status"] = json!(AttemptStatus::Running),
            "completed" | "awaiting_publication" | "delivered" => {
                if let Some(rev) = run["candidate"].as_str().or(run["output"].as_str()) {
                    state["outputs"] = project_run_outputs(db, &run, &attempt, rev)?;
                }
                state["status"] = json!(AttemptStatus::Done);
            }
            other => {
                state["status"] = json!(other);
                state["error"] = run["error"].clone();
            }
        }
        return Ok(());
    }
    let instance = if node["kind"] == "expand"
        && (state["instance"].is_null() || state["instance"] == format!("driver-{attempt}"))
    {
        if state["instance"].is_null() {
            state["instance"] = json!(format!("driver-{attempt}"));
            graph["activity"][field(node, "id")?] = state.clone();
            event(
                graph,
                "host",
                "driver_reserved",
                field(node, "id")?,
                json!({}),
            );
            put(db, "graph", task, graph)?;
        }
        instances::dispatch_locked(
            db,
            task,
            "load",
            &json!({"spool":node["spool"],"_parallel":true,"_driver":attempt}),
            lock,
        )?
    } else if let Some(id) = state["instance"].as_str() {
        get(db, "instance", id)?
    } else {
        instances::dispatch_locked(
            db,
            task,
            "load",
            &json!({"spool":node["spool"],"_parallel":true}),
            lock,
        )?
    };
    state["instance"] = instance["id"].clone();
    if !instance["active"].is_null() {
        state["status"] = json!(AttemptStatus::Queued);
        return Ok(());
    }
    let mut inputs = json!({});
    let mut private = json!({});
    for (port, item) in bound.as_object().unwrap() {
        if item["source"].is_string() {
            inputs[port] = item["source"].clone()
        } else {
            let token = format!("graph-{attempt}-{port}");
            inputs[port] = json!(token);
            private[token] = item.clone();
        }
    }
    let result = instances::dispatch_locked(
        db,
        task,
        "run",
        &json!({"instance":instance["id"],"argv":node["argv"],"outputs":node["outputs"],"inputs":inputs,"input_requests":state["requests"],"_graph_bindings":private,"_graph_sources":node["inputs"],"_graph_run":attempt}),
        lock,
    )?;
    if let Some(request) = result["request_id"].as_str() {
        state["requests"][field(&result, "port")?] = json!(request);
        state["status"] = json!(AttemptStatus::PermissionRequired);
    } else {
        state["status"] = json!(AttemptStatus::Running);
        state["run"] = json!(attempt);
    }
    Ok(())
}

/// Publish port revisions and their owner together. Another task may finish
/// and collect revisions before the scheduler persists its graph activity.
fn project_run_outputs(
    db: &mut Connection,
    run: &Value,
    attempt: &str,
    revision_id: &str,
) -> Result<Value> {
    if let Some(outputs) = run.get("port_revisions") {
        return Ok(outputs.clone());
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let files = get(&tx, "revision", revision_id)?;
    let mut outputs = json!({});
    for name in run["outputs"].as_array().context("Missing outputs")? {
        let name = name.as_str().context("Invalid output")?;
        let entry = files.get(name).context("Missing output")?;
        let id = revision(&tx, &json!({name:entry}))?;
        crate::provenance::flow::produced(&tx, field(run, "task")?, attempt, &id)?;
        outputs[name] = json!({"revision":id, "spool":run["spool"], "run":attempt});
    }
    // Keep the run rowid used by the delivery listing cursor.
    tx.execute("UPDATE objects SET body=json_set(body,'$.port_revisions',json(?)) WHERE kind='run' AND id=?",
        rusqlite::params![outputs.to_string(), attempt])?;
    tx.commit()?;
    Ok(outputs)
}
