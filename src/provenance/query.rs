//! Follow exact block endpoints. An unreadable neighbor is never traversed.
use super::{
    access::{Actor, Right},
    domain::{ArtifactId, BlockRef, VersionId},
};
use crate::store::get;
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Copy)]
pub enum Direction {
    Sources,
    Impact,
}

impl Direction {
    fn query(self) -> &'static str {
        match self {
            Self::Sources => include_str!("sources.sql"),
            Self::Impact => include_str!("impact.sql"),
        }
    }
}

fn can_read(db: &Connection, actor: &Actor, endpoint: &BlockRef) -> Result<bool> {
    actor.allows(
        db,
        &Right::Read {
            artifact: endpoint.artifact.clone(),
            version: endpoint.version,
        },
    )
}

pub fn trace(db: &Connection, actor: &Actor, args: &Value, direction: Direction) -> Result<Value> {
    let first = seeds(db, actor, args)?;
    if first.is_empty() && args["delivery"].is_string() {
        return Ok(
            json!({"precision":"file","delivery":args["delivery"],"bindings":published_files(db,actor,args)?}),
        );
    }
    ensure!(!first.is_empty(), "Block unavailable");
    let mut pending: VecDeque<_> = first.into();
    let mut visited = BTreeSet::new();
    let mut blocks = Vec::new();
    let mut edges = Vec::new();
    let mut neighbors = db.prepare(direction.query())?;
    while let Some(endpoint) = pending.pop_front() {
        if !visited.insert(endpoint.clone()) {
            continue;
        }
        ensure!(can_read(db, actor, &endpoint)?, "Block unavailable");
        blocks.push(describe(db, actor, &endpoint)?);
        let mut rows = neighbors.query(params![
            endpoint.artifact.0,
            endpoint.version.0,
            endpoint.block
        ])?;
        while let Some(row) = rows.next()? {
            let neighbor = BlockRef {
                artifact: ArtifactId(row.get(0)?),
                version: VersionId(row.get(1)?),
                block: row.get(2)?,
            };
            if can_read(db, actor, &neighbor)? {
                let (consumer, source) = match direction {
                    Direction::Sources => (&endpoint, &neighbor),
                    Direction::Impact => (&neighbor, &endpoint),
                };
                edges.push(
                    json!({"consumer":consumer,"source":source,"relation":row.get::<_,String>(3)?}),
                );
                pending.push_back(neighbor);
            }
        }
    }
    Ok(json!({"precision":"block","blocks":blocks,"edges":edges}))
}

fn published_files(db: &Connection, actor: &Actor, args: &Value) -> Result<Value> {
    actor.controller()?;
    let d = get(
        db,
        "graph_delivery",
        args["delivery"].as_str().context("Expected delivery")?,
    )?;
    ensure!(
        d["task"] == actor.task
            || (d["workspace"].is_string()
                && get(db, "task", &actor.task)?["workspace"] == d["workspace"]),
        "Delivery unavailable"
    );
    Ok(d["bindings"].clone())
}

