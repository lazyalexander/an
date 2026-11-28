//! Collect immutable file sets after task closure. Metadata and audit history
//! keep their IDs; only revision payloads with no remaining consumer are swept.
use super::*;
use anyhow::Context;
use rusqlite::TransactionBehavior;
use std::collections::{BTreeMap, BTreeSet};

/// Never retire a task while its instance cleanup or model send is outstanding.
fn settled_tasks(db: &Connection) -> Result<BTreeSet<String>> {
    let mut stmt = db.prepare(
        "SELECT id FROM objects WHERE kind='task' AND json_extract(body,'$.closed')=1
         EXCEPT SELECT json_extract(body,'$.task') FROM objects WHERE kind='instance'
             AND json_extract(body,'$.status') IS NOT 'unloaded'
         EXCEPT SELECT json_extract(body,'$.task') FROM objects WHERE kind='run'
             AND json_extract(body,'$.status') IN ('running','unknown','cancel_pending')
         EXCEPT SELECT json_extract(body,'$.task') FROM objects WHERE kind='model_call'
             AND json_extract(body,'$.state') IN ('reserved','sending')",
    )?;
    let tasks = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(tasks)
}

/// Match against actual registry IDs, including IDs from older registries.
/// New record types are retained conservatively until their ownership is known.
fn retain(value: &Value, unused: &mut BTreeMap<String, u64>) {
    match value {
        Value::String(id) => {
            unused.remove(id);
        }
        Value::Array(items) => items.iter().for_each(|v| retain(v, unused)),
        Value::Object(fields) => {
            for (key, value) in fields {
                unused.remove(key);
                retain(value, unused);
            }
        }
        _ => {}
    }
}

fn mark_published_graph(
    db: &Connection,
    task: &str,
    graph: &Value,
    unused: &mut BTreeMap<String, u64>,
) -> Result<()> {
    let mut stmt = db.prepare(
        "SELECT json_extract(body,'$.node') FROM objects
         WHERE kind='graph_delivery' AND json_extract(body,'$.task')=?",
    )?;
    let roots = stmt
        .query_map([task], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    let lineage = crate::graph::lineage(graph, roots);
    for node in &lineage {
        retain(&graph["nodes"][node], unused);
        let activity = &graph["activity"][node];
        retain(activity, unused);
        // Evidence can point at a run holding a combined candidate or editable
        // work snapshot that is not repeated in the per-port graph projection.
        if let Some(run) = activity["evidence"]["run"].as_str() {
            if let Some(record) = find(db, "run", run)? {
                retain(&record, unused);
            }
        }
    }
    // Sources also exposes the events of earlier attempts on these nodes.
    for event in graph["events"].as_array().into_iter().flatten() {
        let names_node = event["node"]
            .as_str()
            .is_some_and(|id| lineage.contains(id));
        let names_group = event["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|id| lineage.contains(id));
        if names_node || names_group {
            retain(event, unused);
        }
    }
    Ok(())
}

fn mark_roots(
    db: &Connection,
    settled: &BTreeSet<String>,
    unused: &mut BTreeMap<String, u64>,
) -> Result<()> {
    let managed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='pv_version')",
        [],
        |r| r.get(0),
    )?;
    if managed {
        let mut versions =
            db.prepare("SELECT material FROM pv_version WHERE material IS NOT NULL")?;
        for material in versions.query_map([], |r| r.get::<_, String>(0))? {
            retain(&serde_json::from_str(&material?)?, unused);
        }
    }
    let mut stmt = db.prepare("SELECT kind,id,body FROM objects WHERE kind != 'revision'")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let id: String = row.get(1)?;
        let body: Value = serde_json::from_str(&row.get::<_, String>(2)?)?;
        let retired = body["task"].as_str().is_some_and(|t| settled.contains(t));
        match kind.as_str() {
            "provenance_material" if retired => {}
            "frozen_evidence" if managed && retired => {
                let linked:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM pv_version,json_each(evidence) e WHERE e.value=?)
                    OR EXISTS(SELECT 1 FROM objects g JOIN json_each(g.body,'$.rights') r
                    JOIN objects t ON t.kind='turn' AND t.id=json_extract(g.body,'$.target') AND t.body=json_quote(json_extract(g.body,'$.epoch'))
                    WHERE g.kind='provenance_grant' AND json_extract(g.body,'$.active') AND json_extract(r.value,'$.snapshot')=?)",
                    params![id,id],|r|r.get(0))?;
                if linked {
                    retain(&body, unused);
                }
            }
            "graph" if settled.contains(&id) => mark_published_graph(db, &id, &body, unused)?,
            "task" | "intent" if settled.contains(&id) => {}
            "run" if retired && body["status"] != "delivered" => {}
            "request" | "effect" | "model_call" | "call" | "artifact" | "handoff"
            | "intent_history"
                if retired => {}
            // This binding compares a hash when authorizing a flow. It never opens a file.
            "intent_version" => {}
            _ => retain(&body, unused),
        }
    }
    Ok(())
}

