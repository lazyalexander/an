use super::{resolve_grants, Grants, Selector};
use crate::store::{find, put, uid};
use anyhow::{ensure, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
pub(super) fn create(
    tx: &Connection,
    grants: Grants,
    primary: Selector,
    review_owner_digest: Option<String>,
) -> Result<Value> {
    ensure!(!grants.spool.is_empty(), "Grant a spool");
    let (images, tools) = resolve_grants(tx, &grants)?;
    let id = uid();
    put(
        tx,
        "workspace",
        &id,
        &json!({"images":images,"editable":grants.editable,"tools":tools}),
    )?;
    if let Some(identity) = primary.identity()? {
        ensure!(
            find(tx, "binding", &identity)?.is_none(),
            "Primary identity already bound"
        );
        put(
            tx,
            "binding",
            &identity,
            &json!({"workspace":id,"task":null}),
        )?;
        put(tx, "workspace_primary", &id, &json!(identity))?;
    }
    if let Some(digest) = review_owner_digest {
        ensure!(
            digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit()),
            "Expected SHA-256 digest"
        );
        put(tx, "review_owner", &id, &json!(digest.to_ascii_lowercase()))?;
    }

    Ok(json!({"workspace":id}))
}
