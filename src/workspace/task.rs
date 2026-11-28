use super::{config, resolve, resolve_grants, Grants, Selector};
use crate::store::{find, get, inputs, put, revision, uid};
use anyhow::{anyhow, bail, ensure, Context, Result};
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::path::PathBuf;
pub(super) fn new_task(
    db: &Transaction,
    images: &[String],
    editable: &[String],
    inputs: Value,
) -> Result<(String, Value)> {
    ensure!(
        editable.iter().all(|s| images.contains(s)),
        "Editable spool must be granted"
    );
    let mut work = json!({});
    for image in images {
        get(db, "image", image)?;
        work[image] = json!(revision(db, &json!({}))?);
    }
    Ok((
        uid(),
        json!({"work":work,"inputs":inputs,"editable":editable}),
    ))
}
pub(super) fn create(
    tx: &Transaction,
    grants: Grants,
    selector: Selector,
    input: Vec<PathBuf>,
    model: Option<String>,
    model_calls: Option<u64>,
) -> Result<Value> {
    let result = if let Some(key) = selector.identity()? {
        ensure!(
            grants.spool.is_empty() && grants.allow_tool.is_empty() && grants.editable.is_empty(),
            "Bound tasks inherit workspace configuration"
        );
        let id = resolve(tx, &key, true)?;
        if !input.is_empty() {
            let mut task = get(tx, "task", &id)?;
            task["inputs"] = inputs(tx, &input)?;
            put(tx, "task", &id, &task)?;
        }
        json!({"task":id})
    } else {
        if grants.spool.is_empty() && grants.allow_tool.is_empty() && model.is_none() {
            bail!("Grant a spool, model or downstream tool")
        }
        let (images, tools) = resolve_grants(tx, &grants)?;
        let (id, mut t) = new_task(tx, &images, &grants.editable, inputs(tx, &input)?)?;
        if !grants.allow_tool.is_empty() {
            t["tools"] = tools;
        }
        put(tx, "task", &id, &t)?;
        json!({"task":id,"mcp_servers":config(json!(["mcp","--task",id]))?})
    };
    if let Some(alias) = model {
        let cfg = crate::config::load()?;
        let resolved = cfg.aliases().get(&alias).context("Unknown model alias")?;
        let id = result["task"].as_str().unwrap();
        let mut t = get(tx, "task", id)?;
        t["model_grant"] = json!({"alias":alias,"model":resolved,"budget":model_calls.unwrap_or_else(|| cfg.limit("model_default_calls"))});
        put(tx, "task", id, &t)?;
    }

    Ok(result)
}
pub(super) fn resume(tx: &Transaction, delivery: String, editable: Vec<String>) -> Result<Value> {
    if let Some(group) = find(tx, "graph_delivery", &delivery)? {
        let origin = group["task"].as_str().context("Missing delivery origin")?;
        let task = get(tx, "task", origin)?;
        let result = resume_graph(tx, origin, &task, &group, &editable)?;
        return Ok(result);
    }
    let r = get(tx, "run", &delivery)?;
    ensure!(r["status"] == "delivered", "Expected a delivered run");
    let image = r["spool"]
        .as_str()
        .ok_or_else(|| anyhow!("Invalid spool"))?
        .to_owned();
    let (id, mut t) = new_task(
        tx,
        std::slice::from_ref(&image),
        &editable,
        json!({"original":r["input"]}),
    )?;
    t["work"][image] = r.get("result_work").unwrap_or(&r["work"]).clone();
    put(tx, "task", &id, &t)?;

    Ok(json!({"task":id,"mcp_servers":config(json!(["mcp","--task",id]))?}))
}
pub(crate) fn resume_graph(
    db: &Transaction,
    task: &str,
    t: &Value,
    delivery: &Value,
    editable: &[String],
) -> Result<Value> {
    let images: Vec<String> = t["work"]
        .as_object()
        .context("Invalid task")?
        .keys()
        .cloned()
        .collect();
    let (id, mut fresh) = new_task(
        db,
        &images,
        editable,
        json!({"original":delivery["revision"]}),
    )?;
    fresh["workspace"] = t["workspace"].clone();
    fresh["tools"] = t["tools"].clone();
    fresh["origin"] = json!({"delivery":delivery["id"],"from_task":task});
    put(db, "task", &id, &fresh)?;
    Ok(
        json!({"task":id,"origin":fresh["origin"],"mcp_servers":config(json!(["mcp","--task",id]))?}),
    )
}
