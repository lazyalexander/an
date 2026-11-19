use crate::store::*;
use anyhow::{anyhow, bail, ensure, Result};
use clap::{Args, Parser, Subcommand};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Workspace {
        #[command(subcommand)]
        action: Workspace,
    },
    Task {
        #[command(subcommand)]
        action: Task,
    },
    Deliveries {
        #[arg(long)]
        task: String,
        #[arg(long, default_value_t = 0)]
        after: i64,
    },
}
#[derive(Subcommand)]
enum Workspace {
    Create {
        #[command(flatten)]
        grants: Grants,
    },
    Bind {
        workspace: String,
        #[command(flatten)]
        selector: Selector,
    },
    Policy {
        #[command(subcommand)]
        action: Policy,
    },
}
#[derive(Subcommand)]
enum Policy {
    Show {
        workspace: String,
    },
    Set {
        workspace: String,
        #[arg(long)]
        file: PathBuf,
    },
}
#[derive(Subcommand)]
enum Task {
    New {
        #[command(flatten)]
        grants: Grants,
        #[command(flatten)]
        selector: Selector,
        #[arg(long)]
        input: Vec<PathBuf>,
    },
    Resume {
        delivery: String,
        #[arg(long)]
        editable: Vec<String>,
    },
}
#[derive(Args, Default)]
struct Grants {
    #[arg(long)]
    spool: Vec<String>,
    #[arg(long)]
    editable: Vec<String>,
    #[arg(long)]
    allow_tool: Vec<String>,
}
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
fn config(args: Value) -> Result<Value> {
    let env = json!({"AN_HOME":root()?});
    Ok(json!({"an":{"command":std::env::current_exe()?,"args":args,"env":env}}))
}
fn resolve_grants(db: &Connection, g: &Grants) -> Result<(Vec<String>, Value)> {
    ensure!(
        g.editable.iter().all(|e| g.spool.contains(e)),
        "Editable spool must be granted"
    );
    let mut images = vec![];
    for r in &g.spool {
        images.push(
            get(db, "image", r)?
                .as_str()
                .ok_or_else(|| anyhow!("Invalid image record"))?
                .to_owned(),
        );
    }
    let mut tools = json!({});
    for t in &g.allow_tool {
        let (server, tool) = t
            .split_once('/')
            .ok_or_else(|| anyhow!("Expected SERVER/TOOL"))?;
        ensure!(!tool.is_empty(), "Expected tool name");
        tools[t] = get(db, "server", server)?["revision"].clone();
    }
    Ok((images, tools))
}
fn new_task(
    db: &Connection,
    images: &[String],
    editable: &[String],
    inputs: Value,
) -> Result<(String, Value)> {
    ensure!(
        editable.iter().all(|s| images.contains(s)),
        "Editable spool must be granted"
    );
    let mut work = json!({});
    for image in images {
        get(db, "image", image)?;
        work[image] = json!(revision(db, &json!({}))?);
    }
    Ok((
        uid(),
        json!({"work":work,"inputs":inputs,"editable":editable}),
    ))
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
pub fn resolve(db: &Connection, identity: &str, fresh: bool) -> Result<String> {
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
pub fn execute() -> Result<Value> {
    let cli = Cli::parse();
    let mut db = connect()?;
    let mut lock = None;
    if let Command::Task {
        action: Task::New { selector, .. },
    } = &cli.command
    {
        if let Some(id) = selector.identity()? {
            lock = Some(guard(&id, true)?);
        }
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = match cli.command {
        Command::Deliveries { task, after } => deliveries(&tx, &task, after)?,
        Command::Workspace {
            action: Workspace::Create { grants },
        } => {
            ensure!(!grants.spool.is_empty(), "Grant a spool");
            let (images, tools) = resolve_grants(&tx, &grants)?;
            let id = uid();
            put(
                &tx,
                "workspace",
                &id,
                &json!({"images":images,"editable":grants.editable,"tools":tools}),
            )?;
            json!({"workspace":id})
        }
        Command::Workspace {
            action:
                Workspace::Bind {
                    workspace,
                    selector,
                },
        } => {
            let key = selector
                .identity()?
                .ok_or_else(|| anyhow!("Expected bound identity"))?;
            get(&tx, "workspace", &workspace)?;
            if let Some(b) = find(&tx, "binding", &key)? {
                ensure!(
                    b["workspace"] == workspace,
                    "Identity already bound to another workspace"
                );
            } else {
                put(
                    &tx,
                    "binding",
                    &key,
                    &json!({"workspace":workspace,"task":null}),
                )?;
            }
            json!({"workspace":workspace,"mcp_servers":config(selector.args())?})
        }
        Command::Workspace {
            action: Workspace::Policy { action },
        } => {
            let (id, rules) = match action {
                Policy::Show { workspace } => (workspace, None),
                Policy::Set { workspace, file } => (
                    workspace,
                    Some(serde_json::from_slice::<Value>(&fs::read(file)?)?),
                ),
            };
            let mut cfg = get(&tx, "workspace", &id)?;
            if let Some(rules) = rules {
                let images = cfg["images"]
                    .as_array()
                    .ok_or_else(|| anyhow!("Invalid workspace"))?;
                for row in rules
                    .as_array()
                    .ok_or_else(|| anyhow!("Expected flow list"))?
                {
                    let row = row.as_array().ok_or_else(|| anyhow!("Invalid flow"))?;
                    ensure!(row.len() == 3, "Invalid flow");
                    let (s, d) = (&row[1], &row[2]);
                    let valid = match row[0].as_str() {
                        Some("input") => s.is_string() && images.contains(d),
                        Some("bridge_input") => {
                            s.as_str().is_some_and(|k| cfg["tools"].get(k).is_some())
                                && images.contains(d)
                        }
                        Some("bridge_send") => {
                            images.contains(s)
                                && d.as_str().is_some_and(|k| cfg["tools"].get(k).is_some())
                        }
                        Some("handoff") => images.contains(s) && images.contains(d),
                        Some("publish") => images.contains(s) && d == &id,
                        Some("export") => {
                            images.contains(s) && d.as_str().is_some_and(|v| v.starts_with('/'))
                        }
                        _ => false,
                    };
                    ensure!(valid, "Invalid workspace flow");
                }
                cfg["policy"] = json!({"revision":uid(),"flows":rules});
                put(&tx, "workspace", &id, &cfg)?;
            }
            cfg.get("policy")
                .cloned()
                .unwrap_or(json!({"revision":"legacy"}))
        }
        Command::Task {
            action:
                Task::New {
                    grants,
                    selector,
                    input,
                },
        } => {
            if let Some(key) = selector.identity()? {
                ensure!(
                    grants.spool.is_empty()
                        && grants.allow_tool.is_empty()
                        && grants.editable.is_empty()
                        && input.is_empty(),
                    "Bound tasks inherit workspace configuration"
                );
                json!({"task":resolve(&tx,&key,true)?})
            } else {
                if grants.spool.is_empty() && grants.allow_tool.is_empty() {
                    bail!("Grant a spool or a downstream tool")
                }
                let (images, tools) = resolve_grants(&tx, &grants)?;
                let (id, mut t) = new_task(&tx, &images, &grants.editable, inputs(&tx, &input)?)?;
                if !grants.allow_tool.is_empty() {
                    t["tools"] = tools;
                }
                put(&tx, "task", &id, &t)?;
                json!({"task":id,"mcp_servers":config(json!(["mcp","--task",id]))?})
            }
        }
        Command::Task {
            action: Task::Resume { delivery, editable },
        } => {
            let r = get(&tx, "run", &delivery)?;
            ensure!(r["status"] == "delivered", "Expected a delivered run");
            let image = r["spool"]
                .as_str()
                .ok_or_else(|| anyhow!("Invalid spool"))?
                .to_owned();
            let (id, mut t) = new_task(
                &tx,
                std::slice::from_ref(&image),
                &editable,
                json!({"original":r["input"]}),
            )?;
            t["work"][image] = r.get("result_work").unwrap_or(&r["work"]).clone();
            put(&tx, "task", &id, &t)?;
            json!({"task":id,"mcp_servers":config(json!(["mcp","--task",id]))?})
        }
    };
    tx.commit()?;
    drop(lock);
    Ok(result)
}
