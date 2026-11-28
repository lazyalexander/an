use super::string;
use crate::store::get;
use anyhow::{ensure, Context, Result};
use rusqlite::Connection;
use serde_json::Value;

pub(super) fn bridge_artifact(
    db: &Connection,
    task: &str,
    t: &Value,
    source: &str,
) -> Result<Value> {
    let item = get(db, "artifact", source)?;
    let origin = string(&item, "tool")?;
    ensure!(
        item["task"] == task
            && t["tools"][origin] == item["server_revision"]
            && get(db, "server", string(&item, "server")?)?["revision"] == item["server_revision"],
        "Artifact outside task or bridge grant"
    );
    Ok(item)
}

pub(super) fn input_bindings<'a>(
    args: &'a Value,
    manifest: &Value,
) -> Result<Vec<(Option<&'a str>, &'a str)>> {
    let named = args.get("inputs").filter(|v| !v.is_null());
    ensure!(
        named.is_none() || args["input"].is_null(),
        "Choose input or inputs"
    );
    ensure!(
        named.is_none() || args["request_id"].is_null(),
        "Use input_requests for named ports"
    );
    if !args["input_requests"].is_null() {
        let requests = args["input_requests"]
            .as_object()
            .context("Expected input request map")?;
        let ports = named
            .and_then(Value::as_object)
            .context("input_requests requires inputs")?;
        ensure!(
            requests
                .iter()
                .all(|(name, id)| ports.contains_key(name) && id.is_string()),
            "Invalid input request binding"
        );
    }
    let mut bindings = Vec::new();
    if let Some(value) = named {
        let ports = value.as_object().context("Expected named inputs")?;
        let declared = manifest["inputs"]
            .as_array()
            .context("Invalid admitted manifest")?;
        for port in declared {
            let name = string(port, "name")?;
            ensure!(
                port["required"] != true || ports.contains_key(name),
                "Missing required input: {name}"
            );
        }
        for (name, source) in ports {
            ensure!(
                declared.iter().any(|p| p["name"] == name.as_str()),
                "Undeclared input: {name}"
            );
            bindings.push((
                Some(name.as_str()),
                source.as_str().context("Expected source reference")?,
            ));
        }
    } else if let Some(source) = args["input"].as_str() {
        bindings.push((None, source));
    }
    Ok(bindings)
}