fn seeds(db: &Connection, actor: &Actor, args: &Value) -> Result<Vec<BlockRef>> {
    if args["delivery"].is_string() {
        let bindings = published_files(db, actor, args)?;
        let name = args["binding"]
            .as_str()
            .context("Expected delivery binding")?;
        ensure!(bindings.get(name).is_some(), "Unknown binding");
        let mut stmt = db.prepare(
            "SELECT artifact,version,block FROM pv_published JOIN pv_block USING(artifact,version)
            WHERE delivery=? AND binding=? AND (?3 IS NULL OR block=?3) ORDER BY block",
        )?;
        let mut rows = stmt.query(params![
            args["delivery"].as_str(),
            name,
            args["block"].as_str()
        ])?;
        let mut refs = vec![];
        while let Some(row) = rows.next()? {
            let endpoint = BlockRef {
                artifact: ArtifactId(row.get(0)?),
                version: VersionId(row.get(1)?),
                block: row.get(2)?,
            };
            if can_read(db, actor, &endpoint)? {
                refs.push(endpoint);
            }
        }
        if refs.is_empty() && args.get("block").is_some() {
            let managed: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM pv_published WHERE delivery=? AND binding=?)",
                params![args["delivery"].as_str(), name],
                |r| r.get(0),
            )?;
            ensure!(!managed, "Block unavailable");
        }
        return Ok(refs);
    }
    let artifact = ArtifactId(
        args["artifact"]
            .as_str()
            .context("Expected artifact")?
            .into(),
    );
    let version: VersionId = serde_json::from_value(args["version"].clone())?;
    actor.require(
        db,
        Right::Read {
            artifact: artifact.clone(),
            version,
        },
    )?;
    let mut stmt=db.prepare("SELECT block FROM pv_block WHERE artifact=? AND version=? AND (?3 IS NULL OR block=?3) ORDER BY rowid")?;
    let rows = stmt.query_map(
        params![artifact.0, version.0, args["block"].as_str()],
        |r| {
            Ok(BlockRef {
                artifact: artifact.clone(),
                version,
                block: r.get(0)?,
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

struct RecordedBlock {
    task: String,
    author: String,
    participant: Option<String>,
    material: Option<String>,
    evidence: String,
    content: String,
}

fn describe(db: &Connection, actor: &Actor, endpoint: &BlockRef) -> Result<Value> {
    let row = db.query_row(
        "SELECT task,author,participant,material,evidence,body
         FROM pv_version JOIN pv_block USING(artifact,version)
         WHERE artifact=? AND version=? AND block=?",
        params![endpoint.artifact.0, endpoint.version.0, endpoint.block],
        |r| {
            Ok(RecordedBlock {
                task: r.get(0)?,
                author: r.get(1)?,
                participant: r.get(2)?,
                material: r.get(3)?,
                evidence: r.get(4)?,
                content: r.get(5)?,
            })
        },
    )?;
    let local_controller = actor.grant.is_none() && row.task == actor.task;
    let mut item = json!({
        "ref": endpoint,
        "previous_version": endpoint.version.0.checked_sub(1).filter(|n|*n>0),
        "content": serde_json::from_str::<Value>(&row.content)?
    });
    let owner = actor.grant.is_none()
        && actor.allows(
            db,
            &Right::Revise {
                artifact: endpoint.artifact.clone(),
            },
        )?;
    if local_controller || owner {
        if let Some(record) = super::attribution::visible(db, actor, endpoint)? {
            item["attribution"] = record;
        }
        item["registered_by"] = json!(row.author);
        item["reported_participant"] = json!(row.participant);
        if let Some(id) = row.author.strip_prefix("agent:") {
            let grant = get(db, "provenance_grant", id)?;
            item["delegation"] =
                json!({"grant":id,"issuer":grant["issuer"],"target":grant["target"]});
        }
        let choice: Option<String> = db
            .query_row(
                "SELECT body FROM pv_choice WHERE artifact=? AND version=?",
                params![endpoint.artifact.0, endpoint.version.0],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(choice) = choice {
            let decision: Value = serde_json::from_str(&choice)?;
            if decision["task"] == actor.task {
                item["triggered"] = changes(
                    db,
                    &actor.task,
                    decision["id"].as_str().context("Missing receipt")?,
                )?;
                item["decision"] = decision;
            } else if owner {
                item["decision"] = json!({"id":decision["id"],"choice":decision["choice"],
                    "actor":decision["actor"],"grant":decision["grant"]});
            }
        }
    }
    if local_controller {
        if let Some(material) = row.material {
            let mut m: Value = serde_json::from_str(&material)?;
            item["file_flows"] = file_flows(db, &row.task, &m)?;
            if !super::material::visible_in_turn(db, actor, &m)? {
                if let Some(fields) = m.as_object_mut() {
                    fields.remove("execution");
                    fields.remove("bindings");
                }
            }
            item["material"] = m;
        }
    }
    let snapshots: Vec<String> = serde_json::from_str(&row.evidence)?;
    let mut visible = vec![];
    for snapshot in snapshots {
        if actor.allows(
            db,
            &Right::Evidence {
                snapshot: snapshot.clone(),
            },
        )? {
            visible.push(snapshot);
        }
    }
    item["evidence"] = json!(visible);
    Ok(item)
}

fn changes(db: &Connection, task: &str, receipt: &str) -> Result<Value> {
    let mut stmt=db.prepare(
        "SELECT cursor,revision,event FROM pv_point WHERE task=? AND
        coalesce(json_extract(event,'$.decision_receipt'),json_extract(event,'$.data.decision_receipt'))=? ORDER BY cursor"
    )?;
    let mut rows = stmt.query(params![task, receipt])?;
    let mut changes = Vec::new();
    while let Some(row) = rows.next()? {
        changes.push(json!({
            "cursor":row.get::<_,u64>(0)?,
            "revision":row.get::<_,u64>(1)?,
            "event":serde_json::from_str::<Value>(&row.get::<_,String>(2)?)?
        }));
    }
    Ok(json!(changes))
}

fn file_flows(db: &Connection, task: &str, m: &Value) -> Result<Value> {
    let mut stmt = db.prepare(include_str!("file_flows.sql"))?;
    let mut rows = stmt.query(params![
        task,
        m["revision"].as_str(),
        m["attempt"].as_str(),
        m["input"].as_str()
    ])?;
    let mut result = vec![];
    while let Some(r) = rows.next()? {
        result.push(json!({"seq":r.get::<_,i64>(0)?,"phase":r.get::<_,String>(1)?,"attempt":r.get::<_,String>(2)?,
        "target":r.get::<_,String>(3)?,"delivery":r.get::<_,Option<String>>(4)?,"time_ms":r.get::<_,f64>(5)?,"precision":"file"}));
    }
    Ok(json!(result))
}
