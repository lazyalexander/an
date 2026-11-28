//! A frozen projection of one graph position and one selected text block.
use super::{
    access::{Actor, Right},
    domain::BlockRef,
};
use crate::{
    permission,
    store::{canonical, find, get, hash, put},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    cursor: u64,
    node: String,
    source: BlockRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lookup {
    snapshot: String,
}

pub(super) fn create(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let Selection {
        cursor,
        node,
        source,
    } = serde_json::from_value(args.clone())?;
    let workspace = get(db, "task", &actor.task)?["workspace"].clone();
    ensure!(
        workspace.is_string(),
        "Execution snapshot requires a workspace"
    );
    let revision: u64 = db
        .query_row(
            "SELECT revision FROM pv_point WHERE task=? AND cursor=?",
            params![actor.task, cursor],
            |r| r.get(0),
        )
        .optional()?
        .context("Unknown graph position")?;
    let (attempt, status): (String, String) = db
        .query_row(
            "SELECT json_extract(body,'$.activity.id'),json_extract(body,'$.activity.status')
         FROM pv_node_history WHERE task=? AND node=? AND cursor<=? ORDER BY cursor DESC LIMIT 1",
            params![actor.task, node, cursor],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .context("Node unavailable at graph position")?;
    let text: String = db
        .query_row(
            "SELECT b.text FROM pv_frozen_text b JOIN pv_version v USING(artifact,version)
         WHERE v.task=? AND v.registered_cursor<=? AND artifact=? AND version=? AND block=?",
            params![
                actor.task,
                cursor,
                source.artifact.0,
                source.version.0,
                source.block
            ],
            |r| r.get(0),
        )
        .optional()?
        .context("Block unavailable at graph position")?;
    let view = json!({"workspace":workspace,"task":actor.task,"cursor":cursor,"revision":revision,
        "node":node,"attempt":attempt,"status":status,"source":source,"text":text});
    let id = hash(canonical(&view).as_bytes());
    if find(db, "execution_snapshot", &id)?.is_none() {
        put(db, "execution_snapshot", &id, &view)?;
    }
    Ok(json!({"snapshot":id}))
}

pub(super) fn authorize(
    db: &Connection,
    actor: &Actor,
    id: &str,
    target: &str,
    request: Option<&str>,
) -> Result<Option<Value>> {
    let view = get(db, "execution_snapshot", id)?;
    let route = json!(["inspect_block", view["source"], target]);
    permission::gate(
        db,
        &actor.task,
        "inspect_block",
        json!([id, target]),
        json!([view["node"], view["cursor"]]),
        request,
        Some(route),
    )
}

pub(super) fn read(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    let Lookup { snapshot } = serde_json::from_value(args.clone())?;
    actor.require(
        db,
        Right::InspectSnapshot {
            snapshot: snapshot.clone(),
        },
    )?;
    get(db, "execution_snapshot", &snapshot)
}