pub(crate) fn after_finish(db: &mut Connection, task: &str) -> Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if find(&tx, "revision_gc", task)?.is_some() {
        return Ok(());
    }
    let settled = settled_tasks(&tx)?;
    if !settled.contains(task) {
        return Ok(());
    }
    let mut unused = BTreeMap::new();
    let mut before = 0;
    {
        let mut sizes =
            tx.prepare("SELECT id,octet_length(body) FROM objects WHERE kind='revision'")?;
        let mut rows = sizes.query([])?;
        while let Some(row) = rows.next()? {
            let size: u64 = row.get(1)?;
            before += size;
            unused.insert(row.get(0)?, size);
        }
    }
    mark_roots(&tx, &settled, &mut unused)?;
    let freed: u64 = unused.values().sum();
    let mut delete = tx.prepare("DELETE FROM objects WHERE kind='revision' AND id=?")?;
    for id in unused.keys() {
        delete.execute([id])?;
    }
    drop(delete);
    put(
        &tx,
        "revision_gc",
        task,
        &json!({
            "revisions_removed":unused.len(), "payload_bytes_before":before,
            "payload_bytes_after":before-freed, "payload_bytes_reclaimed":freed
        }),
    )?;
    tx.commit().context("Committing revision collection")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn another_tasks_atomic_publication_survives_collection() -> Result<()> {
        let path = std::env::temp_dir().join(crate::store::uid());
        let mut writer = Connection::open(&path)?;
        writer.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
        put(&writer, "task", "finished", &json!({"closed":true}))?;
        let mut collector = Connection::open(&path)?;
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let shared = revision(&tx, &json!({"kept":"YQ=="}))?;
        let abandoned = revision(&tx, &json!({"scratch":"Yg=="}))?;
        let sweep = std::thread::spawn(move || after_finish(&mut collector, "finished"));
        put(&tx, "task", "active", &json!({"inputs":{"keep":shared}}))?;
        tx.commit()?;
        sweep.join().unwrap()?;
        assert!(find(&writer, "revision", &shared)?.is_some());
        assert!(find(&writer, "revision", &abandoned)?.is_none());
        drop(writer);
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn unfinished_send_and_interrupted_sweep_can_both_be_retried() -> Result<()> {
        let mut db = Connection::open_in_memory()?;
        db.execute_batch("CREATE TABLE objects(kind,id,body,PRIMARY KEY(kind,id))")?;
        put(&db, "task", "t", &json!({"closed":true}))?;
        put(
            &db,
            "model_call",
            "call",
            &json!({"task":"t","state":"sending","input":"scratch"}),
        )?;
        put(&db, "revision", "scratch", &json!({"data":"eA=="}))?;
        after_finish(&mut db, "t")?;
        assert!(find(&db, "revision_gc", "t")?.is_none());
        assert!(find(&db, "revision", "scratch")?.is_some());

        put(
            &db,
            "model_call",
            "call",
            &json!({"task":"t","state":"unknown","input":"scratch"}),
        )?;
        db.execute_batch(
            "CREATE TRIGGER fail_receipt BEFORE INSERT ON objects
            WHEN NEW.kind='revision_gc' BEGIN SELECT RAISE(ABORT,'injected failure'); END",
        )?;
        assert!(after_finish(&mut db, "t").is_err());
        assert!(find(&db, "revision", "scratch")?.is_some());
        db.execute_batch("DROP TRIGGER fail_receipt")?;
        after_finish(&mut db, "t")?;
        assert!(find(&db, "revision", "scratch")?.is_none());
        assert_eq!(get(&db, "revision_gc", "t")?["revisions_removed"], 1);
        Ok(())
    }
}
