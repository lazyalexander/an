//! Only explicitly handed-in blocks enter the primary agent's review inbox.
use crate::{
    permission,
    provenance::{Actor, BlockRef, Right},
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

pub(crate) fn is_primary(db: &Connection, task: &str) -> Result<bool> {
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

pub(crate) fn authorize(
    db: &Connection,
    actor: &Actor,
    source: &BlockRef,
    request: Option<&str>,
) -> Result<Option<Value>> {
    let ws = workspace(db, &actor.task)?;
    get(db, "workspace_primary", &ws).context("Workspace has no primary inbox")?;
    crate::provenance::block_registration(db, &actor.task, source)?
        .context("Review block unavailable in source task")?;
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
pub(crate) struct Submission {
    pub idempotency_key: String,
    pub source: BlockRef,
}

#[derive(serde::Serialize)]
struct Receipt<'a> {
    id: &'a str,
    workspace: &'a str,
    source: &'a BlockRef,
    task: &'a str,
    registration_cursor: u64,
    attempt: Option<String>,
    sender: &'a str,
    grant: &'a str,
    recipient: &'static str,
    purpose: &'static str,
    status: &'static str,
}

pub(crate) fn submit(db: &Connection, actor: &Actor, source: &BlockRef) -> Result<Value> {
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
    let (cursor, attempt) = crate::provenance::block_registration(db, &actor.task, source)?
        .context("Review block unavailable in source task")?;
    let id = uid();
    let receipt = Receipt {
        id: &id,
        workspace: &ws,
        source,
        task: &actor.task,
        registration_cursor: cursor,
        attempt,
        sender: &actor.id,
        grant,
        recipient: "primary",
        purpose: "review",
        status: "pending",
    };
    let receipt = serde_json::to_value(receipt)?;
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

pub(crate) fn list(db: &Connection, actor: &Actor) -> Result<Value> {
    actor.require(db, Right::ReadInbox)?;
    ensure!(is_primary(db, &actor.task)?, "Primary inbox outside task");
    let mut stmt = db.prepare(include_str!("inbox.sql"))?;
    let mut rows = stmt.query([workspace(db, &actor.task)?])?;
    let mut items = vec![];
    while let Some(row) = rows.next()? {
        let receipt: Value = serde_json::from_str(&row.get::<_, String>(0)?)?;
        items.push(json!({"receipt":receipt,"text":row.get::<_,String>(1)?}));
    }
    Ok(json!({"items":items}))
}
