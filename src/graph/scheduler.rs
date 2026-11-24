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
                    && graph["nodes"][field(r, "node")?]["save"]["node"] == node["id"])
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

/// Wake the scheduler after a durable change, including worker settlement.
/// A later state read can repeat this reconciliation after a host crash.
pub fn wake(task: &str) -> Result<()> {
    let mut db = connect()?;
    if find(&db, "graph", task)?.is_none() || permission::current(&db, task).is_err() {
        return Ok(());
    }
    let lock = instances::lock(&db, task)?;
    let mut graph = get(&db, "graph", task)?;
    advance(&mut db, task, &mut graph, &lock)
}

pub(super) fn advance(
    db: &mut Connection,
    task: &str,
    graph: &mut Value,
    lock: &File,
) -> Result<()> {
    let nodes = graph["nodes"].as_object().context("Invalid graph")?.clone();
    loop {
        let mut finished = false;
        for (id, node) in &nodes {
            let mut state = graph["activity"][id].clone();
            if state["status"] == "saving" {
                let saver = field(&node["save"], "node")?;
                let run = &graph["activity"][saver];
                if run["status"] == "superseded" {
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
            if !matches!(
                state["status"].as_str(),
                Some("queued" | "running" | "permission_required" | "unknown")
            ) {
                continue;
            }
            let Some(bound) = bindings(db, task, graph, node)? else {
                continue;
            };
            permission::current(db, task)?;
            let result = step(db, task, node, &bound, &mut state, lock);
            if let Err(e) = result {
                if permission::current(db, task).is_err() {
                    return Ok(());
                }
                state["status"] = json!("failed");
                state["error"] = json!(e.to_string());
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

fn step(
    db: &mut Connection,
    task: &str,
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
    if state["authority"] != stamp {
        state["requests"] = json!({});
        state["authority"] = stamp.clone();
    }
    state["bindings"] = bound.clone();
    let attempt = field(state, "id")?.to_owned();
    if node["kind"] == "run" {
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
        let instance = if let Some(id) = state["instance"].as_str() {
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
            &json!({"instance":instance["id"],"argv":node["argv"],"outputs":node["outputs"],"inputs":inputs,"input_requests":state["requests"],"_graph_bindings":private,"_graph_run":attempt}),
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
        if let Some(p) = permission::gate(
            &tx,
            task,
            op,
            json!([origin, name, dest]),
            json!([attempt, item["revision"]]),
            state["requests"][name].as_str(),
            Some(json!([op, origin, dest])),
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
