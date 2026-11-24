use super::*;

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
    if entry.is_null() {
        return Ok(json!({"files":data}));
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

pub fn host(task: &str, action: &str, args: &Value) -> Result<Value> {
    match action {
        "submit" => return dispatch(task, "graph_submit", args),
        "state" => return dispatch(task, "graph_state", args),
        _ => {}
    }
    let mut db = connect()?;
    let _lock = instances::lock(&db, task)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let t = get(&tx, "task", task)?;
    if action == "open" || action == "resume" {
        let delivery = get(&tx, "graph_delivery", field(args, "delivery")?)?;
        ensure!(
            delivery["task"] == task
                || (t["workspace"].is_string() && t["workspace"] == delivery["workspace"]),
            "Delivery outside workspace"
        );
        if action == "resume" {
            let result = crate::workspace::resume_graph(&tx, task, &t, &delivery, &[])?;
            tx.commit()?;
            return Ok(result);
        }
        return package(
            &tx,
            &delivery["bindings"],
            &delivery["package"],
            &Value::Null,
        );
    }
    let mut graph = find(&tx, "graph", task)?
        .unwrap_or(json!({"nodes":{},"revision":0,"events":[],"activity":{}}));
    if action == "finish" {
        if t["closed"] == true {
            return Ok(json!({"closed":true}));
        }
        permission::current(&tx, task)?;
        for state in graph["activity"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, state)| state)
        {
            ensure!(
                matches!(
                    state["status"].as_str(),
                    Some("done" | "failed" | "superseded" | "cancelled")
                ),
                "Unresolved graph node"
            );
        }
        let unresolved:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE kind='run' AND json_extract(body,'$.task')=? AND json_extract(body,'$.status') IN ('running','unknown','cancel_pending'))",[task],|r|r.get(0))?;
        ensure!(!unresolved, "Unresolved run");
        let mut closed = t;
        closed["closed"] = json!(true);
        put(&tx, "task", task, &closed)?;
        put(&tx, "turn", task, &Value::Null)?;
        event(&mut graph, "host", "closed", task, json!({}));
        put(&tx, "graph", task, &graph)?;
        tx.commit()?;
        return Ok(json!({"closed":true}));
    }
    if action == "view" {
        let authorized = permission::current(&tx, task).is_ok();
        if !authorized {
            for state in graph["activity"]
                .as_object_mut()
                .context("Invalid graph activity")?
                .values_mut()
            {
                *state = json!({"status":state["status"]});
            }
            for node in graph["nodes"]
                .as_object_mut()
                .context("Invalid graph nodes")?
                .values_mut()
            {
                *node = json!({"id":node["id"],"kind":node["kind"]});
            }
        }
        return Ok(
            json!({"task":task,"revision":graph["revision"],"nodes":graph["nodes"],"activity":graph["activity"],"closed":t["closed"]==true,"authorized":authorized}),
        );
    }
    if action == "deliveries" {
        let mut stmt=tx.prepare("SELECT body FROM objects WHERE kind='graph_delivery' AND (json_extract(body,'$.task')=? OR json_extract(body,'$.workspace')=?) ORDER BY rowid DESC")?;
        let rows = stmt.query_map(rusqlite::params![task, t["workspace"].as_str()], |r| {
            r.get::<_, String>(0)
        })?;
        let mut records = vec![];
        for row in rows {
            records.push(serde_json::from_str::<Value>(&row?)?);
        }
        return Ok(json!(records));
    }
    if action == "requests" {
        let mut pending = vec![];
        let stamp = permission::stamp(&tx, task)?;
        for state in graph["activity"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, v)| v)
        {
            for (_, key) in state["requests"].as_object().into_iter().flatten() {
                let r = get(&tx, "request", key.as_str().context("Invalid request ID")?)?;
                if r["state"] == "pending" && r["revisions"] == stamp {
                    pending.push(json!({"id":key,"scope":r["scope"],"details":r["details"],"operation":r["operation"]}));
                }
            }
        }
        let mut stmt=tx.prepare("SELECT r.body FROM objects e JOIN objects r ON r.kind='request' AND r.id=json_extract(e.body,'$') WHERE e.kind='graph_evidence_request' AND json_extract(r.body,'$.task')=? AND json_extract(r.body,'$.state')='pending'")?;
        for row in stmt.query_map([task], |row| row.get::<_, String>(0))? {
            let r: Value = serde_json::from_str(&row?)?;
            if r["revisions"] == stamp {
                pending.push(json!({"id":r["id"],"scope":r["scope"],"details":r["details"],"operation":r["operation"]}));
            }
        }
        return Ok(json!(pending));
    }
    if matches!(action, "sources" | "evidence") {
        return sources::read(tx, task, &t, graph, action, args);
    }
    let id = field(args, "node")?;
    let node = graph["nodes"].get(id).context("Unknown node")?.clone();
    let mut state = graph["activity"][id].clone();
    ensure!(
        state["id"] == args["request"]
            && state["authority"]
                == json!(hash(canonical(&permission::stamp(&tx, task)?).as_bytes()))
            && node["kind"] == "surface",
        "Stale surface"
    );
    let request = field(&state, "id")?.to_owned();
    if action == "panel" {
        ensure!(state["status"] == "waiting", "Surface closed");
        let mut result = package(&tx, &state["bindings"], &node["package"], &node["response"])?;
        result["draft"] = find(&tx, "graph_draft", &request)?.unwrap_or(Value::Null);
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
        event(
            &mut graph,
            "user",
            "responded",
            id,
            json!({"request":request,"revision":rev,"bindings":state["bindings"],"response":response}),
        );
        put(&tx, "graph", task, &graph)?;
    } else {
        anyhow::bail!("Unknown graph host action")
    }
    tx.commit()?;
    if action == "respond" {
        drop(_lock);
        wake(task)?;
    }
    Ok(json!({"status":state["status"]}))
}
