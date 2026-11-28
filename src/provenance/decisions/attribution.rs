//! Preserve what a source reports without issuing an executable decision receipt.
use crate::provenance::{
    access::{Actor, Right},
    domain::{Attribution, Block, BlockRef, Relation, ReportedOutcome, VersionRef},
};
use anyhow::{ensure, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub fn save(
    db: &Connection,
    version: &VersionRef,
    blocks: &[Block],
    record: &Attribution,
) -> Result<()> {
    let cited = |source: &BlockRef, relation| {
        blocks
            .iter()
            .flat_map(|block| &block.cites)
            .any(|citation| citation.source == *source && citation.relation == relation)
    };
    ensure!(
        cited(&record.statement, Relation::Quotes),
        "Expected quoted source statement"
    );
    if let Some(authority) = &record.authority {
        ensure!(
            *authority != record.statement && cited(authority, Relation::BasedOn),
            "Expected separate authority citation"
        );
    }
    db.execute(
        "INSERT INTO pv_attribution VALUES(?,?,?,unixepoch())",
        params![
            version.artifact.0,
            version.version.0,
            serde_json::to_string(record)?
        ],
    )?;
    Ok(())
}

pub fn visible(db: &Connection, actor: &Actor, endpoint: &BlockRef) -> Result<Option<Value>> {
    let row: Option<(String, i64)> = db
        .query_row(
            "SELECT record, registered_at FROM pv_attribution WHERE artifact=? AND version=?",
            params![endpoint.artifact.0, endpoint.version.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((body, registered_at)) = row else {
        return Ok(None);
    };
    let record: Attribution = serde_json::from_str(&body)?;
    for source in std::iter::once(&record.statement).chain(&record.authority) {
        if !actor.allows(
            db,
            &Right::Read {
                artifact: source.artifact.clone(),
                version: source.version,
            },
        )? {
            return Ok(None);
        }
    }
    let state = match record.outcome {
        ReportedOutcome::Suggested => "proposed",
        ReportedOutcome::Approved
            if record.authority.is_some()
                && record
                    .speaker
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty()) =>
        {
            "recorded_approval"
        }
        ReportedOutcome::Approved => "unverified",
    };
    Ok(Some(
        json!({"state":state,"source":record,"registered_at":registered_at}),
    ))
}
