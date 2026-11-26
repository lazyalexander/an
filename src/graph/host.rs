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
    if let Some(panel) = entry.get("declaration") {
        let mut result = json!({"files":data,"declaration":panel,"response":schema,"assets":{}});
        if let Some(path) = panel["html"].as_str() {
            let files = get(db, "revision", field(entry, "code_revision")?)?;
            result["html"] = json!(String::from_utf8(artifacts::bytes(&files[path])?)?);
        }
        return Ok(result);
    }
    if entry.is_null() {
        return Ok(json!({"files":data}));
    }
    if !schema.is_null() {
        validation::validate_schema(schema)?;
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
        "say" => return dispatch(task, "graph_say", args),
        _ => {}
    }
    let mut db = connect()?;
    let _lock = instances::lock(&db, task)?;
    if matches!(action, "control" | "view" | "finish") {
        super::model::reconcile_sends(&db, task)?;
    }
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
    if action == "control" {
        let (result, runs, drivers) = control(&tx, task, &t, &mut graph, args)?;
        tx.commit()?;
        // Cancelling observes and stops containers; it never relaunches work.
        for run in runs {
            instances::dispatch_locked(&mut db, task, "cancel", &json!({"run":run}), &_lock)?;
        }
        for instance in drivers {
            instances::dispatch_locked(
                &mut db,
                task,
                "unload",
                &json!({"instance":instance}),
                &_lock,
            )?;
        }
        drop(_lock);
        wake(task)?;
        return Ok(result);
    }
    if action == "instruct" {
        let result = instruct(&tx, task, &t, &mut graph, args)?;
        tx.commit()?;
        return Ok(result);
    }
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
        let authority = if authorized {
            Some(hash(canonical(&permission::stamp(&tx, task)?).as_bytes()))
        } else {
            None
        };
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
        let intent = if authorized {
            crate::intent::public(crate::intent::show(&tx, task)?)
        } else {
            Value::Null
        };
        return Ok(
            json!({"intent":intent,"task":task,"revision":graph["revision"],"nodes":graph["nodes"],"activity":graph["activity"],"closed":t["closed"]==true,"authorized":authorized,"timeline":timeline(&graph, authority.as_deref()),"control":graph.get("control").cloned().unwrap_or(json!("running"))}),
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
        let card = crate::intent::show(&tx, task)?;
        let stamp = permission::stamp(&tx, task)?;
        for state in graph["activity"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(_, v)| v)
        {
            for key in state["requests"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.values())
                .chain(state.get("proposal_request"))
            {
                let r = get(&tx, "request", key.as_str().context("Invalid request ID")?)?;
                if r["state"] == "pending" && r["revisions"] == stamp {
                    let mut entry = json!({"id":key,"scope":r["scope"],"details":r["details"],"operation":r["operation"]});
                    if let Some(version) = r["route"].get("revision") {
                        entry["revision"] = version.clone();
                    }
                    if card["status"] == "pending"
                        && crate::intent::route_index(&card, &r["route"]).is_some()
                    {
                        entry["intent"] = card["id"].clone();
                    }
                    pending.push(entry);
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
    if matches!(action, "panel" | "decision") {
        ensure!(state["status"] == "waiting", "Surface closed");
        let draft = find(&tx, "graph_draft", &request)?.unwrap_or(Value::Null);
        if action == "decision" {
            ensure!(
                node["package"].get("declaration").is_some(),
                "Expected a declared decision"
            );
            return Ok(json!({"draft":draft}));
        }
        let mut result = package(&tx, &state["bindings"], &node["package"], &node["response"])?;
        result["draft"] = draft;
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
        if node["package"]
            .get("declaration")
            .is_some_and(|p| p["decision"]["type"] == "region")
        {
            for (position, size) in [("x", "width"), ("y", "height")] {
                let start = response[position]
                    .as_f64()
                    .context("Invalid region origin")?;
                let length = response[size].as_f64().context("Invalid region size")?;
                ensure!(
                    length > 0.0 && start + length <= 1.0,
                    "Region extends outside artifact or has zero area"
                );
            }
        }
        if node["package"]
            .get("declaration")
            .is_some_and(|p| p["decision"]["type"] == "sketch")
        {
            for stroke in response["strokes"].as_array().into_iter().flatten() {
                for point in stroke["points"].as_array().into_iter().flatten() {
                    ensure!(
                        point.as_array().is_some_and(|p| p.len() == 3),
                        "Sketch points are [x, y, pressure]"
                    );
                }
            }
        }
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

/// A person's control, executed by the host and never relayed to an agent.
///
/// `pause` starts no new work; running work is still observed and settles.
/// `resume` clears the pause. `stop` cancels every unsettled node and pauses,
/// so nothing new starts until the person resumes; cancelled nodes can then
/// be retried by the agent. Returns the runs to cancel and the idle driver
/// instances to unload once this transaction commits.
fn control(
    tx: &Connection,
    task: &str,
    t: &Value,
    graph: &mut Value,
    args: &Value,
) -> Result<(Value, Vec<String>, Vec<String>)> {
    let kind = field(args, "action")?;
    ensure!(
        matches!(kind, "pause" | "resume" | "stop"),
        "Unknown control {kind}"
    );
    permission::current(tx, task)?;
    ensure!(t["closed"] != true, "Task closed");
    let mut cancelled = vec![];
    let mut runs = Vec::new();
    let mut drivers = vec![];
    if kind == "stop" {
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
                if i["active"] != id
                    && !(i["active"].is_null()
                        && matches!(i["intent"].as_str(), Some("stop" | "unload")))
                {
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
            runs.push(id);
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
                            drivers.push(instance.to_owned());
                        }
                    }
                }
            }
            if matches!(
                state["status"].as_str(),
                Some(
                    "running"
                        | "unknown"
                        | "cleanup_pending"
                        | "queued"
                        | "permission_required"
                        | "waiting"
                        | "saving"
                        | "cancel_pending"
                )
            ) {
                if runs.contains(&attempt) {
                    state["status"] = json!("cancel_pending");
                    continue;
                }
                if state["instance"]
                    .as_str()
                    .is_some_and(|i| drivers.iter().any(|d| d == i))
                {
                    state["status"] = json!("cleanup_pending");
                    state["after_cleanup"] = json!("cancelled");
                    continue;
                }
                state["status"] = json!("cancelled");
                cancelled.push(id.clone());
            }
        }
    }
    graph["control"] = json!(if kind == "resume" {
        "running"
    } else {
        "paused"
    });
    let past = match kind {
        "pause" => "paused",
        "resume" => "resumed",
        _ => "stopped",
    };
    event(
        graph,
        "user",
        past,
        task,
        json!({"cancelled":cancelled,"cancelling":runs}),
    );
    put(tx, "graph", task, graph)?;
    Ok((
        json!({"control":graph["control"],"cancelled":cancelled,"cancelling":runs}),
        runs,
        drivers,
    ))
}

/// Project only user-facing fields. Execution event data may contain prompts,
/// commands, intermediate results and private binding metadata.
fn timeline(graph: &Value, authority: Option<&str>) -> Vec<Value> {
    let mut rows = Vec::new();
    let mut decisions = std::collections::HashMap::new();
    for event in graph["events"].as_array().into_iter().flatten() {
        let id = &event["node"];
        let data = &event["data"];
        let kind = event["kind"].as_str().unwrap_or("");
        let node = &graph["nodes"][id.as_str().unwrap_or("")];
        let mut row = json!({"cursor":event["cursor"], "time_ms":event["time_ms"]});
        match kind {
            "done" if node["kind"] == "delivery" && data["delivery"].is_string() => {
                row["kind"] = json!("delivery");
                row["delivery"] = data["delivery"].clone();
            }
            _ if authority.is_none() => continue,
            "instruction" if data["authority"].as_str() == authority => {
                row["kind"] = json!("instruction");
                row["text"] = data["text"].clone();
            }
            "message" if event["actor"] == "agent" => {
                row["kind"] = json!("message");
                row["text"] = data["text"].clone();
                row["purpose"] = data["purpose"].clone();
                row["reply_to"] = data["reply_to"].clone();
            }
            "waiting" if node["kind"] == "surface" && data["authority"].as_str() == authority => {
                let Some(request) = data["id"].as_str() else {
                    continue;
                };
                if decisions.contains_key(request) {
                    continue;
                }
                decisions.insert(request, rows.len());
                row["kind"] = json!("decision");
                row["request"] = json!(request);
                row["node"] = id.clone();
            }
            "responded" => {
                if let Some(&index) = data["request"].as_str().and_then(|key| decisions.get(key)) {
                    let item: &mut Value = &mut rows[index];
                    item["response"] = data["response"].clone();
                }
                continue;
            }
            "submitted" | "expanded" => {
                // Fold the agent's work into one readable stage with progress;
                // commands, inputs and tool calls stay behind the sources view.
                let added = if kind == "submitted" {
                    &event["nodes"]
                } else {
                    &data["nodes"]
                };
                let Some(stage) = stage(graph, added) else {
                    continue;
                };
                row["kind"] = json!("stage");
                row["origin"] = json!(if kind == "submitted" {
                    "agent"
                } else {
                    "driver"
                });
                for (key, value) in stage {
                    row[key] = value;
                }
            }
            "failed" | "unknown" => {
                row["kind"] = json!("failure");
                row["node"] = id.clone();
                row["request"] = data["id"].clone();
                row["reason"] = json!(if kind == "unknown" {
                    "unknown"
                } else {
                    failure_reason(data["error"].as_str().unwrap_or(""))
                });
            }
            "paused" | "resumed" | "stopped" | "closed" => {
                row["kind"] = json!("control");
                row["action"] = json!(kind);
            }
            _ => continue,
        }
        rows.push(row);
    }
    rows
}

/// Work nodes added together, as titled steps with their current status.
fn stage(graph: &Value, added: &Value) -> Option<serde_json::Map<String, Value>> {
    let mut steps = vec![];
    for id in added
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let node = &graph["nodes"][id];
        if !matches!(node["kind"].as_str(), Some("run" | "expand" | "model")) {
            continue; // decisions and deliveries have their own rows
        }
        let status = graph["activity"][id]["status"].as_str().unwrap_or("queued");
        steps.push(json!({"node":id,"title":node.get("title").cloned().unwrap_or(json!(id)),"status":status}));
    }
    if steps.is_empty() {
        return None;
    }
    let done = steps.iter().filter(|s| s["status"] == "done").count();
    let any = |states: &[&str]| {
        steps
            .iter()
            .any(|s| states.contains(&s["status"].as_str().unwrap_or("")))
    };
    let state = if any(&["failed", "unknown"]) {
        "failed"
    } else if done == steps.len() {
        "done"
    } else if any(&[
        "queued",
        "running",
        "permission_required",
        "cleanup_pending",
    ]) {
        "running"
    } else {
        "stopped"
    };
    let mut row = serde_json::Map::new();
    row.insert("total".into(), json!(steps.len()));
    row.insert("done".into(), json!(done));
    row.insert("state".into(), json!(state));
    row.insert("steps".into(), json!(steps));
    Some(row)
}

