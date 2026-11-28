//! Validate proposed nodes and record the admitted graph revision.
use super::*;

pub(super) fn append_nodes(
    db: &Connection,
    task: &str,
    record: &Value,
    graph: &mut Value,
    additions: &[Value],
    driver: Option<&str>,
) -> Result<()> {
    let nodes = graph["nodes"].as_object_mut().unwrap();
    let mut reserved = BTreeMap::new();
    for surface in nodes
        .values()
        .chain(additions.iter())
        .filter(|n| n["continuation"].is_string())
    {
        let saver = field(&surface["save"], "node")?;
        let owner = field(surface, "continuation")?;
        ensure!(
            reserved
                .insert(saver.to_owned(), owner.to_owned())
                .is_none(),
            "Duplicate saver reservation"
        );
    }
    for surface in additions.iter().filter(|n| n["continuation"].is_string()) {
        let saver = field(&surface["save"], "node")?;
        ensure!(
            !nodes.contains_key(saver) && !additions.iter().any(|n| n["id"] == saver),
            "Continuation requires an unclaimed saver"
        );
    }
    for node in additions {
        if node.get("await_decision").is_some() {
            ensure!(
                driver.is_none(),
                "Decision guard requires controller submission"
            );
            crate::provenance::continuation::declare(db, task, node)?;
        }
        if node.get("versions").is_some() || node.get("publication_review").is_some() {
            ensure!(
                driver.is_none(),
                "Delivery block selection requires controller submission"
            );
            crate::provenance::publication::validate(db, task, node)?;
        }
        let id = field(node, "id")?;
        if let Some(owner) = reserved.get(id) {
            ensure!(
                Some(owner.as_str()) == driver,
                "Saver reserved for continuation"
            );
        }
        ensure!(
            node.get("inputs").is_none_or(Value::is_object),
            "Expected input map"
        );
        for (name, r) in node
            .get("inputs")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            artifacts::safe(name)?;
            if let Some(input) = r.get("input") {
                ensure!(
                    record["inputs"]
                        .get(input.as_str().context("Expected input name")?)
                        .is_some(),
                    "Unknown task input"
                );
            }
        }
        ensure!(!nodes.contains_key(id), "Existing node {id}");
        match field(node, "kind")? {
            "model" => validate_model(record, node)?,
            "run" | "expand" => validate_command(db, record, node)?,
            "surface" => {
                ensure!(
                    if node.get("save").is_some() {
                        node["outputs"] == json!(["response", "document"])
                    } else {
                        node["outputs"] == json!(["response"])
                    },
                    "Invalid surface outputs"
                );
            }
            "delivery" => {
                ensure!(
                    node["inputs"].as_object().is_some_and(|m| !m.is_empty())
                        && node["outputs"] == json!([]),
                    "Expected delivery bindings"
                );
            }
            _ => anyhow::bail!("Unknown node kind"),
        }
        let mut names = BTreeSet::new();
        for p in node["outputs"].as_array().context("Expected output list")? {
            ensure!(
                names.insert(artifacts::safe(p.as_str().context("Invalid output name")?)?),
                "Duplicate output"
            );
        }
        let stored = resolve_panel(db, record, node)?;
        nodes.insert(id.to_owned(), stored);
    }
    validate(nodes)?;
    for node in additions
        .iter()
        .filter(|n| n.get("confirm_delivery").is_some())
    {
        ensure!(
            driver.is_none(),
            "Confirmation requires controller submission"
        );
        let id = field(node, "id")?;
        let actions = confirmation::declare(db, task, graph, &graph["nodes"][id])?;
        graph["nodes"][id]["actions"] = actions;
    }
    for node in additions {
        graph["activity"][field(node, "id")?] = attempt::queued(1);
    }
    graph["revision"] = json!(graph["revision"].as_u64().unwrap() + 1);
    Ok(())
}

