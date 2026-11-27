//! Derive reviewable effects from the same validated nodes the scheduler will run.
use super::*;

pub(super) fn review(
    db: &Connection,
    task: &str,
    record: &Value,
    graph: &Value,
    nodes: &[Value],
    cursor: &Value,
) -> Result<Value> {
    let cards = crate::intent::scope_summary(db, task)?;
    let mut routes = Vec::new();
    let mut add = |source: &Value, target: &Value, metadata: bool| -> Result<()> {
        let producer = &graph["nodes"][source["node"].as_str().unwrap_or("")];
        // A saved document retains the saving run's provenance. Its public
        // graph reference still points to the surface's document port.
        let producer = if producer["kind"] == "surface" && source["port"] == "document" {
            let saver = &graph["nodes"][field(&producer["save"], "node")?];
            if saver.is_null() {
                // A future driver supplies the saver. Its output must go
                // through the execution gate once the origin is known.
                return Ok(());
            }
            saver
        } else {
            producer
        };
        let origin = if let Some(input) = source["input"].as_str() {
            format!("task-input:{input}")
        } else {
            match producer["kind"].as_str() {
                Some("run" | "expand") => field(producer, "spool")?.into(),
                Some("model") => format!("model:{}", field(producer, "model")?),
                Some("surface") => "response".into(),
                _ => anyhow::bail!("Unknown candidate producer"),
            }
        };
        let (op, dest) = if metadata {
            ("publish", "graph-metadata".into())
        } else {
            match field(target, "kind")? {
                "run" | "expand" => (
                    if source["input"].is_string() || origin == "response" {
                        "input"
                    } else {
                        "handoff"
                    },
                    field(target, "spool")?.into(),
                ),
                "model" => ("model", format!("model:{}", field(target, "model")?)),
                "delivery" => (
                    "publish",
                    record["workspace"].as_str().unwrap_or(task).into(),
                ),
                "surface" => ("preview", "studio".into()),
                _ => anyhow::bail!("Unknown candidate consumer"),
            }
        };
        let mut rule = json!({"source":source,"operation":op,"origin":origin,"destination":dest});
        if producer["argv"].is_array() {
            rule["producer_argv"] = producer["argv"].clone();
        }
        let mut actual = rule.clone();
        if let Some(input) = source["input"].as_str() {
            actual["revision"] = record["inputs"][input].clone();
        }
        if !cards
            .iter()
            .any(|c| crate::intent::route_index(c, &actual).is_some())
            && !routes.contains(&rule)
        {
            routes.push(rule);
        }
        Ok(())
    };
    for submitted in nodes {
        let node = &graph["nodes"][field(submitted, "id")?];
        for source in node["inputs"]
            .as_object()
            .context("Invalid candidate inputs")?
            .values()
        {
            add(source, node, false)?;
        }
        if node["kind"] == "expand" {
            add(
                &json!({"node":node["id"],"port":"subgraph.json"}),
                node,
                true,
            )?;
        }
    }
    if routes.is_empty() {
        return Ok(cards
            .last()
            .filter(|c| c["status"] == "pending")
            .map(|c| c["id"].clone())
            .unwrap_or(Value::Null));
    }
    Ok(crate::intent::propose(
        db,
        task,
        json!({"turn_scope":routes,"instruction_cursor":cursor}),
    )?["id"]
        .clone())
}

// Candidate input decisions are bound to the run attempt, so the person can
// decide before a guest exists. Recheck and consume them at the actual run.
pub(crate) fn input_gate(
    db: &Connection,
    task: &str,
    spool: &str,
    node_args: &Value,
    port: &str,
    binding: &Value,
) -> Result<Option<Value>> {
    let source = field(&node_args["inputs"], port)?;
    let request = node_args["input_requests"][port].as_str();
    let attempt = field(node_args, "_graph_run")?;
    let origin = binding_origin(binding);
    let op = if binding["spool"].is_string() {
        "handoff"
    } else {
        "input"
    };
    let flow_origin = binding["source"].as_str().unwrap_or(&origin);
    let route =
        crate::intent::graph_route(db, op, &node_args["_graph_sources"][port], spool, binding)?;
    permission::graph_gate(
        db,
        task,
        op,
        json!([source, binding["revision"], spool, port]),
        json!([
            attempt,
            node_args["argv"],
            node_args["outputs"],
            port,
            node_args["inputs"]
        ]),
        request,
        json!([op, flow_origin, spool]),
        route,
    )
}

pub(super) fn inputs_ready(
    db: &mut Connection,
    task: &str,
    node: &Value,
    args: &Value,
    bound: &Value,
    state: &mut Value,
) -> Result<bool> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("SAVEPOINT preview_inputs")?;
    for (port, binding) in bound.as_object().context("Missing bindings")? {
        if let Some(p) = input_gate(&tx, task, field(node, "spool")?, args, port, binding)? {
            let id = field(&p, "request_id")?;
            let row = get(&tx, "request", id)?;
            tx.execute_batch("ROLLBACK TO preview_inputs")?;
            put(&tx, "request", id, &row)?;
            tx.commit()?;
            state["requests"][port] = json!(id);
            state["status"] = json!("permission_required");
            return Ok(false);
        }
    }
    tx.rollback()?;
    Ok(true)
}