/// A readable category for a failure. The person sees the locale text for the
/// category; the raw error stays available through the node's sources.
fn failure_reason(error: &str) -> &'static str {
    let lower = error.to_lowercase();
    if lower.contains("denied") || lower.contains("host policy") {
        "denied"
    } else if lower.contains("authority") || lower.contains("turn") {
        "authority"
    } else if lower.contains("exited") || lower.contains("exit status") {
        "exit"
    } else if lower.contains("output") {
        "output"
    } else if lower.contains("budget") || lower.contains("quota") || lower.contains("exceeds") {
        "budget"
    } else if lower.contains("declaration")
        || lower.contains("manifest")
        || lower.contains("admission")
    {
        "declaration"
    } else {
        "other"
    }
}

/// Relay a person's instruction to the agent, unchanged.
///
/// An never interprets the text. It records who sent it (the person, under
/// the current turn), when, and which graph revision the person was looking
/// at, so the agent can tell when an instruction refers to an older state.
/// The agent reads it as a `user`/`instruction` event from graph state. An
/// instruction never answers a waiting surface.
fn instruct(
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

#[cfg(test)]
mod timeline_tests {
    use super::*;

    #[test]
    fn agent_work_folds_into_stages_with_readable_failures() {
        let mut graph = json!({"nodes":{
            "render":{"kind":"run","title":"渲染草图","argv":["secret-cmd"]},
            "judge":{"kind":"model","title":"挑选方案"},
            "pick":{"kind":"surface"},
            "fix":{"kind":"run"}},
            "activity":{"render":{"status":"done"},"judge":{"status":"running"},
                        "pick":{"status":"waiting"},"fix":{"status":"failed"}},
            "events":[]});
        let events = graph["events"].as_array_mut().unwrap();
        events.push(json!({"cursor":1,"actor":"agent","kind":"submitted",
            "nodes":["render","judge","pick"]}));
        events.push(json!({"cursor":2,"actor":"agent","kind":"submitted","nodes":["pick"]}));
        event(
            &mut graph,
            "host",
            "expanded",
            "driver",
            json!({"nodes":["fix"]}),
        );
        event(
            &mut graph,
            "host",
            "failed",
            "fix",
            json!({"id":"a","error":"Guest command exited exit status: 7"}),
        );
        event(
            &mut graph,
            "host",
            "failed",
            "fix",
            json!({"id":"b","error":"Denied by workspace flow policy"}),
        );
        let rows = timeline(&graph, Some("current"));
        let stages: Vec<_> = rows.iter().filter(|r| r["kind"] == "stage").collect();
        assert_eq!(
            stages.len(),
            2,
            "a submission of only decisions is not a stage"
        );
        assert_eq!(stages[0]["total"], 2);
        assert_eq!(stages[0]["done"], 1);
        assert_eq!(stages[0]["state"], "running");
        assert_eq!(stages[0]["steps"][0]["title"], "渲染草图");
        assert_eq!(stages[1]["origin"], "driver");
        assert_eq!(stages[1]["steps"][0]["title"], "fix");
        assert_eq!(stages[1]["state"], "failed");
        assert!(!serde_json::to_string(&rows).unwrap().contains("secret-cmd"));
        let reasons: Vec<_> = rows
            .iter()
            .filter(|r| r["kind"] == "failure")
            .map(|r| r["reason"].clone())
            .collect();
        assert_eq!(reasons, [json!("exit"), json!("denied")]);
        assert!(timeline(&graph, None).iter().all(|r| r["kind"] != "stage"));
    }

    #[test]
    fn projection_preserves_attempts_and_filters_private_history() {
        let mut graph = json!({"nodes":{"a":{"kind":"surface"},"b":{"kind":"surface"},
            "end":{"kind":"delivery"}},"events":[]});
        let waiting = json!({"id":"first","authority":"current","argv":"hidden"});
        event(
            &mut graph,
            "user",
            "instruction",
            "task",
            json!({"text":"revise","authority":"current"}),
        );
        event(&mut graph, "host", "waiting", "a", waiting.clone());
        event(&mut graph, "host", "waiting", "a", waiting);
        event(
            &mut graph,
            "host",
            "waiting",
            "b",
            json!({"id":"second","authority":"current"}),
        );
        event(
            &mut graph,
            "user",
            "responded",
            "a",
            json!({"request":"first","response":{"choice":"B"}}),
        );
        event(
            &mut graph,
            "host",
            "failed",
            "job",
            json!({"id":"job1","error":"hidden"}),
        );
        event(&mut graph, "user", "paused", "task", json!({}));
        event(
            &mut graph,
            "host",
            "running",
            "job",
            json!({"prompt":"hidden"}),
        );
        event(
            &mut graph,
            "host",
            "waiting",
            "a",
            json!({"id":"retry","authority":"current"}),
        );
        event(
            &mut graph,
            "host",
            "done",
            "end",
            json!({"delivery":"published","bindings":"hidden"}),
        );
        let rows = timeline(&graph, Some("current"));
        assert_eq!(
            rows.iter()
                .map(|r| r["cursor"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 4, 6, 7, 9, 10]
        );
        assert_eq!(rows[1]["response"]["choice"], "B");
        assert_eq!(rows[2]["request"], "second");
        assert!(rows[5].get("response").is_none());
        assert!(!json!(rows).to_string().contains("hidden"));
        assert_eq!(timeline(&graph, None), vec![rows[6].clone()]);
        assert_eq!(timeline(&graph, Some("next")).len(), 3);
    }
}
