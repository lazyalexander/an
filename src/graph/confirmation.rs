//! Freeze a human-readable confirmation against an existing reviewed delivery attempt.
use super::field;
use crate::{
    permission,
    store::{find, get, put, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

pub(super) fn declare(
    db: &Connection,
    task: &str,
    graph: &Value,
    surface: &Value,
) -> Result<Value> {
    permission::controller(db, task)?;
    let panel = &surface["package"]["declaration"];
    ensure!(
        surface["kind"] == "surface"
            && surface.get("save").is_none()
            && panel["decision"]["type"] == "confirm"
            && panel.get("html").is_none(),
        "Expected a basic confirmation surface"
    );
    let target = &surface["confirm_delivery"];
    let id = field(target, "node")?;
    let delivery = graph["nodes"]
        .get(id)
        .context("Unknown reviewed delivery")?;
    let attempt = field(target, "attempt")?;
    ensure!(
        delivery.get("publication_review").is_some()
            && graph["activity"][id]["id"] == attempt
            && graph["activity"][id]["status"] == "queued"
            && surface["inputs"] == delivery["inputs"],
        "Expected the pending reviewed delivery and its exact inputs"
    );
    let doc = delivery["versions"]
        .as_object()
        .and_then(|v| v.values().next())
        .context("Missing reviewed version")?;
    let workspace = get(db, "task", task)?["workspace"].clone();
    let name = format!("{} v{}", field(doc, "artifact")?, doc["version"]);
    Ok(json!([
        {"operation":"confirm_accuracy","document":doc,"label":format!("Confirm accuracy of {name}")},
        {"operation":"approve_publication","document":doc,"workspace":workspace,"node":id,"attempt":attempt,
         "label":format!("Approve publication of {name} to workspace {}",workspace.as_str().context("Missing workspace")?)}
    ]))
}

pub(super) fn record(
    db: &Connection,
    task: &str,
    graph: &Value,
    node: &Value,
    state: &Value,
    response: &Value,
) -> Result<Value> {
    let accepted = response["confirmed"]
        .as_bool()
        .context("Expected explicit confirmation")?;
    let target = &node["confirm_delivery"];
    let attempt = field(target, "attempt")?;
    let id = field(target, "node")?;
    let delivery = &graph["nodes"][id];
    let binding = state["bindings"]
        .as_object()
        .and_then(|b| b.values().next())
        .context("Missing displayed binding")?;
    let doc = &node["actions"][0]["document"];
    ensure!(
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pv_version WHERE artifact=? AND version=? AND task=?
          AND json_extract(material,'$.attempt')=? AND json_extract(material,'$.revision')=?)",
            rusqlite::params![
                doc["artifact"].as_str(),
                doc["version"].as_u64(),
                task,
                binding["run"].as_str().or(binding["attempt"].as_str()),
                binding["revision"].as_str()
            ],
            |r| r.get::<_, bool>(0)
        )?,
        "Displayed material differs from the reviewed version"
    );
    ensure!(
        graph["activity"][id]["id"] == attempt
            && graph["activity"][id]["status"] == "queued"
            && find(db, "publication_release", attempt)?.is_none(),
        "Delivery attempt already released or closed"
    );
    let mut receipts = node["actions"].clone();
    for receipt in receipts
        .as_array_mut()
        .context("Missing confirmation actions")?
    {
        receipt["id"] = json!(uid());
        receipt["confirmed"] = json!(accepted);
        receipt["actor"] = json!(crate::permission::controller_identity(db, task)?);
        receipt["request"] = state["id"].clone();
    }
    let approval = &receipts[1];
    put(
        db,
        "publication_release",
        attempt,
        &json!({"id":approval["id"],"confirmation":state["id"],
            "actor":approval["actor"],"confirmed":accepted,"task":task,"authority":state["authority"],
            "document":doc,"workspace":approval["workspace"],"attempt":attempt,"report":delivery["publication_review"]}),
    )?;
    Ok(receipts)
}
