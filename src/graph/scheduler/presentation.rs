use super::*;

pub(super) fn schedule(
    db: &mut Connection,
    task: &str,
    node: &Value,
    bound: &Value,
    state: &mut Value,
) -> Result<()> {
    let attempt = field(state, "id")?.to_owned();
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
