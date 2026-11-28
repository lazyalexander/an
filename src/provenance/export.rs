//! Persist an intent before writing any destination bytes. A process exit at
//! any point before confirmation leaves an explicit unknown outcome.
use crate::{
    artifacts, permission,
    store::{canonical, connect, find, get, hash, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

struct Delivery {
    task: String,
    revision: String,
    origins: BTreeSet<String>,
    details: Value,
}

impl Delivery {
    fn resolve(tx: &Transaction, id: &str, task: Option<&str>) -> Result<Self> {
        if let Some(group) = find(tx, "graph_delivery", id)? {
            let owner = group["task"].as_str().context("Invalid delivery")?;
            let task = task.unwrap_or(owner);
            ensure!(
                owner == task
                    || (group["workspace"].is_string()
                        && get(tx, "task", task)?["workspace"] == group["workspace"]),
                "Delivery outside workspace"
            );
            let origins: BTreeSet<_> = group["bindings"]
                .as_object()
                .context("Missing delivery bindings")?
                .values()
                .map(crate::graph::binding_origin)
                .collect();
            ensure!(!origins.is_empty(), "Missing delivery origins");
            return Ok(Self {
                task: task.into(),
                revision: group["revision"]
                    .as_str()
                    .context("Missing revision")?
                    .into(),
                origins,
                details: json!({"bindings":group["bindings"]}),
            });
        }
        let r = get(tx, "run", id)?;
        let owner = r["task"].as_str().context("Invalid run")?;
        ensure!(
            r["status"] == "delivered" && task.is_none_or(|t| t == owner),
            "Expected published task delivery"
        );
        Ok(Self {
            task: owner.into(),
            revision: r["output"].as_str().context("Missing revision")?.into(),
            origins: BTreeSet::from([r["spool"].as_str().context("Missing spool")?.into()]),
            details: json!({}),
        })
    }
}

pub fn run(
    id: &str,
    path: &Path,
    task: Option<&str>,
    request: Option<&str>,
    retry: bool,
) -> Result<Value> {
    let mut db = connect()?;
    let dest = std::env::current_dir()?.join(path);
    let dest = if dest.exists() {
        dest.canonicalize()?
    } else {
        dest.parent()
            .context("Invalid export path")?
            .canonicalize()?
            .join(dest.file_name().context("Missing export directory")?)
    };
    // Serialize even exports of the same delivery from different resumed tasks.
    let operation = hash(canonical(&json!([id, dest])).as_bytes());
    let lock = std::fs::File::options()
        .create(true)
        .append(true)
        .open(crate::store::root()?.join(format!("export-{operation}.lock")))?;
    lock.lock()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let group = Delivery::resolve(&tx, id, task)?;
    permission::current(&tx, &group.task)?;
    let prior: Option<String> = tx
        .query_row(
            "SELECT result FROM pv_export WHERE operation=? ORDER BY seq DESC LIMIT 1",
            [&operation],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(prior) = prior {
        let result: Value = serde_json::from_str(&prior)?;
        // Check the caller's authority for this exact destination even when
        // returning an existing receipt in another turn.
        if !retry || result["status"] == "exported" {
            if let Some(p) = group.authorize(&tx, id, &dest, request)? {
                tx.commit()?;
                return Ok(p);
            }
            tx.commit()?;
            return Ok(result);
        }
    }
    if let Some(p) = group.authorize(&tx, id, &dest, request)? {
        tx.commit()?;
        return Ok(p);
    }
    ensure!(!dest.try_exists()?, "Export destination already exists");
    let files = get(&tx, "revision", &group.revision)?;
    let attempt = uid();
    let mut result = json!({"attempt":attempt,"status":"unknown","revision":group.revision,"delivery":id,"destination":dest});
    tx.execute(
        "INSERT INTO pv_export(operation,attempt,result) VALUES(?,?,?)",
        params![operation, attempt, result.to_string()],
    )?;
    export_event(&tx, &group, &result)?;
    tx.commit()?;
    artifacts::stage(&dest, &files, false)
        .context("Export outcome unknown; inspect destination before explicit retry")?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    result["status"] = json!("exported");
    tx.execute(
        "UPDATE pv_export SET result=? WHERE attempt=?",
        params![result.to_string(), attempt],
    )?;
    export_event(&tx, &group, &result)?;
    tx.commit()?;
    Ok(result)
}

impl Delivery {
    fn authorize(
        &self,
        tx: &Transaction,
        id: &str,
        dest: &Path,
        request: Option<&str>,
    ) -> Result<Option<Value>> {
        let flows = self
            .origins
            .iter()
            .map(|origin| json!(["export", origin, dest]))
            .collect::<Vec<_>>();
        permission::gate_flows(
            tx,
            &self.task,
            "export",
            json!([id, self.revision, dest]),
            self.details.clone(),
            request,
            &flows,
        )
    }
}

fn export_event(tx: &Transaction, group: &Delivery, result: &Value) -> Result<()> {
    tx.execute(
        "INSERT INTO pv_flow(task,revision,phase,attempt,target,delivery,time_ms)
        VALUES(?,?,?,?,?,?,unixepoch('subsec')*1000)",
        params![
            group.task,
            group.revision,
            result["status"].as_str(),
            result["attempt"].as_str(),
            result["destination"].as_str(),
            result["delivery"].as_str()
        ],
    )?;
    Ok(())
}
