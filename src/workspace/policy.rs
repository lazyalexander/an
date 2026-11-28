use crate::store::{get, put, uid};
use anyhow::{anyhow, ensure, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

pub(super) fn apply(tx: &Connection, id: String, rules: Option<Value>) -> Result<Value> {
    let mut cfg = get(tx, "workspace", &id)?;
    if let Some(rules) = rules {
        let images = cfg["images"]
            .as_array()
            .ok_or_else(|| anyhow!("Invalid workspace"))?;
        for row in rules
            .as_array()
            .ok_or_else(|| anyhow!("Expected flow list"))?
        {
            let row = row.as_array().ok_or_else(|| anyhow!("Invalid flow"))?;
            ensure!(row.len() == 3, "Invalid flow");
            let (s, d) = (&row[1], &row[2]);
            let valid = match row[0].as_str() {
                Some("input") => s.is_string() && images.contains(d),
                Some("inspect_block") => {
                    let target = d
                        .as_str()
                        .ok_or_else(|| anyhow!("Expected recipient task"))?;
                    crate::provenance::valid_endpoint(s)
                        && get(tx, "task", target)?["workspace"] == id
                }
                Some("submit_review") => crate::provenance::valid_endpoint(s) && d == &id,
                Some("block_input") => crate::provenance::valid_endpoint(s) && images.contains(d),
                Some("bridge_input") => {
                    s.as_str().is_some_and(|k| cfg["tools"].get(k).is_some()) && images.contains(d)
                }
                Some("bridge_send") => {
                    images.contains(s) && d.as_str().is_some_and(|k| cfg["tools"].get(k).is_some())
                }
                Some("model") => {
                    graph_origin(images, s) && d.as_str().is_some_and(|v| v.starts_with("model:"))
                }
                Some("handoff") => {
                    (images.contains(s) || s.as_str().is_some_and(|v| v.starts_with("model:")))
                        && images.contains(d)
                }
                Some("preview") => graph_origin(images, s) && (d == "studio" || d == "provenance"),
                Some("publish") => graph_origin(images, s) && (d == &id || d == "graph-metadata"),
                Some("export") => {
                    graph_origin(images, s) && d.as_str().is_some_and(|v| v.starts_with('/'))
                }
                _ => false,
            };
            ensure!(valid, "Invalid workspace flow");
        }
        cfg["policy"] = json!({"revision":uid(),"flows":rules});
        put(tx, "workspace", &id, &cfg)?;
    }
    Ok(cfg
        .get("policy")
        .cloned()
        .unwrap_or_else(|| json!({"revision":"legacy"})))
}
fn graph_origin(images: &[Value], source: &Value) -> bool {
    images.contains(source)
        || source == "response"
        || source
            .as_str()
            .is_some_and(|s| s.starts_with("task-input:") || s.starts_with("model:"))
}
