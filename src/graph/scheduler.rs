use super::*;

fn bindings(db: &Connection, task: &str, graph: &Value, node: &Value) -> Result<Option<Value>> {
    let mut result = json!({});
    for dep in node["after"].as_array().into_iter().flatten() {
        if graph["activity"][dep.as_str().unwrap()]["status"] != "done" {
            return Ok(None);
        }
    }
    for (name, r) in node["inputs"].as_object().into_iter().flatten() {
        if let Some(source) = r["input"].as_str() {
            result[name] =
                json!({"source":source,"revision":get(db,"task",task)?["inputs"][source]});
        } else {
            let state = &graph["activity"][field(r, "node")?];
            if state["status"] != "done"
                && !(state["status"] == "saving"
                    && r["port"] == "response"
                    && (graph["nodes"][field(r, "node")?]["save"]["node"] == node["id"]
                        || graph["nodes"][field(r, "node")?]["continuation"] == node["id"]))
            {
                return Ok(None);
            }
            result[name] = state["outputs"]
                .get(field(r, "port")?)
                .context("Missing upstream version")?
                .clone();
        }
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
                    state["status"] = json!("superseded");
                    graph["activity"][id] = state;
                    event(graph, "host", "superseded", id, json!({"save_node":saver}));
                    put(db, "graph", task, graph)?;
                } else if run["status"] == "done" {
                    let saved = run["outputs"]
                        .get(field(&node["save"], "port")?)
                        .context("Missing saved version")?
                        .clone();
                    state["outputs"]["document"] = saved.clone();
                    state["status"] = json!("done");
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
                state = json!({"id":uid(),"attempt":state["attempt"].as_u64().unwrap()+1,"status":"queued"});
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
                state["status"] = json!("cancelled");
            } else if state["status"] == "cleanup_pending" {
                state["status"] = state["after_cleanup"].take();
            } else {
                if !matches!(
                    state["status"].as_str(),
                    Some("queued" | "running" | "permission_required" | "unknown")
                ) {
                    continue;
                }
                // A person's pause starts no new work. Execution already under
                // way keeps being observed so it can settle and be reclaimed.
                if paused(graph)
                    && matches!(
                        state["status"].as_str(),
                        Some("queued" | "permission_required")
                    )
                {
                    continue;
                }
                let Some(bound) = bindings(db, task, graph, node)? else {
                    continue;
                };
                permission::current(db, task)?;
                if let Err(error) = step(db, task, graph, node, &bound, &mut state, lock) {
                    if permission::current(db, task).is_err() {
                        return Ok(());
                    }
                    state["status"] = json!("failed");
                    state["error"] = json!(error.to_string());
                }
            }
            if node["kind"] == "expand"
                && matches!(
                    state["status"].as_str(),
                    Some("done" | "failed" | "cancelled" | "authorization_expired")
                )
            {
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
                            state["status"] = json!("cleanup_pending");
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
                        state["status"] = json!("failed");
                        state["error"] = json!(error.to_string());
                    }
                }
            }
            finished |= state["status"] == "done";
            if graph["activity"][id] != state {
                if state["status"] == "authorization_expired" {
                    event(graph, "host", "authorization_expired", id, state.clone());
                    state = json!({"id":uid(),"status":"queued","attempt":state["attempt"].as_u64().unwrap()+1});
                    finished = true;
                }
                graph["activity"][id] = state.clone();
                event(graph, "host", field(&state, "status")?, id, state.clone());
                put(db, "graph", task, graph)?;
            }
        }
        if !finished {
            break;
        }
    }
    Ok(())
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
        state["status"] = json!("permission_required");
        graph["activity"][id] = state.clone();
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
        json!({"attempt":state["id"],"nodes":additions.iter().map(|n| &n["id"]).collect::<Vec<_>>()}),
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
                "running" | "cancel_pending" => state["status"] = json!("running"),
                "completed" | "awaiting_publication" | "delivered" => {
                    if let Some(rev) = run["candidate"].as_str().or(run["output"].as_str()) {
                        let files = get(db, "revision", rev)?;
                        for name in run["outputs"].as_array().into_iter().flatten() {
                            let name = name.as_str().context("Invalid output")?;
                            let rev = revision(
                                db,
                                &json!({name:files.get(name).context("Missing output")?}),
                            )?;
                            state["outputs"][name] =
                                json!({"revision":rev,"spool":run["spool"],"run":attempt});
                        }
                    }
                    state["status"] = json!("done");
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
            state["instance"] = json!(format!("driver-{attempt}"));
            graph["activity"][field(node, "id")?] = state.clone();
            put(db, "graph", task, graph)?;
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
            state["status"] = json!("queued");
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
            state["status"] = json!("permission_required");
        } else {
            state["status"] = json!("running");
            state["run"] = json!(attempt);
        }
        return Ok(());
    }

    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let t = get(&tx, "task", task)?;
    let delivery = node["kind"] == "delivery";
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
            state["status"] = json!("permission_required");
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
        let record = json!({"id":attempt,"task":task,"node":node["id"],"title":node["title"],"bindings":bound,"package":node["package"],"revision":rev,"outputs":files.as_object().unwrap().keys().collect::<Vec<_>>(),"workspace":t["workspace"]});
        put(&tx, "graph_delivery", &attempt, &record)?;
        state["delivery"] = json!(attempt);
        state["status"] = json!("done");
    } else {
        state["status"] = json!("waiting");
    }
    // Commit permission consumption with its result, even across a process exit.
    put(&tx, "graph_ready", &attempt, state)?;
    tx.commit()?;
    Ok(())
}
