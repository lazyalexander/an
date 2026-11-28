use super::{
    access::{Actor, Right},
    domain::{Choice, Decision},
};
use crate::{
    graph,
    store::{find, get, put, uid},
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

fn revision(db: &Connection, task: &str, cursor: u64) -> Result<u64> {
    db.query_row(
        "SELECT revision FROM pv_point WHERE task=? AND cursor=?",
        params![task, cursor],
        |r| r.get(0),
    )
    .optional()?
    .context("Graph history unavailable at cursor")
}

pub fn view(db: &Connection, actor: &Actor, cursor: u64) -> Result<Value> {
    ensure!(
        actor.may_observe_position(),
        "Graph position outside authority"
    );
    let rev = revision(db, &actor.task, cursor)?;
    if actor.grant.is_some() {
        return Ok(json!({"task":actor.task,"cursor":cursor,"revision":rev}));
    }
    let mut stmt = db.prepare(include_str!("history.sql"))?;
    let mut rows = stmt.query(params![actor.task, cursor])?;
    let mut nodes = json!({});
    while let Some(r) = rows.next()? {
        nodes[r.get::<_, String>(0)?] = serde_json::from_str(&r.get::<_, String>(1)?)?;
    }
    let (control, event): (Option<String>, Option<String>) = db.query_row(
        "SELECT control,event FROM pv_point WHERE task=? AND cursor=?",
        params![actor.task, cursor],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut choices=db.prepare("SELECT v.artifact,v.version,CASE WHEN json_extract(c.body,'$.task')=?1 AND
        json_extract(c.body,'$.accepted.cursor')<=?2 THEN json_extract(c.body,'$.choice') ELSE 'proposed' END
        FROM pv_version v JOIN pv_artifact a ON a.id=v.artifact LEFT JOIN pv_choice c USING(artifact,version)
        WHERE a.kind='decision' AND v.task=?1 AND v.registered_cursor<=?2 ORDER BY v.artifact,v.version")?;
    let decisions=choices.query_map(params![actor.task,cursor],|r|Ok(json!({"artifact":r.get::<_,String>(0)?,"version":r.get::<_,u64>(1)?,"state":r.get::<_,String>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(
        json!({"decisions":decisions,"task":actor.task,"cursor":cursor,"revision":rev,"nodes":nodes,"control":control,
        "event":event.map(|s|serde_json::from_str::<Value>(&s)).transpose()?}),
    )
}

pub fn decide(tx: &Transaction, actor: &Actor, decision: Decision) -> Result<Value> {
    let required = match &decision.review {
        Some(report) => {
            ensure!(
                matches!(decision.choice, Choice::Reject),
                "Report-bound decision must reject"
            );
            Right::RejectWithReport {
                artifact: decision.artifact.clone(),
                version: decision.version,
                report: report.clone(),
            }
        }
        None => Right::Decide {
            artifact: decision.artifact.clone(),
            version: decision.version,
        },
    };
    actor.require(tx, required)?;
    let kind: String = tx.query_row(
        "SELECT kind FROM pv_artifact JOIN pv_version ON pv_artifact.id=pv_version.artifact
        WHERE artifact=? AND version=?",
        params![decision.artifact.0, decision.version.0],
        |r| r.get(0),
    )?;
    ensure!(kind == "decision", "Expected decision version");
    let mut graph = find(tx, "graph", &actor.task)?.unwrap_or_else(graph::empty);
    let (proposal_task, registered): (String, u64) = tx.query_row(
        "SELECT task,registered_cursor FROM pv_version WHERE artifact=? AND version=?",
        params![decision.artifact.0, decision.version.0],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ensure!(
        proposal_task != actor.task || registered <= decision.seen_cursor,
        "Proposal did not exist at this cursor"
    );
    let seen = revision(tx, &actor.task, decision.seen_cursor)?;
    let id = uid();
    graph::event(
        &mut graph,
        "host",
        "decision_recorded",
        &actor.task,
        json!({"receipt":id}),
    );
    let event = graph["events"]
        .as_array()
        .context("Missing graph events")?
        .last()
        .context("Missing decision event")?;
    let mut receipt = json!({"id":id,"artifact":decision.artifact,"version":decision.version,"choice":decision.choice,
        "actor":actor.id,"grant":actor.grant,"reported_participant":decision.reported_participant,
        "reason":decision.reason,"task":actor.task,"time_ms":event["time_ms"],
        "seen":{"revision":seen,"cursor":decision.seen_cursor},
        "accepted":{"revision":graph["revision"],"cursor":event["cursor"]}});
    if let Some(grant) = &actor.grant {
        receipt["delegated_by"] = get(tx, "provenance_grant", grant)?["issuer"].clone();
    }
    if let Some(report) = decision.review {
        receipt["review"] = serde_json::to_value(report)?;
    }
    tx.execute(
        "INSERT INTO pv_choice VALUES(?,?,?,?)",
        params![
            decision.artifact.0,
            decision.version.0,
            id,
            receipt.to_string()
        ],
    )
    .context("Decision version already settled")?;
    put(tx, "graph", &actor.task, &graph)?;
    Ok(receipt)
}

pub fn require_accepted(db: &Connection, task: &str, id: &str) -> Result<()> {
    let valid: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pv_choice WHERE receipt=? AND json_extract(body,'$.task')=?
        AND json_extract(body,'$.choice')='accept')",
        params![id, task],
        |r| r.get(0),
    )?;
    ensure!(valid, "Expected an accepted decision in this task");
    Ok(())
}

// Registration is itself a graph event. A decision cannot claim to have seen
// an earlier position than the first appearance of the proposal in that task.
pub fn proposed(tx: &Connection, task: &str, receipt: &Value) -> Result<()> {
    let mut graph = find(tx, "graph", task)?.unwrap_or_else(graph::empty);
    graph::event(
        &mut graph,
        "host",
        "versions_registered",
        task,
        json!({"receipt":receipt["id"]}),
    );
    put(tx, "graph", task, &graph)
}
