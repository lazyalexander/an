use super::domain::{ArtifactId, VersionId};
use crate::permission;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    artifact: ArtifactId,
    version: VersionId,
}

pub fn validate(db: &Connection, task: &str, node: &Value) -> Result<()> {
    if let Some(selection) = node.get("versions") {
        permission::controller(db, task)?;
        let selections: BTreeMap<String, Selection> = serde_json::from_value(selection.clone())?;
        ensure!(
            node["kind"] == "delivery" && !selections.is_empty(),
            "Expected delivery version selections"
        );
        for (name, s) in selections {
            let binding = node["inputs"]
                .get(&name)
                .context("Unknown delivery binding")?;
            ensure!(db.query_row("SELECT EXISTS(SELECT 1 FROM pv_version WHERE artifact=? AND version=? AND task=?
                AND json_extract(material,'$.node')=? AND json_extract(material,'$.port')=?)",
                params![s.artifact.0,s.version.0,task,binding["node"].as_str(),binding["port"].as_str()],|r|r.get::<_,bool>(0))?,
                "Version does not describe this output");
        }
    }
    Ok(())
}

pub fn publish(db: &Connection, task: &str, node: &Value, delivery: &Value) -> Result<()> {
    let Some(selections) = node.get("versions") else {
        return Ok(());
    };
    let selections: BTreeMap<String, Selection> = serde_json::from_value(selections.clone())?;
    for (name, s) in selections {
        let bound = &delivery["bindings"][&name];
        let accepted=db.execute("INSERT INTO pv_published SELECT ?,?,artifact,version,? FROM pv_version
            WHERE artifact=? AND version=? AND task=? AND json_extract(material,'$.attempt')=? AND json_extract(material,'$.revision')=?",
            params![delivery["id"].as_str(),name,delivery["workspace"].as_str(),s.artifact.0,s.version.0,task,bound["run"].as_str().or(bound["attempt"].as_str()),bound["revision"].as_str()])?;
        ensure!(
            accepted == 1,
            "Bound attempt or revision changed before publication"
        );
    }
    Ok(())
}
