//! Evaluate one protected operation inside the caller's transaction.
use super::{context, decision_key};
use crate::store::{canonical, find, get, hash, put, uid};
use anyhow::{anyhow, ensure, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
pub fn gate(
    db: &Connection,
    task: &str,
    op: &str,
    scope: Value,
    details: Value,
    request: Option<&str>,
    flow: Option<Value>,
) -> Result<Option<Value>> {
    gate_flows(db, task, op, scope, details, request, flow.as_slice())
}
pub fn gate_flows(
    db: &Connection,
    task: &str,
    op: &str,
    scope: Value,
    details: Value,
    request: Option<&str>,
    required: &[Value],
) -> Result<Option<Value>> {
    gate_inner(db, task, op, scope, details, request, required, None)
}

#[allow(clippy::too_many_arguments)]
pub fn graph_gate(
    db: &Connection,
    task: &str,
    op: &str,
    scope: Value,
    details: Value,
    request: Option<&str>,
    flow: Value,
    route: Value,
) -> Result<Option<Value>> {
    gate_inner(
        db,
        task,
        op,
        scope,
        details,
        request,
        std::slice::from_ref(&flow),
        Some(&route),
    )
}

#[allow(clippy::too_many_arguments)]
fn gate_inner(
    db: &Connection,
    task: &str,
    op: &str,
    scope: Value,
    details: Value,
    request: Option<&str>,
    required: &[Value],
    route: Option<&Value>,
) -> Result<Option<Value>> {
    let (turn, flows, host) = context(db, task)?;
    let revisions = json!([turn, flows["revision"], host["revision"]]);
    if let Some(rules) = flows["flows"].as_array() {
        ensure!(
            required.iter().all(|flow| rules.contains(flow)),
            "Denied by workspace flow policy"
        );
    }
    let effect = canonical(&json!([op, scope, details]));
    let mut decision = host["default"].as_str().unwrap_or("unavailable");
    for e in host["effects"]
        .as_array()
        .ok_or_else(|| anyhow!("Invalid host policy"))?
    {
        if decision_key(e)? == effect {
            decision = e["decision"].as_str().unwrap_or("unavailable");
            break;
        }
    }
    ensure!(
        matches!(decision, "allow" | "ask"),
        "Host policy: {decision}"
    );
    let key = hash(canonical(&json!([revisions, task, op, scope])).as_bytes());
    let intent = match route {
        Some(actual) => crate::intent::authorize(db, task, &revisions, actual)?,
        None if !matches!(op, "log" | "preview" | "export") => {
            crate::intent::authorize(db, task, &revisions, &Value::Null)?
        }
        None => crate::intent::Match::Standard,
    };
    let waiting = intent == crate::intent::Match::Pending;
    let individual = intent == crate::intent::Match::Ask;
    let approved = matches!(intent, crate::intent::Match::Allow(_));
    if let Some(id) = request {
        let mut r = get(db, "request", id)?;
        ensure!(
            r["scope_key"] == key && canonical(&r["details"]) == canonical(&details),
            "Request changed"
        );
        if (r["state"] == "pending" && !approved)
            || (waiting && matches!(r["state"].as_str(), Some("once" | "turn")))
        {
            return Ok(Some(
                json!({"status":"permission_required","request_id":id}),
            ));
        }
        ensure!(
            matches!(r["state"].as_str(), Some("once" | "turn"))
                || (matches!(r["state"].as_str(), Some("pending" | "intent_approved")) && approved),
            "Request denied or already used"
        );
        r["state"] = json!("used");
        put(db, "request", id, &r)?;
        if !approved {
            return Ok(None);
        }
    }
    if let crate::intent::Match::Allow(id) = intent {
        put(
            db,
            "effect",
            &uid(),
            &json!({"task":task,"operation":op,"scope":scope,"details":details,"revisions":revisions,"authority":"intent","intent":id,"route":route}),
        )?;
        return Ok(None);
    }
    if !waiting && !individual && decision == "allow" {
        put(
            db,
            "effect",
            &uid(),
            &json!({"task":task,"operation":op,"scope":scope,"details":details,"revisions":revisions,"authority":"host"}),
        )?;
        return Ok(None);
    }
    if !waiting && !individual && find(db, "grant", &key)?.is_some() {
        return Ok(None);
    }
    let id = uid();
    put(
        db,
        "request",
        &id,
        &json!({"id":id,"task":task,"turn":turn,"operation":op,"revisions":revisions,"scope":scope,"scope_key":key,"details":details,"state":"pending","route":route}),
    )?;
    Ok(Some(
        json!({"status":"permission_required","request_id":id}),
    ))
}
