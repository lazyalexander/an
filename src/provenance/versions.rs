use super::{
    access::{Actor, Right},
    domain::*,
    material,
};
use crate::{
    artifacts,
    store::{get, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub fn commit(tx: &Transaction, actor: &Actor, batch: Batch) -> Result<Value> {
    ensure!(!batch.versions.is_empty(), "Empty submission");
    let mut created = BTreeSet::new();
    let mut receipt = Vec::new();
    for draft in &batch.versions {
        ensure!(
            valid_name(&draft.artifact.0) && created.insert(&draft.artifact),
            "Invalid or repeated artifact ID"
        );
        receipt.push(insert(
            tx,
            actor,
            draft,
            batch.reported_participant.as_deref(),
        )?);
    }
    // Resolve the whole batch before commit. References within this batch
    // may point forward, subject to the same authority and cycle checks.
    for (draft, item) in batch.versions.iter().zip(&receipt) {
        let version = item["version"].as_u64().unwrap();
        for block in &draft.blocks {
            for citation in &block.cites {
                let src = &citation.source;
                actor.require(
                    tx,
                    Right::Read {
                        artifact: src.artifact.clone(),
                        version: src.version,
                    },
                )?;
                let source: String = tx
                    .query_row(
                        "SELECT body FROM pv_block WHERE artifact=? AND version=? AND block=?",
                        params![src.artifact.0, src.version.0, src.block],
                        |r| r.get(0),
                    )
                    .optional()?
                    .with_context(|| format!("Unresolved source for {}", block.id))?;
                if citation.relation == Relation::Quotes {
                    ensure!(
                        serde_json::from_str::<Value>(&source)?["text"] == block.text,
                        "Quoted bytes differ at {}",
                        block.id
                    );
                }
                tx.execute(
                    "INSERT INTO pv_edge VALUES(?,?,?,?,?,?,?)",
                    params![
                        draft.artifact.0,
                        version,
                        block.id,
                        src.artifact.0,
                        src.version.0,
                        src.block,
                        serde_json::to_value(citation.relation)?.as_str()
                    ],
                )?;
            }
        }
    }
    for item in &receipt {
        let cycle:bool=tx.query_row("WITH RECURSIVE path(start,a,v,b) AS (
             SELECT block,source,source_version,source_block FROM pv_edge WHERE artifact=?1 AND version=?2
             UNION SELECT p.start,e.source,e.source_version,e.source_block FROM path p JOIN pv_edge e
               ON (e.artifact,e.version,e.block)=(p.a,p.v,p.b))
             SELECT EXISTS(SELECT 1 FROM path WHERE (a,v,b)=(?1,?2,start))",
             params![item["artifact"].as_str(),item["version"].as_u64()],|r|r.get(0))?;
        ensure!(!cycle, "Circular block reference");
    }
    Ok(json!({"id":uid(),"versions":receipt}))
}

fn insert(
    tx: &Transaction,
    actor: &Actor,
    draft: &Draft,
    participant: Option<&str>,
) -> Result<Value> {
    let id = &draft.artifact.0;
    let kind = serde_json::to_value(draft.kind)?;
    let version = draft
        .expected
        .0
        .checked_add(1)
        .context("Version overflow")?;
    if draft.expected.0 == 0 {
        actor.controller()?;
        tx.execute(
            "INSERT INTO pv_artifact VALUES(?,?,?,?)",
            params![id, actor.task, kind.as_str(), version],
        )
        .context("Artifact already registered")?;
    } else {
        actor.require(
            tx,
            Right::Revise {
                artifact: draft.artifact.clone(),
            },
        )?;
        ensure!(
            tx.execute(
                "UPDATE pv_artifact SET head=? WHERE id=? AND head=? AND kind=?",
                params![version, id, draft.expected.0, kind.as_str()]
            )? == 1,
            "Version conflict"
        );
    }
    let mut material = draft
        .material
        .as_ref()
        .map(|m| material::admitted(tx, actor, m))
        .transpose()?;
    let file = match (&mut material, &draft.file) {
        (Some(m), Some(path)) => {
            let files = get(
                tx,
                "revision",
                m["revision"].as_str().context("Missing revision")?,
            )?;
            m["file"] = json!(path);
            Some(artifacts::bytes(
                files.get(path).context("Unknown material file")?,
            )?)
        }
        (None, None) => None,
        _ => anyhow::bail!("Expected material and file together"),
    };
    for snapshot in &draft.evidence {
        actor.require(
            tx,
            Right::Evidence {
                snapshot: snapshot.clone(),
            },
        )?;
    }
    ensure!(!draft.blocks.is_empty(), "Expected content blocks");
    let cursor:u64=tx.query_row("SELECT coalesce((SELECT json_array_length(body,'$.events') FROM objects WHERE kind='graph' AND id=?),0)+1",[&actor.task],|r|r.get(0))?;
    tx.execute(
        "INSERT INTO pv_version VALUES(?,?,?,?,?,?,?,?)",
        params![
            id,
            version,
            actor.task,
            cursor,
            actor.id,
            participant,
            material.as_ref().map(Value::to_string),
            serde_json::to_string(&draft.evidence)?
        ],
    )?;
    let mut names = BTreeSet::new();
    for block in &draft.blocks {
        ensure!(
            valid_name(&block.id) && names.insert(&block.id),
            "Invalid or duplicate block ID"
        );
        ensure!(
            block.original == block.cites.is_empty(),
            "Declare a signed original or citations for {}",
            block.id
        );
        match (&file, block.bytes) {
            (Some(data), Some([start, end])) => ensure!(
                data.get(start..end) == Some(block.text.as_bytes()),
                "Invalid UTF-8 byte range for {}",
                block.id
            ),
            (None, None) => (),
            _ => anyhow::bail!("Expected a material range for {}", block.id),
        }
        // Edges have their own visibility rules. Never duplicate endpoints in a readable block.
        tx.execute(
            "INSERT INTO pv_block VALUES(?,?,?,?)",
            params![
                id,
                version,
                block.id,
                json!({"text":block.text,"bytes":block.bytes,"original":block.original})
                    .to_string()
            ],
        )?;
    }
    Ok(json!({"artifact":id,"version":version}))
}
