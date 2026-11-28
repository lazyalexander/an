use super::*;

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

fn accept_expansion(
    db: &mut Connection,
    task: &str,
    graph: &mut Value,
    id: &str,
    state: &mut Value,
) -> Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure!(
        state["authority"] == json!(hash(canonical(&permission::stamp(&tx, task)?).as_bytes())),
        "Driver authority changed before acceptance"
    );
    let proposal_revision = field(&state["outputs"]["subgraph.json"], "revision")?;
    let origin = &graph["nodes"][id]["spool"];
    if let Some(request) = permission::graph_gate(
        &tx,
        task,
        "publish",
        json!([origin, "graph-metadata", state["id"], proposal_revision]),
        json!([state["id"], proposal_revision]),
        state["proposal_request"].as_str(),
        json!(["publish", origin, "graph-metadata"]),
        crate::intent::graph_route(
            &tx,
            "publish",
            &json!({"node":id,"port":"subgraph.json"}),
            "graph-metadata",
            &state["outputs"]["subgraph.json"],
        )?,
    )? {
        state["proposal_request"] = request["request_id"].clone();
        state["status"] = json!(AttemptStatus::PermissionRequired);
        graph["activity"][id] = state.clone();
        event(graph, "host", "proposal_permission_required", id, json!({}));
        put(&tx, "graph", task, graph)?;
        tx.commit()?;
        return Ok(());
    }
    let revision = get(
        &tx,
        "revision",
        field(&state["outputs"]["subgraph.json"], "revision")?,
    )?;
    let proposal: Value = serde_json::from_slice(&artifacts::bytes(&revision["subgraph.json"])?)?;
    let additions = proposal["nodes"]
        .as_array()
        .filter(|n| !n.is_empty())
        .context("Expected proposed nodes")?;
    for surface in graph["nodes"]
        .as_object()
        .unwrap()
        .values()
        .filter(|n| n["continuation"] == id)
    {
        ensure!(
            additions.iter().any(|n| n["id"] == surface["save"]["node"]),
            "Continuation omitted save run"
        );
    }
    let budget = crate::config::load()?.limit("graph_expansion_budget") as usize;
    ensure!(
        graph["drivers"].as_object().map_or(0, |m| m.len()) + additions.len() <= budget,
        "Task expansion budget exceeded"
    );
    let mut next = graph.clone();
    append_nodes(
        &tx,
        task,
        &get(&tx, "task", task)?,
        &mut next,
        additions,
        Some(id),
    )?;
    for addition in additions {
        next["drivers"][field(addition, "id")?] = json!(id);
    }
    next["activity"][id] = state.clone();
    event(
        &mut next,
        "host",
        "expanded",
        id,
        json!({"attempt":state["id"],"decision_receipt":state["decision_receipt"],"nodes":additions.iter().map(|n| &n["id"]).collect::<Vec<_>>()}),
    );
    put(&tx, "graph", task, &next)?;
    tx.commit()?;
    *graph = next;
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
    let attempt = field(state, "id")?.to_owned();
    if node["kind"] == "run" || node["kind"] == "expand" {
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
        return Ok(());
    }

    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let t = get(&tx, "task", task)?;
    let delivery = node["kind"] == "delivery";
    let mut release = None;
    if node.get("publication_review").is_some() {
        release = crate::provenance::release::authorized(&tx, task, &attempt)?;
        if release.is_none() {
            state["waiting_reason"] = json!({"reason":"publication_review"});
            return Ok(());
        }
    }
    let op = if delivery { "publish" } else { "preview" };
    // Keep grants for the whole set atomic when one file still needs consent.
    tx.execute_batch("SAVEPOINT graph_access")?;
    for (name, item) in bound.as_object().unwrap() {
        let origin = super::binding_origin(item);
        let dest = if delivery {
            t["workspace"].as_str().unwrap_or(task)
        } else {
            "studio"
        };
        if let Some(p) = permission::graph_gate(
            &tx,
            task,
            op,
            json!([origin, name, dest]),
            json!([attempt, item["revision"]]),
            state["requests"][name].as_str(),
            json!([op, origin, dest]),
            crate::intent::graph_route(&tx, op, &node["inputs"][name], dest, item)?,
        )? {
            let id = field(&p, "request_id")?;
            let row = get(&tx, "request", id)?;
            tx.execute_batch("ROLLBACK TO graph_access")?;
            put(&tx, "request", id, &row)?;
            state["requests"][name] = json!(id);
            state["status"] = json!(AttemptStatus::PermissionRequired);
            tx.commit()?;
            return Ok(());
        }
    }
    if delivery {
        if node.get("package").is_some() {
            package(&tx, bound, &node["package"], &Value::Null)?;
        }
        let mut files = json!({});
        for (name, item) in bound.as_object().unwrap() {
            let rev = get(&tx, "revision", field(item, "revision")?)?;
            for (file, entry) in rev.as_object().context("Invalid file set")? {
                let path = format!("{}/{}", artifacts::safe(name)?, artifacts::safe(file)?);
                ensure!(
                    files.get(&path).is_none(),
                    "Duplicate delivery path: {path}"
                );
                files[path] = entry.clone();
            }
        }
        for path in files.as_object().unwrap().keys() {
            for (slash, _) in path.match_indices('/') {
                ensure!(
                    files.get(&path[..slash]).is_none(),
                    "Delivery file/directory conflict: {path}"
                );
            }
        }
        let rev = revision(&tx, &files)?;
        let mut record = json!({"id":attempt,"task":task,"node":node["id"],"title":node["title"],"bindings":bound,"package":node["package"],"revision":rev,"outputs":files.as_object().unwrap().keys().collect::<Vec<_>>(),"workspace":t["workspace"]});
        if let Some(receipt) = release {
            record["release_receipt"] = receipt["id"].clone();
            state["release_receipt"] = receipt["id"].clone();
        }
        crate::provenance::flow::record(
            &tx,
            task,
            crate::provenance::flow::Phase::Published,
            &attempt,
            t["workspace"].as_str().unwrap_or(task),
            bound,
        )?;
        crate::provenance::publication::publish(&tx, task, node, &record)?;
        put(&tx, "graph_delivery", &attempt, &record)?;
        state["delivery"] = json!(attempt);
        state["status"] = json!(AttemptStatus::Done);
    } else {
        state["status"] = json!(AttemptStatus::Waiting);
    }
    // Commit permission consumption with its result, even across a process exit.
    put(&tx, "graph_ready", &attempt, state)?;
    tx.commit()?;
    Ok(())
}
