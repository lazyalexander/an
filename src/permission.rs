mod flows;
use crate::store::*;
use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::{Parser, ValueEnum};
pub use flows::{gate, gate_flows, graph_gate};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
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
    Delegate,
    End,
    Pending,
    Preview,
    Decide,
    Policy,
    Effect,
    IntentPropose,
    IntentShow,
    IntentDecide,
}
pub fn current(db: &Connection, task: &str) -> Result<String> {
    let active = get(db, "turn", task)?;
    let epoch = active.as_str().context("Active host turn required")?;
    let host = env::var("AN_TURN").unwrap_or_default();
    if host_valid(db, task, &host)? {
        return Ok(epoch.to_owned());
    }
    if matches!(
        env::args().nth(1).as_deref(),
        Some("mcp" | "_worker" | "_model-worker")
    ) {
        let token = env::var("AN_AGENT_TOKEN").unwrap_or_default();
        if !token.is_empty()
            && find(db, "agent_auth", &hash(token.as_bytes()))? == Some(json!([task, epoch]))
        {
            return Ok(epoch.to_owned());
        }
    }
    bail!("Active host turn required")
}

pub(crate) fn controller(db: &Connection, task: &str) -> Result<()> {
    ensure!(
        get(db, "turn", task)?.is_string()
            && host_valid(db, task, &env::var("AN_TURN").unwrap_or_default())?,
        "Active controller credential required"
    );
    Ok(())
}

fn host_valid(db: &Connection, task: &str, secret: &str) -> Result<bool> {
    if secret.is_empty() {
        return Ok(false);
    }
    let digest = json!(hash(secret.as_bytes()));
    if let Some(keys) = find(db, "host_auth", task)? {
        return Ok(keys["controller"] == digest || keys["turn"] == digest);
    }
    Ok(find(db, "turn", task)? == Some(json!(secret)))
}

pub fn cursors(args: Value) -> Result<Value> {
    let mut db = connect()?;
    let tx = db.transaction()?;
    let mut result = json!({});
    for (task, secret) in args.as_object().context("Expected task credentials")? {
        let Some(record) = find(&tx, "task", task)? else {
            continue;
        };
        let epoch = find(&tx, "turn", task)?.unwrap_or(Value::Null);
        let valid = host_valid(&tx, task, secret.as_str().unwrap_or(""))?;
        let (cursor,paused): (i64,bool) = tx.query_row(
            "SELECT coalesce(json_array_length(body,'$.events'),0), coalesce(json_extract(body,'$.control')='paused',0) FROM objects WHERE kind='graph' AND id=?",
            [task], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.unwrap_or_default();
        result[task] = json!({"closed":record["closed"]==true,"ended":epoch.is_null(),"active":valid,
            "generation":hash(canonical(&epoch).as_bytes()),"cursor":cursor,"paused":paused});
    }
    Ok(result)
}
fn context(db: &Connection, task: &str) -> Result<(String, Value, Value)> {
    policy_context(db, task, current(db, task)?)
}
fn policy_context(db: &Connection, task: &str, turn: String) -> Result<(String, Value, Value)> {
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
/// Compare the scope of a previously issued delegation without treating its
/// bearer as the task controller.
pub(crate) fn delegation_scope(db: &Connection, task: &str) -> Result<Value> {
    let epoch = get(db, "turn", task)?;
    let Some(epoch) = epoch.as_str() else {
        return Ok(Value::Null);
    };
    let (turn, flows, host) = policy_context(db, task, epoch.to_owned())?;
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
pub fn execute() -> Result<Value> {
    let Commands::Permission {
        action,
        task,
        file,
        request_id,
        decision,
    } = Cli::parse().command;
    let mut db = connect()?;
    // The decision can append a graph event. Serialize with scheduler writers,
    // which hold this lock even between their SQLite transactions.
    let graph_lock = matches!(action, Action::IntentDecide)
        .then(|| crate::instances::lock(&db, &task))
        .transpose()?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    get(&tx, "task", &task)?;
    let read_policy = || -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(
            file.as_ref()
                .ok_or_else(|| anyhow!("Expected policy file"))?,
        )?)?)
    };
    let wake_graph = matches!(
        action,
        Action::Decide | Action::Policy | Action::Effect | Action::IntentDecide
    );
    let result = if matches!(action, Action::Begin) {
        ensure!(get(&tx, "task", &task)?["closed"] != true, "Task is closed");
        let previous = find(&tx, "host_auth", &task)?;
        if previous.is_some() || find(&tx, "turn", &task)?.is_some_and(|v| v.is_string()) {
            ensure!(
                host_valid(&tx, &task, &env::var("AN_TURN").unwrap_or_default())?,
                "Controller credential required"
            );
        }
        let epoch = uid();
        let turn = uid();
        let controller = uid();
        let owner = previous
            .as_ref()
            .map(|k| k["controller"].clone())
            .unwrap_or_else(|| json!(hash(controller.as_bytes())));
        put(
            &tx,
            "host_auth",
            &task,
            &json!({"controller":owner,"turn":hash(turn.as_bytes())}),
        )?;
        put(&tx, "turn", &task, &json!(epoch))?;
        if file.is_some() {
            set_host(&tx, &epoch, read_policy()?)?;
        }
        let mut result = json!({"turn":turn});
        if previous.is_none() {
            result["controller"] = json!(controller);
        }
        result
    } else {
        let turn = current(&tx, &task)?;
        match action {
            Action::Begin => unreachable!(),
            Action::Delegate => {
                ensure!(
                    find(&tx, "host_auth", &task)?.is_some(),
                    "Begin a new turn before delegating a legacy credential"
                );
                let token =
                    hash(canonical(&json!(["agent", env::var("AN_TURN")?, turn])).as_bytes());
                put(
                    &tx,
                    "agent_auth",
                    &hash(token.as_bytes()),
                    &json!([task, turn]),
                )?;
                json!({"agent_token":token})
            }
            Action::IntentShow => crate::intent::public(crate::intent::show(&tx, &task)?),
            Action::IntentPropose => {
                crate::intent::public(crate::intent::propose(&tx, &task, read_policy()?)?)
            }
            Action::IntentDecide => {
                let id = request_id.context("Expected intent ID")?;
                let decisions = if let Some(value) = decision {
                    let count = crate::intent::show(&tx, &task)?["routes"]
                        .as_array()
                        .context("Stale intent")?
                        .len();
                    if matches!(value.as_str(), "allow" | "deny") {
                        json!(vec![value; count])
                    } else {
                        serde_json::from_str(&value)?
                    }
                } else {
                    read_policy()?
                };
                crate::intent::public(crate::intent::decide(&tx, &task, &id, decisions)?)
            }
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
    drop(graph_lock);
    if wake_graph {
        crate::graph::wake(&task)?;
    }
    Ok(result)
}
