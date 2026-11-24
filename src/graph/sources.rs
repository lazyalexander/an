use super::*;

pub(super) fn read(
    tx: rusqlite::Transaction,
    task: &str,
    t: &Value,
    mut graph: Value,
    action: &str,
    args: &Value,
) -> Result<Value> {
    let delivery = args["delivery"]
        .as_str()
        .map(|id| get(&tx, "graph_delivery", id))
        .transpose()?;
    let (origin, id) = if let Some(d) = &delivery {
        ensure!(
            d["task"] == task || (t["workspace"].is_string() && t["workspace"] == d["workspace"]),
            "Delivery outside workspace"
        );
        (field(d, "task")?, field(d, "node")?)
    } else {
        (task, field(args, "node")?)
    };
    if origin != task {
        graph = get(&tx, "graph", origin)?;
    }
    ensure!(graph["nodes"].get(id).is_some(), "Unknown node");
    if permission::current(&tx, task).is_err() {
        ensure!(action == "sources", "Evidence requires an active host turn");
        return Ok(
            json!({id:{"node":{"id":id,"kind":graph["nodes"][id]["kind"]},"execution":{"status":graph["activity"][id]["status"]}}}),
        );
    }
    let mut pending = vec![id.to_owned()];
    let mut trace = json!({});
    while let Some(key) = pending.pop() {
        if trace.get(&key).is_some() {
            continue;
        }
        let node = &graph["nodes"][&key];
        if let Some(driver) = graph["drivers"][&key].as_str() {
            pending.push(driver.to_owned());
        }
        if let Some(save) = node["save"]["node"]
            .as_str()
            .filter(|id| graph["nodes"].get(id).is_some())
        {
            pending.push(save.to_owned());
        }
        pending.extend(
            node["inputs"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(_, r)| r["node"].as_str().map(str::to_owned)),
        );
        pending.extend(
            node["after"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned)),
        );
        trace[&key] = json!({"node":node,"execution":graph["activity"][&key],"events":graph["events"].as_array().unwrap().iter().filter(|e|e["node"]==key || e["nodes"].as_array().is_some_and(|ids|ids.contains(&json!(key)))).collect::<Vec<_>>()});
    }
    if action == "evidence" {
        let source = field(args, "source")?;
        let evidence = trace
            .get(source)
            .context("Outside selected lineage")?
            .get("execution")
            .context("Missing attempt")?["evidence"]
            .clone();
        let run = field(&evidence, "run")?;
        let kind = field(args, "type")?;
        ensure!(
            matches!(kind, "input" | "work" | "code" | "log"),
            "Unknown evidence type"
        );
        let auth = permission::stamp(&tx, task).context("Evidence requires an active host turn")?;
        let op = if kind == "log" { "log" } else { "preview" };
        let scope = json!([origin, run, kind, "studio"]);
        let details = json!([evidence[kind]]);
        let key = canonical(&json!([task, auth, scope, details]));
        let pending = find(&tx, "graph_evidence_request", &key)?;
        let flow = if kind == "log" {
            None
        } else {
            Some(json!(["preview", evidence["spool"], "studio"]))
        };
        if let Some(p) = permission::gate(
            &tx,
            task,
            op,
            scope,
            details,
            pending.as_ref().and_then(Value::as_str),
            flow,
        )? {
            put(&tx, "graph_evidence_request", &key, &p["request_id"])?;
            tx.commit()?;
            return Ok(p);
        }
        tx.execute(
            "DELETE FROM objects WHERE kind='graph_evidence_request' AND id=?",
            [&key],
        )?;
        let result = if kind == "log" {
            use std::io::{Read, Seek, SeekFrom};
            let mut f = File::open(root()?.join("runs").join(run).join("log"))
                .context("Log unavailable")?;
            f.seek(SeekFrom::Start(f.metadata()?.len().saturating_sub(65536)))?;
            let mut bytes = vec![];
            f.take(65536).read_to_end(&mut bytes)?;
            json!(String::from_utf8_lossy(&bytes))
        } else {
            get(&tx, "revision", field(&evidence, kind)?)?
        };
        tx.commit()?;
        return Ok(result);
    }
    if origin != task {
        for item in trace.as_object_mut().unwrap().values_mut() {
            item.as_object_mut().unwrap().remove("events");
            let state = &item["execution"];
            *item = json!({"node":{"id":item["node"]["id"],"kind":item["node"]["kind"]},"execution":{"status":state["status"],"evidence":state["evidence"].get("run").map(|run|json!({"run":run})),"bindings":state["bindings"]}});
        }
    }
    Ok(trace)
}