fn validate_model(record: &Value, node: &Value) -> Result<()> {
    ensure!(
        field(node, "model")?
            == record["model_grant"]["alias"]
                .as_str()
                .context("No task model grant")?,
        "Model outside task grant"
    );
    ensure!(
        record["model_grant"]["model"].is_string(),
        "This model requires the visual experiment branch"
    );
    ensure!(
        node["inputs"]
            .as_object()
            .is_some_and(|m| m.len() == 1 && m.contains_key("request"))
            && node["outputs"] == json!(["result.json"]),
        "Expected request binding and result.json"
    );
    ensure!(
        node.as_object().unwrap().keys().all(|k| matches!(
            k.as_str(),
            "id" | "kind" | "title" | "after" | "model" | "inputs" | "outputs"
        )),
        "Unexpected model node field"
    );
    Ok(())
}

fn validate_command(db: &Connection, record: &Value, node: &Value) -> Result<()> {
    if node["kind"] == "expand" {
        ensure!(
            !record["editable"]
                .as_array()
                .is_some_and(|images| images.contains(&node["spool"])),
            "Driver requires a read-only spool"
        );
        ensure!(
            node["outputs"] == json!(["subgraph.json"]),
            "Expected subgraph.json"
        );
    }
    ensure!(
        record["work"].get(field(node, "spool")?).is_some(),
        "Spool outside task"
    );
    let spool = field(node, "spool")?;
    let digest = spool.rsplit_once('@').context("Invalid image reference")?.1;
    let manifest = get(db, "admission", digest)?["manifest"].clone();
    if let Some(panels) = manifest.get("panels") {
        crate::panels::validate(panels)?;
    }
    let ports = manifest["inputs"]
        .as_array()
        .context("Invalid manifest inputs")?;
    for port in ports {
        ensure!(
            port["required"] != true || node["inputs"].get(field(port, "name")?).is_some(),
            "Missing required input"
        );
    }
    for name in node["inputs"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k)
    {
        ensure!(
            ports.iter().any(|port| port["name"] == name.as_str()),
            "Undeclared input {name}"
        );
    }
    let argv: Vec<String> = serde_json::from_value(node["argv"].clone())?;
    ensure!(
        argv.first()
            .is_some_and(|s| !s.is_empty() && !s.starts_with('-')),
        "Invalid argv"
    );
    Ok(())
}

fn resolve_panel(db: &Connection, record: &Value, node: &Value) -> Result<Value> {
    let mut stored = node.clone();
    if matches!(node["kind"].as_str(), Some("surface" | "delivery")) {
        ensure!(
            node.as_object().unwrap().keys().all(|k| matches!(
                k.as_str(),
                "id" | "kind"
                    | "title"
                    | "after"
                    | "inputs"
                    | "outputs"
                    | "save"
                    | "continuation"
                    | "versions"
                    | "publication_review"
                    | "confirm_delivery"
                    | "panel"
                    | "spool"
            )),
            "New panels require an admitted declaration"
        );
        if node["kind"] != "delivery" || node.get("panel").is_some() || node.get("spool").is_some()
        {
            let spool = field(node, "spool")?;
            ensure!(
                record["work"].get(spool).is_some(),
                "Panel spool outside task"
            );
            let admission = get(
                db,
                "admission",
                spool.rsplit_once('@').context("Invalid spool")?.1,
            )?;
            let manifest = &admission["manifest"];
            ensure!(
                manifest["schema"] == "an.spool/v2",
                "Expected v2 panel declaration"
            );
            crate::panels::validate(&manifest["panels"])?;
            let panel = manifest["panels"]
                .get(field(node, "panel")?)
                .context("Unknown panel")?;
            ensure!(panel["kind"] == node["kind"], "Panel kind mismatch");
            crate::panels::check_bindings(panel, &node["inputs"])?;
            if stored.get("title").is_none() {
                stored["title"] = panel["title"].clone();
            }
            if node["kind"] == "surface" {
                stored["response"] = crate::panels::schema(panel)?;
            }
            stored["package"] = json!({"declaration":panel,"code_revision":admission["code_revision"],"manifest_digest":admission["manifest_digest"]});
        }
    }
    Ok(stored)
}
