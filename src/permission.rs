use crate::store::*;
use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::{Parser, ValueEnum};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::{collections::HashSet, env, fs, path::PathBuf};
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(clap::Subcommand)]
enum Commands {
    Permission {
        #[arg(value_enum)]
        action: Action,
        #[arg(long)]
        task: String,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long)]
        decision: Option<String>,
    },
}
#[derive(Clone, ValueEnum)]
enum Action {
    Begin,
    End,
    Pending,
    Preview,
    Decide,
    Policy,
    Effect,
}
pub fn current(db: &Connection, task: &str) -> Result<String> {
    let turn = env::var("AN_TURN").unwrap_or_default();
    ensure!(
        !turn.is_empty() && get(db, "turn", task)? == turn,
        "Active host turn required"
    );
    Ok(turn)
}
fn context(db: &Connection, task: &str) -> Result<(String, Value, Value)> {
    let turn = current(db, task)?;
    let t = get(db, "task", task)?;
    let flows = if let Some(w) = t["workspace"].as_str() {
        get(db, "workspace", w)?.get("policy").cloned()
    } else {
        None
    }
    .unwrap_or(json!({"revision":"legacy"}));
    let host = find(db, "host_policy", &turn)?
        .unwrap_or(json!({"revision":"legacy","default":"ask","effects":[]}));
    Ok((turn, flows, host))
}
pub fn stamp(db: &Connection, task: &str) -> Result<Value> {
    let (turn, flows, host) = context(db, task)?;
    Ok(json!([turn, flows["revision"], host["revision"]]))
}
fn decision_key(effect: &Value) -> Result<String> {
    for field in ["operation", "scope", "details"] {
        ensure!(effect.get(field).is_some(), "Missing effect field {field}");
    }
    Ok(canonical(&json!([
        effect["operation"],
        effect["scope"],
        effect["details"]
    ])))
}
fn set_host(db: &Connection, turn: &str, mut config: Value) -> Result<String> {
    ensure!(
        matches!(
            config["default"].as_str(),
            Some("ask" | "unavailable" | "deny")
        ),
        "Invalid host default"
    );
    let mut seen = HashSet::new();
    for e in config["effects"]
        .as_array()
        .ok_or_else(|| anyhow!("Expected effects list"))?
    {
        ensure!(
            matches!(
                e["decision"].as_str(),
                Some("ask" | "unavailable" | "deny" | "allow")
            ) && seen.insert(decision_key(e)?),
            "Invalid or duplicate host decision"
        );
    }
    let rev = uid();
    config["revision"] = json!(rev);
    put(db, "host_policy", turn, &config)?;
    Ok(rev)
}
pub fn gate(
    db: &Connection,
    task: &str,
    op: &str,
    scope: Value,
    details: Value,
    request: Option<&str>,
    flow: Option<Value>,
) -> Result<Option<Value>> {
    let (turn, flows, host) = context(db, task)?;
    let revisions = json!([turn, flows["revision"], host["revision"]]);
    if let (Some(flow), Some(rules)) = (flow, flows["flows"].as_array()) {
        ensure!(rules.contains(&flow), "Denied by workspace flow policy");
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
    if let Some(id) = request {
        let mut r = get(db, "request", id)?;
        ensure!(
            r["scope_key"] == key && canonical(&r["details"]) == canonical(&details),
            "Request changed"
        );
        if r["state"] == "pending" {
            return Ok(Some(
                json!({"status":"permission_required","request_id":id}),
            ));
        }
        ensure!(
            matches!(r["state"].as_str(), Some("once" | "turn")),
            "Request denied or already used"
        );
        r["state"] = json!("used");
        put(db, "request", id, &r)?;
        return Ok(None);
    }
    if decision == "allow" {
        put(
            db,
            "effect",
            &uid(),
            &json!({"task":task,"operation":op,"scope":scope,"details":details,"revisions":revisions,"authority":"host"}),
        )?;
        return Ok(None);
    }
    if find(db, "grant", &key)?.is_some() {
        return Ok(None);
    }
    let id = uid();
    put(
        db,
        "request",
        &id,
        &json!({"id":id,"task":task,"turn":turn,"operation":op,"revisions":revisions,"scope":scope,"scope_key":key,"details":details,"state":"pending"}),
    )?;
    Ok(Some(
        json!({"status":"permission_required","request_id":id}),
    ))
}
pub fn execute() -> Result<Value> {
    let Commands::Permission {
        action,
        task,
        file,
        request_id,
        decision,
    } = Cli::parse().command;
    let mut db = connect()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    get(&tx, "task", &task)?;
    let read_policy = || -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(
            file.as_ref()
                .ok_or_else(|| anyhow!("Expected policy file"))?,
        )?)?)
    };
    let result = if matches!(action, Action::Begin) {
        let turn = uid();
        put(&tx, "turn", &task, &json!(turn))?;
        if file.is_some() {
            set_host(&tx, &turn, read_policy()?)?;
        }
        json!({"turn":turn})
    } else {
        let turn = current(&tx, &task)?;
        match action {
            Action::Begin => unreachable!(),
            Action::End => {
                put(&tx, "turn", &task, &Value::Null)?;
                json!({"closed":true})
            }
            Action::Policy => json!(set_host(&tx, &turn, read_policy()?)?),
            Action::Pending => {
                let rev = stamp(&tx, &task)?;
                let mut stmt=tx.prepare("SELECT body FROM objects WHERE kind='request' AND json_extract(body,'$.turn')=? AND json_extract(body,'$.state')='pending' ORDER BY rowid")?;
                let mut rows = stmt.query([turn])?;
                let mut items = vec![];
                while let Some(row) = rows.next()? {
                    let r: Value = serde_json::from_str(&row.get::<_, String>(0)?)?;
                    if r["revisions"] == rev {
                        items.push(r);
                    }
                }
                json!(items)
            }
            Action::Preview => {
                let id = request_id.context("Expected request id")?;
                let r = get(&tx, "request", &id)?;
                ensure!(
                    r["task"] == task
                        && r["state"] == "pending"
                        && r["revisions"] == stamp(&tx, &task)?
                        && r["operation"] == "mcp",
                    "Expected current pending bridge request"
                );
                let scope = &r["scope"];
                let server = scope[0].as_str().context("Invalid bridge")?;
                let raw = scope[1].as_str().context("Invalid tool")?;
                ensure!(
                    get(&tx, "server", server)?["revision"] == scope[2]
                        && get(&tx, "task", &task)?["tools"][format!("{server}/{raw}")] == scope[2],
                    "Bridge grant changed"
                );
                let revision = r["details"]["source_revision"]
                    .as_str()
                    .context("Request has no artifact source")?;
                let source = crate::transfer::source(&tx, &task, &scope[3])?;
                ensure!(
                    source.as_ref().is_some_and(|(rev, _)| rev == revision),
                    "Bridge candidate changed"
                );
                json!({"revision":revision, "files":get(&tx,"revision",revision)?})
            }
            Action::Decide | Action::Effect => {
                let id = request_id.ok_or_else(|| anyhow!("Expected request id"))?;
                let decision = decision.ok_or_else(|| anyhow!("Expected decision"))?;
                let mut r = get(&tx, "request", &id)?;
                ensure!(
                    r["turn"] == turn && r["task"] == task && r["state"] == "pending",
                    "Invalid or stale decision"
                );
                ensure!(r["revisions"] == stamp(&tx, &task)?, "Policy changed");
                if matches!(action, Action::Effect) {
                    let (_, _, mut host) = context(&tx, &task)?;
                    let key = decision_key(&r)?;
                    let effects = host["effects"]
                        .as_array_mut()
                        .ok_or_else(|| anyhow!("Invalid host effects"))?;
                    effects.retain(|e| decision_key(e).ok() != Some(key.clone()));
                    effects.push(json!({"operation":r["operation"],"scope":r["scope"],"details":r["details"],"decision":decision}));
                    json!({"revision":set_host(&tx,&turn,host)?})
                } else {
                    if !matches!(decision.as_str(), "deny" | "turn" | "once") {
                        bail!("Invalid user decision")
                    }
                    r["state"] = json!(decision);
                    put(&tx, "request", &id, &r)?;
                    if decision == "turn" {
                        put(
                            &tx,
                            "grant",
                            r["scope_key"]
                                .as_str()
                                .ok_or_else(|| anyhow!("Invalid scope"))?,
                            &json!(true),
                        )?;
                    }
                    json!({"request_id":id,"decision":decision})
                }
            }
        }
    };
    tx.commit()?;
    Ok(result)
}
