use super::*;

pub(super) fn accept_expansion(
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
