use crate::{
    intent, permission,
    store::{canonical, get, hash},
};
use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

pub(super) fn task(db: &Connection, task: &str, t: &Value, mut graph: Value) -> Result<Value> {
    let authorized = permission::current(db, task).is_ok();
    let authority = if authorized {
        Some(hash(canonical(&permission::stamp(db, task)?).as_bytes()))
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
        intent::public(intent::show(db, task)?)
    } else {
        Value::Null
    };
    Ok(
        json!({"intent":intent,"task":task,"revision":graph["revision"],"nodes":graph["nodes"],"activity":graph["activity"],"closed":t["closed"]==true,"authorized":authorized,"timeline":timeline(&graph, authority.as_deref()),"control":graph.get("control").cloned().unwrap_or(json!("running"))}),
    )
}

pub(super) fn requests(db: &Connection, task: &str, graph: &Value) -> Result<Value> {
    let mut pending = vec![];
    let card = intent::show(db, task)?;
    let stamp = permission::stamp(db, task)?;
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
            let r = get(db, "request", key.as_str().context("Invalid request ID")?)?;
            if r["state"] == "pending" && r["revisions"] == stamp {
                let mut entry = json!({"id":key,"scope":r["scope"],"details":r["details"],"operation":r["operation"]});
                if let Some(version) = r["route"].get("revision") {
                    entry["revision"] = version.clone();
                }
                if card["status"] == "pending" && intent::route_index(&card, &r["route"]).is_some()
                {
                    entry["intent"] = card["id"].clone();
                }
                pending.push(entry);
            }
        }
    }
    let mut stmt=db.prepare("SELECT r.body FROM objects e JOIN objects r ON r.kind='request' AND r.id=json_extract(e.body,'$') WHERE e.kind='graph_evidence_request' AND json_extract(r.body,'$.task')=? AND json_extract(r.body,'$.state')='pending'")?;
    for row in stmt.query_map([task], |row| row.get::<_, String>(0))? {
        let r: Value = serde_json::from_str(&row?)?;
        if r["revisions"] == stamp {
            pending.push(json!({"id":r["id"],"scope":r["scope"],"details":r["details"],"operation":r["operation"]}));
        }
    }
    Ok(json!(pending))
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

#[cfg(test)]
mod timeline_tests {
    use super::*;
    use crate::graph::event;

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
