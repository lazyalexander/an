use super::{config, task::new_task};
use crate::store::{canonical, find, get, hash, put, root};
use anyhow::{anyhow, ensure, Result};
use clap::Args;
use rusqlite::{Connection, Transaction};
use serde_json::{json, Value};
use std::fs;
#[derive(Args)]
pub struct Selector {
    #[arg(long)]
    pub harness: Option<String>,
    #[arg(long, conflicts_with = "key")]
    pub session: Option<String>,
    #[arg(long)]
    pub key: Option<String>,
}
impl Selector {
    pub fn identity(&self) -> Result<Option<String>> {
        if self.harness.is_none() && self.session.is_none() && self.key.is_none() {
            return Ok(None);
        }
        let h = self
            .harness
            .as_deref()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| anyhow!("Expected harness"))?;
        let (kind, id) = self
            .session
            .as_ref()
            .map(|s| ("session", s))
            .or_else(|| self.key.as_ref().map(|s| ("key", s)))
            .ok_or_else(|| anyhow!("Expected session or key"))?;
        ensure!(!id.is_empty(), "Empty identity");
        Ok(Some(canonical(&json!([h, kind, id]))))
    }
    fn args(&self) -> Value {
        if let Some(s) = &self.session {
            json!(["mcp", "--harness", self.harness, "--session", s])
        } else {
            json!(["mcp", "--harness", self.harness, "--key", self.key])
        }
    }
}
pub fn guard(identity: &str, rotate: bool) -> Result<fs::File> {
    let dir = root()?.join("locks");
    fs::create_dir_all(&dir)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("binding-{}", hash(identity.as_bytes()))))?;
    if rotate {
        file.try_lock()?;
    } else {
        file.try_lock_shared()?;
    }
    Ok(file)
}
pub fn resolve(db: &Transaction, identity: &str, fresh: bool) -> Result<String> {
    let mut b = get(db, "binding", identity)?;
    if let Some(old) = b["task"].as_str() {
        if !fresh {
            return Ok(old.into());
        }
        let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM objects WHERE kind='instance' AND json_extract(body,'$.task')=? AND json_extract(body,'$.status')!='unloaded')",[old],|r|r.get(0))?;
        ensure!(!active, "Unload instances first");
        put(db, "turn", old, &Value::Null)?;
    }
    let cfg = get(
        db,
        "workspace",
        b["workspace"]
            .as_str()
            .ok_or_else(|| anyhow!("Invalid binding"))?,
    )?;
    let (id, mut t) = new_task(
        db,
        &serde_json::from_value::<Vec<String>>(cfg["images"].clone())?,
        &serde_json::from_value::<Vec<String>>(cfg["editable"].clone())?,
        json!({}),
    )?;
    t["workspace"] = b["workspace"].clone();
    t["tools"] = cfg["tools"].clone();
    b["task"] = json!(id);
    put(db, "task", &id, &t)?;
    put(db, "binding", identity, &b)?;
    Ok(id)
}
pub(super) fn bind(tx: &Connection, workspace: String, selector: Selector) -> Result<Value> {
    let key = selector
        .identity()?
        .ok_or_else(|| anyhow!("Expected bound identity"))?;
    get(tx, "workspace", &workspace)?;
    if let Some(b) = find(tx, "binding", &key)? {
        ensure!(
            b["workspace"] == workspace,
            "Identity already bound to another workspace"
        );
    } else {
        put(
            tx,
            "binding",
            &key,
            &json!({"workspace":workspace,"task":null}),
        )?;
    }

    Ok(json!({"workspace":workspace,"mcp_servers":config(selector.args())?}))
}
