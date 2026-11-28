//! Only explicitly handed-in blocks enter the primary agent's review inbox.
use super::{
    access::{Actor, Right},
    domain::BlockRef,
};
use crate::{
    permission,
    store::{find, get, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};

fn workspace(db: &Connection, task: &str) -> Result<String> {
    Ok(get(db, "task", task)?["workspace"]
        .as_str()
        .context("Task requires a workspace")?
        .to_owned())
}

pub(super) fn is_primary(db: &Connection, task: &str) -> Result<bool> {
    let ws = workspace(db, task)?;
    let Some(identity) = find(db, "workspace_primary", &ws)? else {
        return Ok(false);
    };
    let binding = get(
        db,
        "binding",
        identity.as_str().context("Invalid primary identity")?,
    )?;
    Ok(binding["workspace"] == ws && binding["task"] == task)
}

pub(super) fn authorize(
    db: &Connection,
    actor: &Actor,
    source: &BlockRef,
    request: Option<&str>,
) -> Result<Option<Value>> {
    let ws = workspace(db, &actor.task)?;
    get(db, "workspace_primary", &ws).context("Workspace has no primary inbox")?;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pv_block b JOIN pv_version v USING(artifact,version)
        WHERE b.artifact=? AND b.version=? AND b.block=? AND v.task=?)",
        params![
            source.artifact.0,
            source.version.0,
            source.block,
            actor.task
        ],
        |r| r.get(0),
    )?;
    ensure!(exists, "Review block unavailable in source task");
    permission::gate(
        db,
        &actor.task,
        "submit_review",
        json!([source, ws]),
        json!(["review"]),
        request,
        Some(json!(["submit_review", source, ws])),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Submission {
    pub idempotency_key: String,
    pub source: BlockRef,
}

pub(super) fn submit(db: &Connection, actor: &Actor, source: &BlockRef) -> Result<Value> {
    let grant = actor
        .grant
        .as_ref()
        .context("Review hand-in requires an approved delegation")?;
    actor.require(
        db,
        Right::SubmitReview {
            source: source.clone(),
        },
    )?;
    let ws = workspace(db, &actor.task)?;
    let (cursor, attempt): (u64, Option<String>) = db.query_row(
        "SELECT registered_cursor,json_extract(material,'$.attempt') FROM pv_version
        WHERE artifact=? AND version=? AND task=?",
        params![source.artifact.0, source.version.0, actor.task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let id = uid();
    let receipt = json!({
        "id": id,
        "workspace": ws,
        "source": source,
        "task": actor.task,
        "registration_cursor": cursor,
        "attempt": attempt,
        "sender": actor.id,
        "grant": grant,
        "recipient": "primary",
        "purpose": "review",
        "status": "pending",
    });
    db.execute(
        "INSERT INTO pv_inbox(workspace,receipt,artifact,version,block,metadata)
         VALUES(?,?,?,?,?,?)",
        params![
            ws,
            id,
            source.artifact.0,
            source.version.0,
            source.block,
            receipt.to_string()
        ],
    )?;
    Ok(receipt)
}

pub(super) fn list(db: &Connection, actor: &Actor) -> Result<Value> {
    actor.require(db, Right::ReadInbox)?;
    ensure!(is_primary(db, &actor.task)?, "Primary inbox outside task");
    let mut stmt = db.prepare(
        "SELECT i.metadata,json_extract(b.body,'$.text') FROM pv_inbox i
        JOIN pv_block b USING(artifact,version,block) WHERE i.workspace=? ORDER BY i.seq",
    )?;
    let mut rows = stmt.query([workspace(db, &actor.task)?])?;
    let mut items = vec![];
    while let Some(row) = rows.next()? {
        let receipt: Value = serde_json::from_str(&row.get::<_, String>(0)?)?;
        items.push(json!({"receipt":receipt,"text":row.get::<_,String>(1)?}));
    }
    Ok(json!({"items":items}))
}
